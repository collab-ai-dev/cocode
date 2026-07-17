# Subagents and agent teams

cocode can hand a piece of work to a separate agent that runs with its own
context window, its own tool permissions, and often its own model. This page
covers the built-in subagent types, how to define your own in markdown, how to
invoke them, and the experimental agent-teams subsystem that lets several named
agents work in parallel and message each other.

## Subagents

A subagent is a fresh agent instance spawned by the main agent to do one job and
report back. The main agent sends a prompt; the subagent runs its own turn loop
with its own message history; only the subagent's final text comes back into the
parent conversation.

The reason to reach for one is **context isolation**. When a question requires
reading twenty files to produce a three-line answer, doing it inline burns the
parent's context on file dumps it will never need again. Delegating means the
parent keeps the conclusion and drops the search. The same isolation makes
subagents a natural fit for parallel work: several spawned in one turn run
concurrently, each in its own context.

Subagents are always available — there is no feature gate on the `Agent` tool.

The nesting depth is capped at 5. A subagent can spawn its own subagents, but
once a parent is already at depth 5 the spawn is rejected and the agent is told
to do the work directly with its own tools.

### Built-in agent types

Agent type names are **case-sensitive**. `Explore` and `Plan` are PascalCase;
the rest are lowercase kebab-case. Passing `explore` where `Explore` is meant
will not match the built-in one-shot rules, so use the exact spelling below.

| Type | Available by default | What it is for |
|---|---|---|
| `general-purpose` | Yes | Researching complex questions, searching for code, and executing multi-step tasks. Has access to every tool. The fallback when `subagent_type` is omitted. |
| `Explore` | Yes | Fast read-only search agent for locating code. |
| `Plan` | Yes | Software architect agent for designing implementation plans. |
| `statusline-setup` | Yes | Configures your status line setting. Restricted to `Read` and `Edit`. |
| `coco-guide` | Yes, in interactive sessions | Answers questions about the CLI, the Agent SDK, and the Claude API. Read-only tools plus web access. |
| `verification` | **No — off by default** | Verifies that implementation work is correct before reporting completion. Runs builds, tests, and linters to produce a PASS/FAIL/PARTIAL verdict. |

The roster is resolved per session. Interactive CLI and TUI sessions get
`general-purpose`, `statusline-setup`, `Explore`, `Plan`, and `coco-guide`.
`verification` is defined in the catalog but is **not** included in the default
roster, so it will not appear in the agent list and cannot be spawned unless a
host build opts it in. Non-interactive embeddings can disable the built-in
roster entirely and inject their own definitions.

### Explore

`Explore` is a read-only search agent. It cannot edit files: `Agent`,
`Workflow`, `SendUserMessage`, `ExitPlanMode`, `Edit`, `Write`, and
`NotebookEdit` are all denied to it. It also runs with `omitClaudeMd` set, so it
does not load your CLAUDE.md files — it is meant to find things, not to absorb
project conventions.

When you delegate to it, **specify the search breadth in the prompt**. The three
recognized hints are `"quick"` for a single targeted lookup, `"medium"` for
moderate exploration, and `"very thorough"` to search across multiple locations
and naming conventions.

`Explore` is deliberately **not** for code review, design-doc auditing,
cross-file consistency checks, or open-ended analysis. It reads excerpts rather
than whole files and will miss content past its read window, so an audit
delegated to it will quietly return an incomplete answer. Use `general-purpose`
for work that must read files end to end.

### Plan

`Plan` is a software architect agent. It carries the same read-only tool denial
and the same `omitClaudeMd` behavior as `Explore`. Use it when you want an
implementation strategy: it returns step-by-step plans, identifies the critical
files, and weighs architectural trade-offs. It does not write the code.

### One-shot agents

`Explore` and `Plan` are one-shot: they run, they answer, and they cannot be
re-addressed afterwards. Every other agent type returns an agent id that can be
used to continue it with `SendMessage`.

## Custom agents

A custom agent is a markdown file with YAML frontmatter. The frontmatter
declares the agent's identity and constraints; the markdown body below the
frontmatter becomes the agent's system prompt.

### Where the files go

| Directory | Scope |
|---|---|
| `~/.cocode/agents/` | User-level — available in every project. |
| `<project>/.cocode/agents/` | Project-level — checked into the repo, available to anyone working in it. |

Both directories are walked **two levels deep**, so you can group agents in
subdirectories: `agents/review/security.md` and `agents/review/perf.md` both
load. Anything nested deeper than one subdirectory is ignored.

Only `.md` files are read. Files larger than 1 MiB are skipped silently (a debug
log records it) so a stray binary that happens to end in `.md` can never bloat
your prompt.

### Precedence

When two sources define an agent with the same `name`, the higher-priority
source wins. Priority runs lowest to highest:

```
built-in  <  plugin  <  user  <  project  <  CLI flag  <  policy
```

So a project-level `Explore.md` overrides the built-in `Explore`, and a
user-level agent is overridden by a project-level one of the same name. Run
`/agents paths` to see exactly which directories are being searched in your
current session.

Plugin-contributed agents are namespaced `<plugin>:<agent>` and are **stripped**
of `permissionMode`, `hooks`, and `mcpServers` at load time, so installing a
plugin cannot silently escalate an agent beyond install-time trust.

### Frontmatter reference

Both `snake_case` and `camelCase` spellings are accepted for every multi-word
key, and **unknown fields are silently ignored** — a typo will not fail the
load, it will just do nothing, so use `/agents validate` to check your work.

Two fields are required:

| Field | Notes |
|---|---|
| `name` | The agent's identity and the key used for lookup, precedence, and `subagent_type`. Must be non-empty. |
| `description` | What the agent is for. This is the text the main agent reads when deciding whether to delegate, so write it as selection criteria. Aliases: `whenToUse`, `when_to_use`. |

Everything else is optional:

| Field | Aliases | Accepted values |
|---|---|---|
| `model` | — | `provider/model_id`, or `inherit` to use the parent's model. Prefer `modelRole`. |
| `modelRole` | `model_role` | `main`, `fast`, `plan`, `explore`, `review`, `subagent`, `memory`, `hook_agent`. Case-insensitive. |
| `effort` | — | `off`, `auto`, `minimal`, `low`, `medium`, `high`, `xhigh`, or the alias `max`. A **numeric** value is rejected with a warning — this is a lookup key into the model's supported thinking levels, not a token budget. |
| `tools` | `allowed_tools` | YAML list or comma-separated string. `['*']` or omitting the key means every tool. `[]` means **no** tools. |
| `disallowedTools` | `disallowed_tools` | YAML list or comma-separated string of tool names to deny. |
| `permissionMode` | `permission_mode` | `default`, `plan`, `dontAsk`, `acceptEdits`, `bubble`, `bypassPermissions`, `auto`, `ask`, `deny`. |
| `isolation` | — | `none` (default) or `worktree`. `remote` parses but is rejected at spawn time — it is not supported. |
| `memory` | — | `user`, `project`, or `local`. |
| `maxTurns` | `max_turns` | A positive integer. Zero or negative is rejected with a warning. |
| `background` | — | `true` forces every spawn of this agent to run in the background. |
| `omitClaudeMd` | `omit_claude_md` | `true` skips CLAUDE.md loading. Appropriate for read-only search agents. |
| `useExactTools` | `use_exact_tools` | `true` keeps the tool-schema prefix byte-stable for prompt-cache hits. |
| `color` | — | `red`, `blue`, `green`, `yellow`, `purple`, `orange`, `pink`, `cyan`. |
| `identity` | — | Identity override string. |
| `initialPrompt` | `initial_prompt` | A prefix prepended to the agent's first user turn. This is **not** the system prompt — the markdown body is. |
| `criticalSystemReminder` | `critical_system_reminder`, `criticalSystemReminder_EXPERIMENTAL` | A short reminder re-injected on every user turn. |
| `skills` | — | Skill names to preload when the agent starts. |
| `mcpServers` | `mcp_servers` | A list of server names, inline `{name: {...}}` definitions, or a mix of both. |
| `hooks` | — | A nested hooks mapping, same shape as the `hooks` block in `settings.json`, scoped to this agent's lifecycle. |

An invalid value for a known field does **not** fail the load. The field is
dropped, the definition still loads, and the problem is reported as a warning by
`/agents validate`. Only a missing or empty `name` or `description` fails the
file outright.

### A complete example

Save this as `.cocode/agents/db-reviewer.md` in your project:

```markdown
---
name: db-reviewer
description: >
  Reviews database migrations and schema changes for correctness, index
  coverage, and backwards compatibility. Use before merging any change
  under migrations/ or any edit to a model definition.
modelRole: review
effort: high
tools:
  - Read
  - Grep
  - Glob
  - Bash
disallowedTools:
  - Write
  - Edit
permissionMode: dontAsk
maxTurns: 20
color: cyan
omitClaudeMd: false
---

You are a database migration reviewer. Given a set of changes, you:

1. Read every migration file in the change and the models it touches.
2. Check that each migration is reversible, or explicitly document why not.
3. Verify that every new foreign key and every column used in a WHERE or
   JOIN has index coverage.
4. Flag any change that drops or renames a column without a two-phase
   deploy path.

Report findings as a list ordered by severity. For each finding give the
file, the line, the specific risk, and a concrete fix. If a change is clean,
say so plainly rather than inventing concerns.
```

The agent becomes available on the next session. Verify it loaded with
`/agents show db-reviewer`.

### Prefer `modelRole` over `model`

Setting `model: openai/gpt-5` hardcodes a provider and a model id into a file
that may be shared across a team or checked into a repo where nobody else has
that provider configured. Setting `modelRole: review` instead routes the agent
through whatever model you have mapped to the `review` role in your own
settings, so the same agent file works for everyone.

Neither `model` nor `modelRole` is exposed to the main agent as a tool
parameter — both are operator-owned knobs. The model picks *which agent* to
delegate to; you decide what that agent runs on. See
[models and MoA](models-and-moa.md) for how roles resolve to concrete models.

If an agent declares neither, the model role is derived from the agent type:
`Explore` runs on the `explore` role, `Plan` on `plan`, `verification` on
`review`, and everything else — including all custom agents — on the `subagent`
role.

## Invoking an agent

### The Agent tool

The main agent delegates by calling the **`Agent`** tool. `Task` is accepted as
an alias for the same tool in permission rules — there is no separate `Task`
tool.

The two required parameters are `prompt` (the task) and `description` (a 3-5
word summary). `subagent_type` selects the agent; omitting it gives you
`general-purpose`.

Because a subagent's own tool calls are each permission-checked in the child,
the spawn itself is treated as read-only and is auto-approved rather than
prompting. Permission rules still bite: an `Agent(<type>)` deny rule removes
that type from the listing the model sees and rejects the spawn if it is
attempted anyway.

If the model names an agent type that does not exist, the spawn fails with the
list of available types and a pointer to the agent directories, rather than
silently degrading into an unconfigured agent.

### `@agent-` mentions

Typing `@agent-<type>` in your prompt — for example `@agent-db-reviewer look at
the migration I just wrote` — attaches a reminder telling the model you want
that agent invoked. It is a strong hint, not a hard dispatch: the model still
issues the `Agent` call itself and passes along the context it thinks is
relevant.

### The `/agents` command

Run `/agents` with no arguments in the TUI to open the agents overlay. The text
sub-commands work everywhere, including headless and SDK sessions:

| Command | What it does |
|---|---|
| `/agents list` | Every active agent with its source and model. |
| `/agents show <name>` | Full detail for one agent. `/agents <name>` is a shortcut. |
| `/agents paths` | The search directories in precedence order. |
| `/agents validate` | Load failures and warnings — run this after editing an agent file. |
| `/agents reload` | Re-scans the directories from disk. |

One caveat on `/agents reload`: it re-scans the directories and shows you what
is on disk now, but the **engine's live agent registry is loaded once at session
start**. Adding, removing, or editing a markdown agent takes effect in your next
session. `/agents list` and `/agents show` reflect current disk state, which is
what makes them useful for checking your edits before you restart.

Editing and deleting agents from the TUI overlay is not implemented — edit the
markdown files directly.

## Agent teams

> **Experimental, and off by default.** Agent teams is gated behind
> `Feature::AgentTeams`, which ships disabled. Everything in this section is
> unavailable until you turn it on.

An ordinary subagent runs, answers, and exits. A **teammate** is a named,
addressable agent that stays running alongside the main agent, can be messaged
mid-flight, and can message back. The main agent acts as a team lead handing out
work and collecting results.

### Turning it on

Add the feature to your settings:

```jsonc
// ~/.cocode/settings.json
{
  "features": {
    // Experimental: named, addressable teammates + SendMessage coordination.
    "agent_teams": true
  }
}
```

Or set `COCO_FEATURE_AGENT_TEAMS=1` in the environment for a single run. Feature
resolution runs defaults, then the `features` block in `settings.json`, then
`COCO_FEATURE_*` environment variables, so the env var wins over the file.

With the feature off, the `SendMessage` tool is hidden from the model entirely,
and the `name` and `mode` parameters are dropped from the `Agent` tool's schema
so the model cannot invent a team spawn. Attempting one anyway fails with
`Agent Teams is not available in this session.`

### Spawning a teammate

Once enabled, passing a `name` to the `Agent` tool spawns a teammate instead of
an ordinary subagent:

```
Agent({
  name: "researcher",
  description: "Survey retry strategies",
  prompt: "Survey how each provider crate handles 429 retries. Report back."
})
```

The session owns exactly one implicit team, seeded at startup. There is no
team-creation step and no team-selection parameter — the `team_name` tool
parameter is deprecated and ignored, and the `TeamCreate` / `TeamDelete` tools
have been retired.

Teammates cannot spawn other teammates: the team is one level deep, with the
main agent as lead.

A teammate spawn may also carry `mode: "plan"`, which forces that teammate into
plan mode even when the lead is running with looser permissions. This is the
only `mode` value honored, and it only ever restricts — the model cannot use it
to escalate a teammate's permissions.

### Messaging with SendMessage

`SendMessage` is the only channel between agents. An agent's ordinary text
output is not visible to its teammates; to communicate it must call the tool.
Incoming messages are delivered automatically — nobody polls an inbox.

| Parameter | Required | Meaning |
|---|---|---|
| `to` | Yes | A bare teammate name, or `"*"` to broadcast to everyone. Must not be an agent id — a value containing `@` is rejected. |
| `summary` | For plain-text messages | A 5-10 word description, used by the lead's UI message stack. |
| `message` | Yes | The message text, or a structured control payload. |

Broadcast with `"*"` is linear in team size and correspondingly expensive; it is
worth reserving for things everyone genuinely needs.

`SendMessage` also addresses background agents, not just teammates. If the
target is a background agent that has already finished, the message transparently
resumes it under the same id with its transcript intact. If it is still running,
the message is queued and surfaces on the agent's next turn.

Beyond plain text, `SendMessage` carries a small set of structured control
payloads — `shutdown_request`, `shutdown_response`, and
`plan_approval_response` — used to negotiate teammate shutdown and plan
approval. Approving a shutdown terminates the target's process; rejecting a plan
sends the teammate back to revise.

### The mailbox on disk

Teammate messaging is file-based. The tree lives under your config home:

```
~/.cocode/teams/
  <team-name>/
    config.json              # team membership and metadata
    inboxes/
      <agent-name>.json      # that agent's pending messages
    permissions/
      pending/
      resolved/
```

Writes are guarded by advisory file locks with retry and backoff, so several
processes can share a team safely. `team-lead` is the canonical name for the
lead's own inbox.

Set **`COCO_TEAMS_DIR`** to relocate the whole tree — it replaces
`~/.cocode/teams` outright. This is mainly useful for isolating tests or
pointing a team at shared storage.

### Worktree isolation

Any subagent — teammate or not — can be spawned with `isolation: "worktree"`,
which gives it a real git worktree under the project's `.cocode/worktrees/`
directory so it works on an isolated copy of the repo. The worktree is cleaned
up automatically if the agent leaves it unchanged, and stale worktrees left
behind by a crashed parent are reaped at startup.

An agent file can request this permanently with `isolation: worktree` in its
frontmatter, in which case every spawn is isolated whether or not the model asks
for it.

Two constraints: worktree isolation only works inside a git repository, and it
is mutually exclusive with a spawn-time `cwd` override, since an isolated agent
runs in the worktree's path by definition.

Note that `Feature::Worktree` gates the interactive `EnterWorktree` /
`ExitWorktree` **tools**, not subagent isolation. Spawning with
`isolation: "worktree"` needs only a git repo, no feature flag.

### Configuration

The `agent_teams` block in `settings.json` carries the team's internal
parameters. It does **not** turn the feature on — that is the `features` block
above.

```jsonc
// ~/.cocode/settings.json
{
  "features": {
    "agent_teams": true
  },
  "agent_teams": {
    // How teammates are spawned. Default: "in-process".
    "teammate_mode": "in-process",

    // Model role for teammates that don't specify one. Default: "main".
    "default_model_role": "main",

    // Per-agent-type role overrides.
    "agent_type_model_roles": {
      "researcher": "explore"
    },

    // Show the spinner tree instead of pills. Default: true.
    "show_spinner_tree": true,

    // Max concurrent in-process agents. Default: 8. Values below 1 clamp to 1.
    "max_agents": 8
  }
}
```

`teammate_mode` picks the backend that hosts each teammate:

| Value | Behavior |
|---|---|
| `in-process` | **Default.** Teammates run inside the main process. No terminal multiplexer needed. |
| `tmux` | Force the tmux backend — each teammate gets its own pane. |
| `iterm2` | Force the iTerm2 backend. |
| `auto` | Try tmux or iTerm2 first, fall back to in-process. |

Panes are opt-in: the default is `in-process`, so you get teammates without any
terminal setup, and you choose `auto`, `tmux`, or `iterm2` when you want to
watch each teammate work in its own pane.

One limit worth knowing: in-process teammates cannot spawn **background**
sub-agents, because a background child would outlive the supervisor its
lifecycle is bound to. A synchronous spawn from inside a teammate is fine.

### Remote agents

Remote agent execution is deliberately not supported. `isolation: "remote"`
parses, but every spawn that resolves to it is rejected:

```
Isolation mode 'remote' is not supported in this build. Use 'worktree' for
local isolation or omit the field for no isolation.
```

Use `worktree` for local isolation instead.
