# crate-coco-sandbox

Sandbox enforcement crate. Two-module design that mirrors TS's
adapter-over-runtime split: a thin Claude-specific **adapter** sits on
top of a generic **runtime** that does the actual platform wrapping.

## TS Source

- `utils/sandbox/sandbox-adapter.ts` (985 LOC) — the adapter layer
- `tools/BashTool/shouldUseSandbox.ts` (154 LOC) — decision logic
- `entrypoints/sandboxTypes.ts` (156 LOC) — settings schema
- The TS adapter delegates the runtime to the closed-source npm package
  `@anthropic-ai/sandbox-runtime`. coco-rs implements both halves
  in-tree because there is no Rust equivalent of that package.

## Architecture

```
                 settings.json + permission rules
                          │
                          ▼
                  coco_sandbox::adapter
                  ─────────────────────
                  Claude-specific glue (~900 LOC)
                  · convertToSandboxRuntimeConfig
                  · path-resolve helpers
                  · worktree detect, bare-repo scrub
                  · sandbox_unavailable_reason
                          │
                          │ produces SandboxConfig + EnforcementLevel
                          ▼
                  coco_sandbox::{state, platform, ...}
                  ────────────────────────────────────
                  Universal runtime (~1500 LOC)
                  · SandboxState  — Arc-shared, hot-reloadable
                  · SandboxPlatform trait
                      └─ macOS: Seatbelt SBPL (platform/macos.rs)
                      └─ Linux: bubblewrap + seccomp (platform/linux.rs)
                      └─ Windows: stubbed (platform/windows.rs)
                  · ProxyServer / BridgeManager — HTTP/SOCKS5 + UDS bridges
                  · ViolationStore — ring buffer (max 100)
                  · PermissionChecker — fail-closed path/network check
                  · SandboxApprovalBridge — interactive approval (SDK)
                  · glob_expansion — bounded-depth deny-read expander
```

## Key Types

### Adapter (`coco_sandbox::adapter`)

| Type | Purpose |
|---|---|
| `AdapterInputs<'a>` | Settings + permission rules + cwd context — pure inputs, no I/O |
| `AdapterOutput` | `EnforcementLevel` + (possibly mutated) `SandboxSettings` + `SandboxConfig` |
| `build_runtime_config(inputs) -> AdapterOutput` | Mirrors TS `convertToSandboxRuntimeConfig` |
| `resolve_permission_rule_path(pattern, root) -> PathBuf` | Permission-rule path conventions: `//x` → absolute, `/x` → settings-relative |
| `resolve_filesystem_path(path, root) -> PathBuf` | Settings-section path conventions: absolute stays absolute (#30067) |
| `bare_repo_scrub_paths(cwd, original) -> Vec<PathBuf>` | TS issue #29316 mitigation — paths to scrub post-command |
| `scrub_bare_repo_files(paths)` | Best-effort delete of planted bare-repo files |
| `detect_worktree_main_repo(cwd) -> Option<PathBuf>` | Resolves `<cwd>/.git` pointer file when in a worktree |
| `sandbox_unavailable_reason(...) -> Option<String>` | UX warning when user enabled sandbox but it can't run |

### Runtime (`coco_sandbox::{state, config, platform, …}`)

| Type | Purpose |
|---|---|
| `SandboxState` | Shared via `Arc`; hot-reloadable enforcement, network proxy state, violations ring |
| `SandboxState::try_wrap_command(command, bypass, &mut Command) -> Result<bool>` | One-shot helper — combines snapshot + platform wrap; called by shell/PowerShell |
| `SandboxState::command_snapshot(command, bypass) -> CommandSandboxSnapshot` | Per-command decision + cached config under a single `RwLock` read |
| `SandboxState::update_config(...)` | Hot-reload (called by `SettingsWatcher`) |
| `SandboxConfig` | Runtime restriction lists: writable roots, deny paths, `denied_read_globs`, `glob_scan_max_depth`, allow-network, proxy state |
| `SandboxSettings` | User-facing settings: `enabled`, `fail_if_unavailable`, `excluded_commands`, `filesystem.*`, `network.*`, `ignore_violations` |
| `glob_expansion::expand` | Bounded-depth glob expander for `denied_read_globs` (uses `globset` + `walkdir`); platforms call this at wrap time |
| `EnforcementLevel` | `Disabled` / `ReadOnly` / `WorkspaceWrite` / `Strict` — runtime behavior (resolved from `SandboxMode`) |
| `SandboxBypass` | `No` / `Requested` — non-bool param wrapper |
| `WritableRoot` | Path + `readonly_subpaths` — auto-detects `.git` pointer files |
| `SandboxPlatform` (trait) | `available()` + `wrap_command(config, command, session_tag, &mut Command)` |
| `PermissionChecker` | Fail-closed path/network check; optional `SandboxApprovalBridge` for interactive approval |
| `ViolationStore` | Ring buffer (max 100) + observer channel + ignore patterns |
| `ViolationMonitor` | macOS: `log stream` parser; Linux/Windows: passive |
| `ProxyServer` | HTTP CONNECT + SOCKS5 with `DomainFilter` |
| `BridgeManager` | UDS-based proxy bridge for Linux network namespaces |
| `SandboxApprovalBridge` (trait) | Async approval; `NoOpSandboxApprovalBridge` rejects everything |

`SandboxMode` lives in `coco-types` (canonical, user-facing). The crate
deliberately does NOT redefine it.

## Dependency Layer

L2: `coco-sandbox` — depends on `coco-types`, `coco-error`,
`coco-utils-string`. Consumed by `coco-shell`, `coco-tool-runtime`,
`coco-tools`, `app/cli`, `app/query`.

## Configuration & Bootstrap

The CLI session bootstrap (`app/cli/src/session_runtime.rs::build_sandbox_state`)
constructs `Arc<SandboxState>` once per session by:

1. Gating on `Feature::Sandbox` + non-`FullAccess` mode
2. Calling `check_enable_gates(&settings)` (platform + deps + allowlist)
3. On gate failure, calling `sandbox_unavailable_reason(...)` to surface a
   user-facing banner on stderr; if `sandbox.fail_if_unavailable` is set,
   the function returns `Err(...)` and coco exits before the REPL starts
   (TS parity with `entrypoints/sandboxTypes.ts:95`).
4. Collecting permission rules from the merged `Settings`
5. Calling `adapter::build_runtime_config(inputs)`
6. Constructing `SandboxState::new(...)` (or `external(...)` for
   `ExternalSandbox` mode)

The signature is `Result<Option<Arc<SandboxState>>>` — `Ok(None)` means
"degrade to unsandboxed" and `Err` means "exit with diagnostic".

The `Arc<SandboxState>` is threaded into:

- `QueryEngineConfig::sandbox_state`
- `ToolUseContext::sandbox_state`
- `coco_shell::ExecOptions::sandbox` (when Bash spawns a child)
- PowerShell tool (calls `state.try_wrap_command(...)` directly)

## Post-Command Hooks

After every shell-executor child exits (success / cancel / timeout / IO
error), `scrub_bare_repo_after_command(...)` calls
`bare_repo_scrub_paths(cwd, original_cwd)` to compute the post-command
deletion set, then `scrub_bare_repo_files(...)` removes any planted
`HEAD` / `objects` / `refs` / `hooks` / `config` artefacts. Mitigation
for [anthropics/claude-code#29316](https://github.com/anthropics/claude-code/issues/29316),
matching TS `cleanupAfterCommand()`.

## Deny-Read Globs

`SandboxSettings.filesystem.deny_read` entries that contain glob
metacharacters (`*`, `?`, `[`) are routed by the adapter into
`SandboxConfig.denied_read_globs`; literal paths land in
`denied_read_paths` as before. At platform-wrap time both
`platform/macos.rs` and `platform/linux.rs` call
`glob_expansion::expand(roots, globs, glob_scan_max_depth)` to enumerate
matches under the writable roots and merge them into the deny list:

- macOS — append to the Seatbelt `(deny file-read* (subpath ...))` block
- Linux — emit `--ro-bind-try /dev/null <path>` per match, same trick
  used for symlink-attack mitigation

`mandatory_deny_search_depth` (default 3) caps the walk so a poorly-scoped
glob (`**/*.env`) cannot stall the bootstrap.

## Hot Reload

`SandboxState::update_config(enforcement, settings, config)` replaces
the mutable inner config under the `RwLock`. The `SettingsWatcher`
pump (in `coco-config`) re-runs the adapter on settings change and
calls this. Proxy servers and the violation store are preserved across
reloads.

## Failure Policy

Fail-closed everywhere. Single documented opt-out:
`dangerouslyDisableSandbox: true` parameter (gated by
`SandboxSettings.allow_unsandboxed_commands`). Bootstrap failures
(missing `bwrap`, unsupported platform) fall back to unsandboxed
**only when** `sandbox.fail_if_unavailable` is `false` (default); set
it to `true` for hard startup failure.

## What's NOT here

- Process hardening (setuid drops, prctl flags) — that's `coco-process-hardening`
- TUI views for violations — bound to `ViolationStore` via observer channel; UI lives in `coco-tui`

## Cross-References

- User-facing doc: [docs/sandbox.md](../sandbox.md)
- Bypass model: [docs/coco-rs/bypass-permissions.md](bypass-permissions.md)
- Permission system: [docs/coco-rs/crate-coco-permissions.md](crate-coco-permissions.md)
