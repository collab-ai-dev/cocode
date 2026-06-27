# coco-sandbox

Sandbox runtime + adapter. Two-module split: adapter (policy → runtime config) over
sandbox-runtime. Rust ships both halves in-tree.

See [crate-coco-sandbox.md](../../../docs/coco-rs/crate-coco-sandbox.md)
for the full architecture, including how `app/cli/session_runtime.rs`
bootstraps the state.

## Layout

| Module | Role |
|---|---|
| `adapter` | Claude policy → runtime config (path resolvers, permission-rule extraction, bare-repo scrub, worktree detect, `sandbox_unavailable_reason`) |
| `config` | `SandboxConfig`, `SandboxSettings`, `EnforcementLevel`, `WritableRoot`, `FilesystemConfig`, `NetworkConfig`, `IgnoreViolationsConfig`, `SandboxBypass` |
| `state` | `SandboxState` (Arc-shared, hot-reloadable) + `CommandSandboxSnapshot` + `try_wrap_command` |
| `bootstrap` | 4-gate enable check (settings → platform → enabled-list → deps) |
| `bridge` | `SandboxApprovalBridge` async trait + `NoOpSandboxApprovalBridge` |
| `checker` | `PermissionChecker` — fail-closed path / network check |
| `platform` | OS-specific `SandboxPlatform` impls (macos/linux/windows) |
| `monitor` | macOS `log stream` parser; Linux/Windows passive |
| `seccomp` | Linux: in-process BPF filter (Restricted / ProxyRouted modes) |
| `violation` | `ViolationStore` ring buffer + observer channel |
| `proxy` | HTTP CONNECT + SOCKS5 + UDS bridge for netns |
| `deps` | Platform-specific binary detection (bwrap, sandbox-exec, socat) |

## Key Invariants

- `SandboxMode` is owned by `coco-types`. This crate **never** redefines it.
- `SandboxState` is the only object the rest of the workspace consumes.
  Don't expose `SandboxConfig` / `EnforcementLevel` to tool code; thread
  `Arc<SandboxState>` instead.
- Adapter functions are pure (no I/O except `Path::exists` for
  bare-repo scrub). The CLI bootstrap does I/O up front and passes the
  results into `AdapterInputs`.
- Bootstrap is fail-closed: when gates fail, `build_sandbox_state` returns
  `None` and commands run unsandboxed. The `fail_if_unavailable` setting
  upgrades that to a hard error (not yet wired into the CLI banner).

## Tests

`tests/enforcement.rs` — integration tests (Linux only, gated on
`bwrap` availability). `*.test.rs` companion files for unit tests.
