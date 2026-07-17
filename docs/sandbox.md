# Sandbox

cocode can wrap the shell commands it runs in an OS-level sandbox — Seatbelt on
macOS, bubblewrap plus seccomp on Linux — so a command can only touch the files
and hosts you allow. It is **off by default**. This page covers turning it on,
choosing a posture, and what each configuration key actually does.

The sandbox is the second half of a pair. [Permissions](permissions.md) decide
*whether* a tool call runs at all; the sandbox constrains *what a command can
touch* once it has been allowed to run. Neither replaces the other, and the
sandbox only covers shell commands (`Bash`, `PowerShell`) — not the file tools,
not MCP.

## Quick start

```jsonc
// ~/.cocode/settings.json
{
  "features": { "sandbox": true },
  "sandbox": {
    "mode": "workspace_write",
    "excluded_commands": ["git", "npm"],
    "allow_network": false
  }
}
```

Or from the environment, which is handy for a single run:

```bash
COCO_FEATURE_SANDBOX=1 COCO_SANDBOX_MODE=workspace_write coco
```

The switch is the **feature gate** — `features.sandbox`, default `false`. There
is also a `sandbox.enabled` key in the schema, but it is not the gate: once the
feature is on, the runtime sets it internally regardless of what you wrote. Set
`features.sandbox`.

## Modes

`sandbox.mode` selects the posture applied once the gate is on. It defaults to
`read_only`.

| Mode | What it does |
|---|---|
| `read_only` (default) | Reads are allowed broadly; all writes are blocked. |
| `workspace_write` | Reads are allowed; writes are limited to the working directory, `--add-dir` roots, and paths granted by permission rules or `sandbox.filesystem.allow_write`. |
| `full_access` | No sandbox at all. The runtime is never constructed, so this is equivalent to leaving the feature off. |
| `external_sandbox` | Platform wrapping is skipped because you are already inside a container. Write-scope checks, domain filtering, and violation tracking still apply. |

`workspace_write` is the mode most people want: the agent can build and edit
inside the project, and nothing else on the machine is writable.

Note that `full_access` and `external_sandbox` are not "weaker sandboxes" — the
first turns enforcement off entirely, and the second delegates it to Docker (or
whatever you are running inside) while keeping cocode's own bookkeeping.

## Where writes are allowed

In `workspace_write`, the writable set is assembled from several sources:

- The current working directory, plus the sandbox's own temp directory.
- Every `--add-dir` / `permissions.additional_directories` root.
- `Edit(path)` **allow** rules from your permission settings.
- `sandbox.filesystem.allow_write` paths.
- The main repository, when the working directory is a git worktree.

Inside any writable root, three subpaths stay read-only regardless: `.git`,
`.cocode`, and `.agents`. In a worktree or submodule, where `.git` is a pointer
file rather than a directory, the real gitdir is resolved and protected too.

Some paths are always denied write, so a sandboxed command cannot edit its way
out of the sandbox:

- Every settings file — `~/.cocode/settings.json`, the project
  `.cocode/settings.json` and `.cocode/settings.local.json`, the managed policy
  file and its `managed-settings.d` drop-in directory, and the global config
  file.
- `.cocode/agents` and `.cocode/skills` in the project — auto-loaded skills and
  agent definitions carry command-level privilege, so they are protected like
  commands.

Separately, planted bare-repo files (`HEAD`, `objects`, `refs`, `hooks`,
`config`) that did not exist before a sandboxed command ran are deleted after it
finishes, so a later unsandboxed `git` invocation cannot pick up metadata the
agent dropped in the working directory.

### How permission rules feed the sandbox

The sandbox reads your `permissions` block and folds it into the platform policy:

| Rule | Effect |
|---|---|
| `Edit(/path)` allow | adds `path` to the writable roots |
| `Edit(/path)` deny | adds `path` to the deny-write list |
| `Read(/path)` deny | adds `path` to the deny-read list |
| `WebFetch(domain:HOST)` allow | adds `HOST` to the network allow list |
| `WebFetch(domain:HOST)` deny | adds `HOST` to the network deny list |

Path anchoring in a permission rule follows the permission-rule convention: a
single leading `/` is **settings-relative** (rooted at the project for
project/local rules), `//` is filesystem-absolute (`//etc/hosts` → `/etc/hosts`),
and `~/` is your home directory. See [permissions](permissions.md#file-scopes).

Paths under `sandbox.filesystem.*` use ordinary semantics instead — absolute
stays absolute, `~/` expands, relative resolves against the settings root. (`//x`
is still accepted there as an escape hatch and means `/x`.) The two conventions
differ, which is a wart worth remembering when moving a path between blocks.

## Filesystem configuration

```jsonc
{
  "sandbox": {
    "filesystem": {
      "allow_write": ["/srv/scratch"],
      "deny_write": ["vendor"],
      "deny_read": ["**/*.env", "secrets/**", "/abs/literal/path"],
      "allow_read": ["/etc/ssl/certs"],
      "allow_git_config": false,
      "allow_managed_read_paths_only": false
    },
    "mandatory_deny_search_depth": 3
  }
}
```

`allow_read` re-permits reading inside a `deny_read` region and takes precedence
for matching paths. `allow_git_config` (default `false`) controls whether
`.git/config` and `~/.gitconfig` are writable.

`deny_read` entries containing `*`, `?`, or `[` are treated as glob patterns.
They are expanded against your writable roots at sandbox-bootstrap time, bounded
by `mandatory_deny_search_depth` (default `3`), and the matching files are added
to the platform deny list. Globs that match nothing are dropped silently; if a
pattern needs deeper expansion than the cap allows, use an explicit absolute path
instead.

`allow_managed_read_paths_only`, when set in managed policy, honors `allow_read`
only from the policy layer — user, project, local, and flag entries are ignored,
while `deny_read` from every layer is still respected.

### Hiding credentials

```jsonc
{
  "sandbox": {
    "credentials": {
      "files": [{ "path": "~/.aws/credentials", "mode": "deny" }],
      "env_vars": [{ "name": "GITHUB_TOKEN", "mode": "deny" }]
    }
  }
}
```

Listed files become unreadable inside the sandbox; listed environment variables
are removed from the command's environment before it launches. `deny` is the only
supported mode. Relative file paths resolve against the settings source that
declared them.

## Network

Enabling the sandbox isolates the network by default. There is no implicit
"allow everything" — with no domain configuration at all, the egress filter
denies by default.

```jsonc
{
  "sandbox": {
    "allow_network": false,
    "network": {
      "mode": "full",
      "allowed_domains": ["github.com", "*.crates.io"],
      "denied_domains": ["evil.example"],
      "block_non_public_ips": true,
      "allow_local_binding": false,
      "allow_unix_sockets": [],
      "allow_all_unix_sockets": false
    }
  }
}
```

`sandbox.allow_network` is the coarse switch: set it to `true` and network
isolation is bypassed entirely. Leave it `false` and egress is routed through a
local filtering proxy that enforces the domain lists. A `*.example.com` wildcard
matches subdomains but not the bare domain. If only `denied_domains` is set, the
filter runs as a blocklist; as soon as `allowed_domains` is non-empty, it is an
allowlist.

`network.mode` is `full` (default) or `limited`. In `limited`, only GET, HEAD,
and OPTIONS are permitted and CONNECT tunnels and SOCKS5 are blocked — the proxy
cannot inspect methods through a tunnel, so it refuses to open one.
`block_non_public_ips` rejects connections to loopback, RFC-1918, link-local,
CGNAT, and other reserved ranges (SSRF prevention).

On Linux the proxy needs `socat` to bridge out of the network namespace. Without
it, the posture stays fail-closed: the network is blocked for the session and a
warning is logged. Installing socat (`apt install socat`) enables per-domain
filtering.

## Excluding commands

Some commands do not survive sandboxing — anything that writes to a shared store
outside the workspace, for instance. `sandbox.excluded_commands` runs them
unwrapped:

```jsonc
{ "sandbox": { "excluded_commands": ["git", "npm:*", "npm run *"] } }
```

Three pattern forms:

| Pattern | Matches |
|---|---|
| `git` | `git` alone and `git <args>` (exact first word) |
| `npm:*` | `npm` and any subcommand |
| `npm run *` | trailing-wildcard glob |

Matching is done against normalized variants of the command, so leading
environment assignments, absolute paths, and safe wrappers are all peeled before
comparison: `FOO=bar /usr/bin/git status` matches `git`, and so does
`timeout 30 nice -n 5 git status`.

This is a convenience feature, not a security boundary — an excluded command runs
with your full privileges.

## Per-command bypass

The `Bash` and `PowerShell` tools accept a `dangerouslyDisableSandbox: true`
parameter. It is honored only while `sandbox.allow_unsandboxed_commands` is
`true` (the default). Set it to `false` — ideally in managed policy — to prevent
any per-command escape:

```jsonc
{ "sandbox": { "allow_unsandboxed_commands": false } }
```

The reverse direction is `sandbox.auto_allow_bash_if_sandboxed` (default
`true`): when a command is going to be sandboxed anyway, a tool-wide `Bash` ask
rule is skipped rather than prompting you for containment you already have.
Command-specific ask rules still prompt.

## Platform support

| Platform | Backend | Status |
|---|---|---|
| macOS | Seatbelt (`sandbox-exec`) | Enforced. Requires `/usr/bin/sandbox-exec` (ships with the OS). |
| Linux, incl. WSL2 | bubblewrap + in-process seccomp | Enforced. Requires `bwrap`; `socat` additionally needed for network filtering. |
| WSL1 | — | Refused. WSL1 lacks the namespace syscalls bubblewrap needs; use WSL2. |
| Windows | restricted token + ACL | **Not enforced.** The outer stage exists but no native backend ships, so commands run unwrapped. |
| Everything else | — | Unsupported. |

Be careful with the Windows row: the startup gates currently pass on Windows and
the platform then reports itself unavailable, so commands silently run
unsandboxed with no warning. Do not rely on the sandbox on Windows.

Before starting, cocode checks four gates in order: the feature is on, the
platform is supported, the platform is in `sandbox.enabled_platforms` (defaults
to `["macos", "linux", "windows"]`), and the required binaries are present. If a
gate fails, it prints one reason to stderr and runs commands unsandboxed:

```
[coco] sandbox unavailable: sandbox.enabled is set but dependencies are missing: bwrap
```

To make that a hard startup failure instead of a degraded session:

```jsonc
{ "sandbox": { "fail_if_unavailable": true } }
```

## Violations

A sandbox denial does not prompt. It is recorded in a per-session ring buffer
(the most recent 100), surfaced in the TUI as a toast (`Sandbox blocked 3
violations`), and shown to the model as a `<sandbox_violations>` block so it can
adjust rather than retrying blindly. A blocking modal per burst would interrupt
the turn repeatedly, so it is deliberately a passive surface.

On macOS, denials are read in real time from the system log; on Linux they arrive
as `EPERM` when the syscall returns.

`sandbox.ignore_violations` suppresses known-benign ones, keyed by command
pattern (or `"*"` for all):

```jsonc
{ "sandbox": { "ignore_violations": { "npm": ["file-write-data"] } } }
```

Configuration changes are hot-reloaded: editing `settings.json` mid-session
re-applies the policy in place, and SDK clients receive a `sandbox/stateChanged`
notification.

## The `/sandbox` command

`/sandbox` with no argument prints the available modes and exclusion
subcommands. With an argument it persists to **user** settings
(`~/.cocode/settings.json`):

```
/sandbox workspace_write        # set sandbox.mode
/sandbox exclusions             # list sandbox.excluded_commands
/sandbox exclude npm:*          # add an exclusion
/sandbox unexclude npm:*        # remove one
```

It accepts a few spellings per mode: `readonly` / `read-only` for `read_only`,
`workspace-write` or `strict` for `workspace_write`, `none` / `off` / `disable`
for `full_access`, and `external-sandbox` for `external_sandbox`.

`/sandbox` sets the mode only — it does not turn the feature on. If
`features.sandbox` is false, changing the mode has no effect.

## Environment variables

| Variable | Effect |
|---|---|
| `COCO_FEATURE_SANDBOX` | `1`/`0` — the feature gate, overriding `features.sandbox`. |
| `COCO_SANDBOX_MODE` | Overrides `sandbox.mode`. |
| `COCO_SANDBOX_EXCLUDED_COMMANDS` | Colon- or comma-separated exclusion patterns. |
| `COCO_SANDBOX_ALLOW_NETWORK` | Truthy value sets `allow_network = true`. |
| `COCO_SANDBOX_FAIL_IF_UNAVAILABLE` | Truthy value sets `fail_if_unavailable = true`. |

`COCO_SANDBOX_MODE` accepts `workspace_write` / `workspace-write` / `strict`,
`full_access` / `full-access` / `none`, and `external_sandbox` /
`external-sandbox`. Anything else — including a typo — resolves to `read_only`
without an error, so check the value if the posture is not what you expected.

The nested `filesystem` and `network` blocks have no environment path by design;
they come from `settings.json` only.

## Full settings reference

Every key under `sandbox`, with its default:

| Key | Default | Meaning |
|---|---|---|
| `mode` | `read_only` | Enforcement posture. |
| `allow_network` | `false` | Coarse switch to bypass network isolation. |
| `enabled` | `false` | Not the gate — see [Quick start](#quick-start). Use `features.sandbox`. |
| `fail_if_unavailable` | `false` | Hard-fail startup instead of degrading to unsandboxed. |
| `auto_allow_bash_if_sandboxed` | `true` | Skip a tool-wide Bash ask rule when the command will be sandboxed. |
| `allow_unsandboxed_commands` | `true` | Honor the per-command `dangerouslyDisableSandbox` parameter. |
| `enabled_platforms` | `["macos","linux","windows"]` | Platforms where the sandbox may start. |
| `excluded_commands` | `[]` | Command patterns that run unwrapped. |
| `filesystem` | `{}` | See [Filesystem configuration](#filesystem-configuration). |
| `network` | `{}` | See [Network](#network). |
| `credentials` | unset | Files / env vars hidden from sandboxed commands. |
| `ignore_violations` | `{}` | Per-command violation suppression. |
| `enable_weaker_nested_sandbox` | `false` | Relax isolation for Docker/WSL nesting. |
| `enable_weaker_network_isolation` | `false` | macOS: allow the trustd mach lookup Go binaries (`gh`, `gcloud`, `terraform`, `kubectl`) need for TLS verification. |
| `allow_pty` | `true` | macOS: allow sandboxed commands to allocate TTYs. |
| `mandatory_deny_search_depth` | `3` | Directory walk depth when expanding `deny_read` globs. |
| `ripgrep` | unset | Custom ripgrep binary and default args. |

## See also

- [Permissions](permissions.md) — the layer that decides whether a command runs
  at all, and the rule syntax the sandbox reads.
