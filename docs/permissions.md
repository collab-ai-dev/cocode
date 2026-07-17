# Permissions

Every tool call the model makes — a shell command, a file write, an MCP call —
passes through a permission decision before it runs. This page explains how that
decision is made: the modes you can put a session in, the rule syntax you write
in `settings.json`, the LLM classifier behind auto mode, and the bypass
killswitch.

Permissions decide **whether** a tool call runs. If you also want to constrain
**what a command can touch** once it is allowed to run, that is the
[sandbox](sandbox.md) — a separate, independently-gated layer.

## How a decision is made

For each tool call, cocode walks a fixed pipeline and returns the first decision
that matches. Ordered from highest to lowest precedence:

1. **Deny rules.** A matching deny rule wins immediately, no matter which
   settings layer it came from. There is no "allow beats deny" case.
2. **The tool's own check.** Each tool can inspect the concrete input and return
   a verdict the generic rule engine cannot express — Bash parses subcommands,
   Read and Write resolve and vet paths, WebFetch checks the URL. A tool may
   allow, deny, ask, or stay silent and defer to the rules.
3. **Allow rules**, tried in source-priority order (see below). A compound shell
   command (`a && b`) is also allowed when *every* one of its subcommands is
   individually covered by an allow rule.
4. **Ask rules.** A tool-wide ask (`"Bash"` in the `ask` list) forces a prompt;
   a content-scoped ask (`"Bash(git push:*)"`) forces a prompt for just those
   commands.
5. **Path-safety checks** for file-modifying tools. Dangerous targets — path
   traversal, shell expansion in a path, sensitive system files — force a prompt
   even in permissive modes.
6. **MCP server-level rules**, so `mcp__github` can allow or ask for every tool
   that server exposes.
7. **Mode fallthrough.** Nothing matched, so the current permission mode decides.

Because the tool's own check runs at step 2, some content-scoped rules never
reach the central engine at all. Rules for `Bash`, `PowerShell`, `Write`,
`Edit`, `NotebookEdit`, and `ApplyPatch` are matched centrally. Content rules for
`Read`, `Grep`, `Glob`, `WebFetch`, and `Agent` are deliberately handed to the
tool, which knows how to scope them (a `Read(/secret/**)` deny must be matched
against the *resolved* path, not the raw string). The outcome is the same; only
the place the match happens differs.

If a `PreToolUse` hook returns a permission opinion, it is honored and this
pipeline is skipped entirely.

## Permission modes

A session is always in exactly one mode. Modes only decide what happens at
step 7 — an explicit deny rule still denies in every mode except
`bypassPermissions`.

| Mode | Wire value | What it does when no rule matched |
|---|---|---|
| Default | `default` | Read-only tools are allowed; everything else prompts. |
| Accept edits | `acceptEdits` | Read-only tools **and** file writes/edits are allowed; shell and everything else still prompts. |
| Plan | `plan` | Read-only tools are allowed; everything else prompts. |
| Bypass permissions | `bypassPermissions` | Everything is allowed. No prompts, no rules. |
| Don't ask | `dontAsk` | Anything that would have prompted is **denied** instead. |
| Auto | `auto` | Read-only tools are allowed; everything else goes to the classifier. |
| Bubble | `bubble` | The decision is escalated to the parent agent. Internal; used for subagents. |

A few of these deserve more than a table row.

**Default** is the baseline and the one you will spend most of your time in. The
status bar renders it as `⏯ manual mode on · shift+tab to cycle`. "Manual" is the
honest label: you are the one approving each side-effecting call.

**Plan mode** is not enforced by the permission layer alone — it is mostly a
prompt-level contract that the agent researches before it writes. What the
permission layer adds is that non-read-only tools prompt. One exception: if the
session was started with bypass unlocked, plan mode auto-allows rather than
prompting, on the theory that you already opted out of prompts.

**Don't ask** is easy to misread. It is *not* a quieter bypass. It converts every
would-be prompt into a denial, including prompts raised by ask rules, path-safety
checks, and MCP rules. In practice it means "run exactly what my allow rules
permit and nothing else" — useful for unattended runs where you want a hard
failure instead of a hang. (Note that the in-app description text for this mode
currently says the opposite; the evaluator denies.)

**Bubble** is not something you select. A subagent set to bubble escalates its
decisions to whoever spawned it.

Subagents inherit the parent's mode. If the parent is in a trust mode —
`bypassPermissions`, `acceptEdits`, or `auto` — the child always inherits it and
its own declared mode is ignored, so a nested agent cannot quietly downgrade and
start re-prompting.

### Cycling modes with Shift+Tab

In the TUI, `Shift+Tab` cycles forward through the available modes:

```
default → acceptEdits → [plan] → [bypassPermissions] → [auto] → default
```

The bracketed steps are skipped when unavailable. Plan is skipped when the
`plan_mode` feature is off. Bypass appears only if this session unlocked it (see
below). Auto appears unless `auto_mode.disabled` is set in settings. `dontAsk`
and `bubble` are not in the cycle — reach `dontAsk` via `--permission-mode
dontAsk` or `permissions.default_mode`.

The mode a session starts in is resolved in this order: `--dangerously-skip-permissions`,
then `--permission-mode <mode>`, then `permissions.default_mode` from settings,
then `default`.

```bash
coco --permission-mode acceptEdits
coco --permission-mode plan -p "audit the auth module and propose a fix"
```

## Where rules live

Rules are string arrays under a `permissions` key in any `settings.json`:

```jsonc
// ~/.cocode/settings.json
{
  "permissions": {
    "allow": [
      "Bash(git status:*)",
      "Bash(git diff:*)",
      "Bash(cargo test *)",
      "Read"
    ],
    "deny": [
      "Read(//etc/**)",
      "Bash(curl:*)"
    ],
    "ask": [
      "Bash(git push:*)"
    ],
    "additional_directories": ["../shared-lib"],
    "default_mode": "acceptEdits"
  }
}
```

The same block is valid in the project file (`.cocode/settings.json`, checked in),
the local file (`.cocode/settings.local.json`, personal, usually gitignored), a
file passed with `--settings`, and the managed/enterprise policy file. Rules are
not overridden layer by layer — they are **merged**, and every rule from every
layer participates. Settings files are JSONC, so comments are allowed.

With one deliberate exception: **`default_mode` and `disable_bypass_mode` are
ignored in the project file.** Those two decide whether a session can start with
every tool call auto-approved, and `.cocode/settings.json` arrives with the
repository — it is written by whoever published the code you just cloned, not by
you. Only the user, local, flag, and policy layers can set them. Put them in
`~/.cocode/settings.json` (yours) or `.cocode/settings.local.json` (yours, and
gitignored), not in the checked-in file.

When two rules disagree, the more specific source wins:

```
session > command > cliArg > flag settings > local > project > user > policy
```

That ordering applies to allow rules. Deny always wins outright, regardless of
which layer wrote it.

Other keys in the `permissions` block:

| Key | Type | Meaning |
|---|---|---|
| `allow` / `deny` / `ask` | string[] | Rule lists, as above. |
| `default_mode` | string | Mode the session starts in (`default`, `plan`, `acceptEdits`, `auto`, `dontAsk`, `bypassPermissions`). Ignored in the project file. |
| `disable_bypass_mode` | bool | Killswitch: makes `bypassPermissions` unreachable. Ignored in the project file; any trusted layer setting it `true` wins, so a repo cannot switch it back off. |
| `additional_directories` | string[] | Extra roots the agent may read and write outside the project. |
| `allow_managed_permission_rules_only` | bool | When set in managed policy, only policy-sourced rules are honored; user/project/local/CLI rules are dropped entirely. |
| `permission_explainer_enabled` | bool | Default on. Enables the on-demand LLM risk explanation on permission prompts (`Ctrl+E`). |

### The `/permissions` command

In the TUI, `/permissions` opens an editor over your rules — list, add, delete,
with a confirm step. Adding a rule there persists it to the settings file you
choose. `/permissions allow <tool>` and `/permissions deny <tool>` add a rule for
the current session only; `/permissions reset` clears the session-scoped rules
and leaves your files untouched.

Outside the TUI (SDK, print mode), `/permissions` is read-only and will tell you
so — edit `settings.json` directly instead.

## Rule syntax

A rule is either a bare tool name or a tool name with a parenthesized scope:

```
Read                        # every Read call
Bash(git status:*)          # only git status and its subcommands
mcp__github__create_issue   # one MCP tool
mcp__github                 # every tool on the github MCP server
mcp__github__*              # same, via prefix wildcard
*                           # every tool
```

`Tool()` and `Tool(*)` are both treated as the bare tool name. Literal
parentheses inside a scope are escaped with a backslash: `Bash(python -c
"print\(1\)")`.

### Shell scopes

For `Bash` and `PowerShell`, the scope is a command pattern in one of three
forms:

| Form | Example | Matches |
|---|---|---|
| Exact | `Bash(git status)` | only the exact string `git status` |
| Prefix | `Bash(git status:*)` | any command starting with `git status` |
| Wildcard | `Bash(cargo * --release)` | glob-style; `*` matches any run of characters |

The prefix form is what the approval dialog suggests when you pick "always
allow", and it is a plain string prefix — `Bash(git:*)` matches `git status`, but
it also matches anything else beginning with those characters. Prefer the most
specific prefix you are comfortable with.

The wildcard form has one convenience: a trailing ` *` is optional, so
`Bash(git *)` matches bare `git` as well as `git log`. `\*` matches a literal
asterisk. PowerShell patterns match case-insensitively; Bash patterns are
case-sensitive.

Deny and ask rules match more aggressively than allow rules, on purpose. Before
matching, they strip leading environment assignments and safe wrappers and split
compound commands, so `FOO=1 timeout 5 curl evil.com` and `echo hi && curl
evil.com` both still hit a `Bash(curl:*)` deny. Allow rules refuse to match a
compound at all, so `Bash(cd:*)` cannot be chained into auto-allowing
`cd /tmp && curl evil.com`.

### File scopes

`Edit(...)`, `Read(...)`, and friends take a path pattern with gitignore-like
glob semantics. How the pattern is anchored depends on its first characters:

| Pattern | Anchored at |
|---|---|
| `src/**` | the current working directory |
| `/src/**` | the **settings root** for the rule's source — the project root for project/local/CLI rules, your config home for user-settings rules |
| `//etc/**` | the filesystem root (`/etc/**`) |
| `~/.aws/**` | your home directory |

The single-leading-slash form is the one that surprises people: in a project
`settings.json`, `Edit(/src/**)` means "`src/**` under this project", not
"`/src/**` on this machine". Use `//` when you mean an absolute path.

An `Edit(path)` allow rule also grants reads of that path.

### Tool exposure vs. permission rules

`--allowed-tools` and `--disallowed-tools` are a different mechanism from the
rules above. They take plain tool ids and control which tools are **exposed to
the model at all**, rather than what happens when one is called:

```bash
coco -p "summarize the README" --allowed-tools Read Glob Grep
```

Two caveats worth knowing. They accept tool ids only — a scoped string like
`Bash(git status:*)` is not parsed as a rule here and simply matches nothing.
And they are applied in print mode (`-p`) and are not wired into the interactive
TUI session, so launching the TUI with `--allowed-tools` will not narrow the
tool set.

### Additional directories

`--add-dir` (repeatable) and `permissions.additional_directories` widen the set of
roots the agent treats as its workspace. Paths inside them are read- and
write-allowed without a prompt in the modes that permit edits, and they become
writable roots in the sandbox. Relative paths resolve against the working
directory.

```bash
coco --add-dir ../shared-lib --add-dir /srv/fixtures
```

`/add-dir <path>` does the same thing mid-session.

## Auto mode and the classifier

Auto mode (`auto`) replaces "prompt the user" with "ask a model". It is a real
LLM call, not a heuristic table, and it is worth understanding before you trust
it.

When a call in auto mode reaches the point where Default mode would have
prompted, cocode runs a series of cheap checks first:

- A **safe-tool allowlist** (Read, Grep, Glob, Lsp, task/todo bookkeeping,
  plan-mode tools, and similar) short-circuits to allow without any model call.
- **Read-only Bash commands** are allowed by shell analysis.
- **File writes** get a path-safety scan. Some blocks are *immune* — traversal,
  shell expansion in the path, dangerous system targets — and are never
  classifier-approvable. A write that passes every check and lands inside the
  working directory or an additional directory is allowed outright.
- A **preapproved WebFetch URL** is allowed.

Anything left over goes to the classifier: a two-stage XML prompt. Stage 1 is a
fast, small-budget pass that answers `<block>yes</block>` or `<block>no</block>`.
A `yes` — or an unparseable answer — escalates to stage 2, a larger pass with
room to think. If stage 2 is also unparseable, the action is blocked. Both stages
share a system prompt so the provider's prompt cache can absorb most of the cost.

The classifier is only shown user-authored text and the assistant's tool calls.
Assistant prose and tool *results* are deliberately excluded, because both are
attacker-influenceable and must not be able to talk the security classifier into
approving something.

When the classifier cannot answer:

- **Transcript too long** (a deterministic context overrun that a retry cannot
  fix) falls back to a manual prompt, or aborts the turn when the session cannot
  prompt.
- **Unavailable** (a transient transport or capacity failure) **fails closed** —
  denied, even interactively. Set `auto_mode.classifier_unavailable_fail_open` to
  `true` to get a manual prompt instead; a headless session still denies, since
  there is no prompt to show.

Relevant settings:

```jsonc
// ~/.cocode/settings.json
{
  "auto_mode": {
    "disabled": false,                          // removes auto from the Shift+Tab cycle
    "classifier_mode": "both",                  // "both" | "fast" | "thinking"
    "classifier_unavailable_fail_open": false,  // true = prompt instead of deny on outage
    "classify_all_shell": false,                // ignore Bash/PowerShell allow rules in auto mode
    "allow": [],
    "soft_deny": [],
    "environment": []
  }
}
```

`classify_all_shell` is the paranoid switch: it suspends your shell allow rules
while auto mode is active so every shell call is classified rather than
rubber-stamped by a broad `Bash(git:*)`. It is honored from user, local, flag, or
policy settings — project settings cannot turn it on, and cannot turn it off
either.

## Bypass and the killswitch

`bypassPermissions` allows every tool call. No rules, no prompts, no path safety.
It exists for throwaway containers and CI sandboxes. On a machine with your
credentials and your source tree, a prompt-injected agent in bypass mode can do
anything you can do.

Two flags, with a real difference:

| Flag | Effect |
|---|---|
| `--dangerously-skip-permissions` | Starts the session **in** bypass **and** unlocks it as a Shift+Tab target. |
| `--allow-dangerously-skip-permissions` | Unlocks bypass **without** entering it. You start in your normal mode and can cycle in later. |

The second is the one to reach for if you want the option available for a
specific step without running the whole session unguarded.

There is one hard refusal: if the process is running as root or under `sudo` and
is not in a sandbox, requesting bypass is a startup error rather than a warning.
Set `IS_SANDBOX=1` (or run under bubblewrap) if you genuinely mean it.

### The killswitch

Bypass can be forced off, in which case any attempt to enter it — flag, slash
command, SDK control — is refused and the session falls through to the next
candidate mode with a printed reason. Two ways to engage it:

```bash
# Operator override: CI, shared workstations, security-sensitive runs.
COCO_PERMISSIONS_DISABLE_BYPASS=1 coco
```

```jsonc
// settings.json — for a fleet, put this in the managed policy file
{
  "permissions": {
    "disable_bypass_mode": true
  }
}
```

Both keys are read only from layers you control — user, local, flag, and policy.
A checked-in `.cocode/settings.json` cannot set `default_mode` to
`bypassPermissions` to start your session unguarded, and cannot set
`disable_bypass_mode` to `false` to undo a killswitch you engaged. The killswitch
is an OR across those trusted layers: once any of them turns it on, no
lower-precedence layer turns it off.

This is a trust boundary, not a precedence rule. A project file is written by
whoever published the repository you cloned; letting it decide whether the
permission system runs would defeat the point of having one.

## Denial tracking and retries

A model that keeps getting denied tends to keep trying. cocode tracks denials per
session and breaks the loop: after **3 consecutive** denials, or **20 total**,
auto mode stops classifying and hands the next decision to you. Hitting the total
cap resets both counters so the session continues past one review prompt instead
of denying forever. Any allowed action clears the consecutive streak, and a
compaction clears the counters entirely (the context that drove them is gone).

In a headless session, where there is no prompt to fall back to, hitting either
limit aborts the turn instead.

### Reviewing what the classifier blocked

Each auto-mode classifier denial raises a toast and is recorded in a **recently
denied** list — the last 20, visible as a tab in the `/permissions` editor. Each
row can be marked in two ways:

- **Approve** records your consent in the transcript ("Permission granted for
  `<command>`. You may now retry…") so the model knows to try again on its next
  turn.
- **Retry** does the same *and* immediately starts a turn, so the model retries
  without waiting for you to type anything.

Neither action writes a permission rule. If you want the call to stop being
classified at all, add an allow rule from the editor's rules tab or with
`/permissions allow <rule>`.

## Relationship to the sandbox

The two layers answer different questions and neither replaces the other:

- **Permissions** decide *whether* a tool call runs at all. They are on by
  default, cross-platform, and cover every tool.
- **The [sandbox](sandbox.md)** constrains *what a shell command can touch* once
  it has been allowed to run — filesystem and network, enforced by the OS. It is
  off by default and covers shell commands only.

They do talk to each other. The sandbox reads your permission rules when it
builds its policy: `Edit(path)` allows become writable roots, `Read(path)` denies
become unreadable paths, and `WebFetch(domain:host)` rules become network
allow/deny entries. In the other direction, when a Bash command is known to be
sandboxed and `sandbox.auto_allow_bash_if_sandboxed` is on (the default), a
tool-wide `Bash` ask rule is skipped — the sandbox is doing the containment, so
the prompt is noise. Command-specific ask rules are still honored.

Running with the sandbox on is the sane way to make `acceptEdits` or `auto`
comfortable. Running with bypass *and* no sandbox leaves nothing between the
model and your machine.
