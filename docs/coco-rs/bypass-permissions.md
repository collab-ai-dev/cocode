# Bypass Permissions: CLI Flags, Capability Gate, and Killswitch

**TS reference**: `utils/permissions/permissionSetup.ts`,
`utils/permissions/bypassPermissionsKillswitch.ts`, `setup.ts:395-442`,
`cli/print.ts:4588-4600`.

`BypassPermissions` is the most permissive mode coco-rs ships — it auto-allows
every tool call without prompting the user. The surface around it is
deliberately layered: a session has a **startup capability** (may it ever reach
bypass?) and a **live mode** (is it in bypass right now?). Those two concepts
are orthogonal. This doc describes both layers plus the policy killswitch that
lets operators disable bypass globally.

## CLI flags

Two flags gate bypass. Passing neither leaves the session unable to reach
`BypassPermissions` for its entire lifetime.

| Flag | Effect at startup | Unlocks bypass capability? |
|---|---|---|
| `--dangerously-skip-permissions` | Session starts **in** `BypassPermissions` | ✅ yes |
| `--allow-dangerously-skip-permissions` | Session starts in default (or `--permission-mode`) | ✅ yes |
| (neither) | Default | ❌ no |

The distinction matters when a user wants to start conservatively but retain
the option to cycle into bypass via Shift+Tab or the plan-exit overlay. Without
`--allow-dangerously-skip-permissions`, the only way to reach bypass is to
start there.

`--permission-mode bypassPermissions` is equivalent to
`--dangerously-skip-permissions` for capability derivation: whenever the
resolved startup mode is `BypassPermissions`, capability is unlocked.

## Startup resolution

When the CLI boots, `app/cli/src/main.rs::resolve_startup_permission_state`
runs four steps in order (`coco_permissions::resolve_initial_permission_mode`
implements steps 1-2 as a pure function with a full test matrix):

1. **Pick the initial mode.** Walk `[--dangerously-skip-permissions,
   --permission-mode <m>, settings.permissions.default_mode]` in order. The
   first candidate that isn't blocked by the killswitch wins. If every
   candidate is blocked or the list is empty, fall through to `Default`.
2. **Compute capability.** `bypass_permissions_available = (resolved_mode ==
   BypassPermissions || --allow-dangerously-skip-permissions) && !killswitch`.
3. **Run the sudo/sandbox guard.** If the session will start in bypass **or**
   unlocked it via the allow-flag, refuse to run as root/sudo unless a sandbox
   marker (`IS_SANDBOX=1`, `COCO_CODE_BUBBLEWRAP`, `CLAUDE_CODE_BUBBLEWRAP`)
   is truthy. Matches `setup.ts:395-442`.
4. **Surface the downgrade notification.** If the killswitch downgraded a
   requested bypass mode, the notification is printed to stderr (headless) or
   shown as a `Toast::warning` in the TUI.

## Killswitch

The killswitch is an emergency override that forcibly disables `BypassPermissions`
regardless of CLI flags or mode requests.

Two activation vectors:

### Environment variable

```sh
COCO_PERMISSIONS_DISABLE_BYPASS=1   # truthy: 1 | true | yes | on (case-insensitive)
```

Scope: process-wide. Intended for CI, shared workstations, or
security-sensitive runs. Takes precedence over every other source.

### Settings policy

```json
{
  "permissions": {
    "disable_bypass_mode": true
  }
}
```

Scope: managed-settings policy layer. Overrides user/project/local settings
when set at a higher precedence layer. TS equivalent:
`settings.permissions.disableBypassPermissionsMode === 'disable'` (the Rust
port simplifies the 3-state string to a boolean).

### Enforcement sites

- **Startup walk** (`resolve_initial_permission_mode`) — skips `BypassPermissions`
  when the killswitch is engaged, falls through to the next candidate.
- **Capability gate** (`compute_bypass_capability`) — always returns `false`
  when the killswitch is engaged.
- **TUI `UserCommand::SetPermissionMode` handler** (`tui_runner.rs`) — defense
  in depth: refuses to escalate into `BypassPermissions` when the session's
  startup capability is off.
- **SDK `control/setPermissionMode` handler**
  (`sdk_server/handlers/runtime.rs`) — returns `PERMISSION_DENIED` when a
  client attempts mid-session bypass without capability. TS parity:
  `cli/print.ts:4588-4600`.

## Downstream propagation

The static `bypass_permissions_available: bool` capability is threaded through
every subsystem that cares about it:

- `QueryEngineConfig.bypass_permissions_available` — engine-level.
- `ToolPermissionContext.bypass_available` — per-turn, read by permission
  evaluation (`core/permissions/src/evaluate.rs`).
- TUI `SessionState.bypass_permissions_available` — consumed by Shift+Tab
  cycle (`PermissionMode::next_in_cycle`) and the plan-exit overlay renderer.
- SDK `SdkServerState.bypass_permissions_available` — consulted by the
  mid-session bypass guard.
- `AgentQueryConfig.bypass_permissions_available` (in both
  `core/tool/src/agent_query.rs` and `app/state/src/swarm_runner_loop.rs`) —
  in-process subagents and swarm teammates inherit the parent's capability.

On mode changes, the SDK handler emits `permission/modeChanged` with the
current mode AND the static capability so attached clients stay in sync.

## Intentional skips vs. TS

Per [CLAUDE.md](../../CLAUDE.md) the coco-rs port deliberately skips:

- **GrowthBook feature flag** (`tengu_disable_bypass_permissions_mode`) —
  replaced by the settings-policy killswitch.
- **USER_TYPE=ant branches** — the Docker-offline-sandbox runtime check and
  ant-specific dangerous-classifier rules don't apply to external users.
- **CCR remote mode** (`CLAUDE_CODE_REMOTE`) — coco-rs doesn't ship the CCR
  web UI, so the CCR-only `bypassPermissions`-in-remote filter is a no-op here.
- **Ultraplan** (CCR-gated plan-refinement flow) — same reason.

## Quick reference: combinations

| User runs | Starts in | Capability | Can Shift+Tab to bypass? |
|---|---|---|---|
| `coco` | Default | ❌ | ❌ |
| `coco --dangerously-skip-permissions` | Bypass | ✅ | ✅ |
| `coco --allow-dangerously-skip-permissions` | Default | ✅ | ✅ |
| `coco --permission-mode bypassPermissions` | Bypass | ✅ | ✅ |
| `coco --permission-mode acceptEdits` | AcceptEdits | ❌ | ❌ |
| `coco --dangerously-skip-permissions` + killswitch engaged | Default (downgraded, toast shown) | ❌ | ❌ |
| `coco --dangerously-skip-permissions --permission-mode acceptEdits` + killswitch | AcceptEdits (walk fell through) | ❌ | ❌ |
