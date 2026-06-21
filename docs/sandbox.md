# Sandbox

coco-rs ships an optional OS-level sandbox that wraps shell and PowerShell
commands with platform enforcement (Seatbelt on macOS, bubblewrap +
seccomp on Linux). It's **disabled by default** — enable it explicitly
when you want filesystem and network restrictions applied to commands the
agent runs.

## Quick start

```jsonc
// ~/.coco/settings.json
{
  "features": { "sandbox": true },
  "sandbox": {
    "mode": "workspace_write",
    "excluded_commands": ["git", "npm"],
    "allow_network": false
  }
}
```

Or via env: `COCO_SANDBOX_MODE=workspace_write` (and
`COCO_FEATURE_SANDBOX=true`).

## Modes

| Mode | What gets restricted |
|---|---|
| `read_only` | Reads allowed everywhere; writes blocked outside writable roots |
| `workspace_write` (recommended) | Reads allowed; writes only to CWD + `additional_directories` + permission-rule allow-paths |
| `full_access` | No platform wrapping (sandbox effectively off) |
| `external_sandbox` | Platform wrapping skipped — assume the agent is already inside Docker / a container; proxy filtering and violation tracking still apply |

## How permission rules feed the sandbox

The sandbox reads your `permissions` rules and translates them into
runtime restrictions:

| Rule | Effect |
|---|---|
| `Edit(/path)` (allow) | Adds `path` to writable roots |
| `Edit(/path)` (deny) | Adds `path` to deny-write list |
| `Read(/path)` (deny) | Adds `path` to deny-read list |
| `WebFetch(domain:HOST)` (allow) | Adds `HOST` to network allow-list |
| `WebFetch(domain:HOST)` (deny) | Adds `HOST` to network deny-list |

Path conventions match Claude Code's:

- `/path` in a permission rule is **settings-relative** (resolved
  against the project's settings root)
- `//path` in a permission rule is **filesystem-absolute** (`//etc/hosts` → `/etc/hosts`)
- `~/path` expands to your home directory
- Paths in `sandbox.filesystem.*` settings sections use **standard
  semantics** (absolute stays absolute) — see #30067 in the TS repo for
  the rationale

## Always-denied paths

Independent of your rules, the sandbox denies writes to:

- `~/.claude/settings.json` and `<project>/.claude/settings.json`
  (so the agent can't disable its own permissions)
- `~/.claude/skills` and `<project>/.claude/skills`
  (auto-loaded skills have command-level privilege; protect them like commands)
- Bare-repo files that didn't exist at session start (`HEAD`, `objects`,
  `refs`, `hooks`, `config`) — mitigates a CVE-grade attack where
  planted git metadata escapes the sandbox; see TS issue #29316.
  Any of these planted by a sandboxed command are **also** scrubbed
  post-execution (matching TS `cleanupAfterCommand()`), so subsequent
  unsandboxed `git` invocations don't pick them up.

## Glob-pattern deny-read

`sandbox.filesystem.deny_read` entries containing `*`, `?`, or `[` are
treated as glob patterns. At sandbox-bootstrap time the patterns are
expanded against your writable roots (bounded by
`sandbox.mandatory_deny_search_depth`, default `3`) and the matching
files are added to the platform deny list:

```jsonc
{
  "sandbox": {
    "filesystem": {
      "deny_read": [
        "**/*.env",
        "secrets/**",
        "/abs/literal/path"   // literal paths still work
      ]
    },
    "mandatory_deny_search_depth": 5  // walk up to 5 dirs deep
  }
}
```

Globs that match nothing are silently dropped. Patterns that need
deeper expansion than the depth cap should be replaced with explicit
absolute paths.

## Bypassing for one command

Bash and PowerShell tools accept `dangerouslyDisableSandbox: true`. This
is honored only when `sandbox.allow_unsandboxed_commands` is `true` in
settings (the default). Set it to `false` in managed-settings policy to
prevent any per-command bypass.

`sandbox.excluded_commands` whitelists patterns by command prefix —
matches use the same fixed-point variant expansion as Claude Code
(`FOO=bar /usr/bin/git status` matches `git`).

## Platform support

| Platform | Backend | Notes |
|---|---|---|
| macOS | Seatbelt (`sandbox-exec`) | Requires `/usr/bin/sandbox-exec` (system) |
| Linux (native or WSL2) | bubblewrap + seccomp | Requires `bwrap`; `socat` recommended for network bridging |
| Windows | Restricted token + ACL | **Not yet integrated** — the bootstrap layer detects the platform but the inner-stage helper is stubbed |
| WSL1 | unsupported | sandbox refuses to start |

If you set `sandbox.enabled = true` but the platform/dependencies aren't
available, coco prints one warning at startup and runs commands
unsandboxed. Set `sandbox.fail_if_unavailable = true` to make this a
hard startup error instead.

## Violations

Sandbox denials don't prompt — they're recorded in a per-session ring
buffer (max 100). On macOS, denials surface in real time via
`log stream`; on Linux they arrive when the syscall returns `EPERM`.

The `/sandbox` command (TUI) shows recent violations and
configuration. SDK clients receive a `SandboxStateChanged` notification
when sandbox state hot-reloads.

## See also

- Crate plan: [docs/coco-rs/crate-coco-sandbox.md](coco-rs/crate-coco-sandbox.md)
- Architecture overview: [docs/coco-rs/CLAUDE.md](coco-rs/CLAUDE.md)
- Bypass model: [docs/coco-rs/bypass-permissions.md](coco-rs/bypass-permissions.md)
