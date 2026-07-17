# Tools

Tools are the actions the model can take on your machine: reading and writing files, running shell
commands, searching the web, spawning subagents, and so on. This page lists every built-in tool,
explains which ones are visible by default, and shows how to allow or deny them.

## What a tool is

A tool is a named function the model can call during a turn. Each tool has a JSON schema for its
input, and cocode executes it locally and feeds the result back into the conversation. The model
never runs code itself — it asks for a tool call, cocode decides whether that call is allowed, runs
it, and returns the output.

Two independent mechanisms decide what actually happens when the model asks for a tool:

**Visibility** determines whether the model is even told the tool exists. A tool that is filtered out
is absent from the request payload, so the model cannot call it. Visibility is computed fresh for
every turn from four things, in this order:

1. The tool's own gate. Most tools check a [feature flag](configuration.md) — `WebSearch` requires
   the `web_search` feature, `NotebookEdit` requires `notebook_edit`, and so on. Some check runtime
   state instead: `Bash` only appears when the active shell tool is bash, and `LSP` additionally
   requires a language server to be connected.
2. Model overrides. A model entry can declare tools it adds beyond the baseline and tools it
   rejects. This is how the gpt-5 family swaps `Write`/`Edit` for `apply_patch`.
3. The session tool filter, populated from `--allowed-tools` / `--disallowed-tools`.
4. Subagent narrowing. A subagent inherits its parent's filter and may only narrow it further,
   never widen it.

**Permission** determines whether an allowed-to-be-visible call is actually executed. This is a
separate evaluation that happens at call time against your allow/ask/deny rules and the current
permission mode. A tool can be perfectly visible and still be denied, or prompt you for approval.
See [permissions](permissions.md) for the full model.

## The built-in tools

The "gate" column names the feature flag or runtime condition that must hold for the model to see
the tool, and whether that gate is on by default. "Always" means the tool is visible in every
session unless you filter it out yourself.

<!-- BEGIN GENERATED: tools -->

### File I/O

| Tool | What it does | Gate (default) |
|---|---|---|
| `Bash` | Runs a shell command, optionally in the background. | Active shell tool is bash (on, except Windows) |
| `PowerShell` | Runs a PowerShell command. | Active shell tool is powershell (on for Windows) |
| `Read` | Reads a file — text, images, PDFs, and notebooks. | Always |
| `Write` | Creates a file or overwrites an existing one. | Always (removed for gpt-5 models) |
| `Edit` | Replaces an exact string inside an existing file. | Always (removed for gpt-5 models) |
| `Glob` | Finds files by name pattern or wildcard. | Always |
| `Grep` | Searches file contents with a regex, via ripgrep. | Always |
| `NotebookEdit` | Edits a cell inside a Jupyter `.ipynb` notebook. | `notebook_edit` (off) |
| `apply_patch` | Applies a freeform unified-diff patch that creates, edits, or deletes files in one envelope. | Only for models that declare it (off) |

### Web

| Tool | What it does | Gate (default) |
|---|---|---|
| `WebFetch` | Fetches a URL and extracts its content as text. | `web_fetch` (on) |
| `WebSearch` | Searches the web for current information. | `web_search` (on) |

### Agent, workflow, and team

| Tool | What it does | Gate (default) |
|---|---|---|
| `Agent` | Delegates a scoped piece of work to a subagent that runs its own turn loop and reports back. | Always |
| `Workflow` | Runs a local workflow script. Also callable as `RunWorkflow`. | `workflow` (on) |
| `Skill` | Invokes a skill (a slash-command-style markdown workflow). | Always |
| `SendMessage` | Sends a message to a named teammate in the session team. | `agent_teams` (off, experimental) |

### Task management

| Tool | What it does | Gate (default) |
|---|---|---|
| `TaskCreate` | Creates a durable plan item. | `task_v2` (on) |
| `TaskGet` | Reads one task by id. | `task_v2` (on) |
| `TaskList` | Lists the session's tasks. | `task_v2` (on) |
| `TaskUpdate` | Updates a task's status or contents. | `task_v2` (on) |
| `TodoWrite` | Rewrites the per-agent todo checklist. The v1 fallback for `task_v2`. | `task_v2` **off** |
| `TaskStop` | Kills a running background task. Also matched by the legacy name `KillShell`. | Always |
| `TaskOutput` | Deprecated fallback for reading a background task's output. | Always |

### Plan and worktree

| Tool | What it does | Gate (default) |
|---|---|---|
| `EnterPlanMode` | Switches the session into plan mode so an approach is designed before code is written. | `plan_mode` (on) |
| `ExitPlanMode` | Presents the finished plan for approval and leaves plan mode. | `plan_mode` (on) |
| `EnterWorktree` | Creates an isolated git worktree and switches the session into it. | `worktree` (off) |
| `ExitWorktree` | Leaves a worktree and returns to the original directory. | `worktree` (off) |

### Utility

| Tool | What it does | Gate (default) |
|---|---|---|
| `AskUserQuestion` | Asks you a multiple-choice question and waits for your pick. | Always |
| `ToolSearch` | Loads the full schema of a deferred tool on demand. | `tool_search` (on), and only when the turn actually has deferred tools |
| `Config` | Reads or writes cocode settings such as model and provider. | Always |
| `LSP` | Code intelligence: definitions, references, hover, symbols, call hierarchy. | `lsp` (off) **and** a language server connected |
| `SendUserMessage` | Sends a message straight to you on a side channel. | `kairos_brief` (off) |
| `Sleep` | Pauses execution for a number of seconds. | `proactive` (off) |

### MCP management

| Tool | What it does | Gate (default) |
|---|---|---|
| `McpAuth` | Starts or refreshes authentication for an MCP server. | `mcp` (on) |
| `ListMcpResourcesTool` | Lists resources published by connected MCP servers. | `mcp` (on) |
| `ReadMcpResourceTool` | Reads one MCP resource by URI. | `mcp` (on) |
| `ReadMcpResourceDirTool` | Lists the children of an MCP directory resource. | `mcp` (on) |

### Scheduling

| Tool | What it does | Gate (default) |
|---|---|---|
| `CronCreate` | Schedules a prompt to run on a cron schedule or once at a future time. | `agent_triggers` (on) |
| `CronDelete` | Cancels a scheduled job. | `agent_triggers` (on) |
| `CronList` | Lists active scheduled jobs. | `agent_triggers` (on) |
| `ScheduleWakeup` | Sets the delay before the next self-paced iteration. | `agent_triggers` (on) |
| `Monitor` | Watches an event stream and wakes the agent when output arrives. | `agent_triggers` (on) **and** a task handle on the session |
| `RemoteTrigger` | Manages scheduled remote agent triggers. | `agent_triggers_remote` (off) |

### Goals

| Tool | What it does | Gate (default) |
|---|---|---|
| `get_goal` | Reads the live goal: objective, status, budget, usage. | A goal is live |
| `report_goal_turn` | Reports how the turn advanced the goal, with evidence. | A goal is live |

### SDK-internal

| Tool | What it does | Gate (default) |
|---|---|---|
| `StructuredOutput` | Captures the model's final answer as JSON matching a caller-supplied schema. | Injected only when a JSON schema is supplied (off) |

<!-- END GENERATED: tools -->

## Things that trip people up

**There is no `LS` tool.** Use `Glob` to list files by pattern, or `Bash` with `ls`. Some built-in
prompt bodies still mention `LS` in their pre-approved tool lists; that entry is inert because no
such tool is registered.

**The subagent tool is `Agent`, not `Task`.** `Task` is accepted as a legacy alias when matching
permission rules and hook matchers, so an old `Task` rule keeps working. It is not a callable tool
name — the model always sees and calls `Agent`. The same legacy-alias treatment applies to
`RunWorkflow` (canonical `Workflow`), `KillShell` (canonical `TaskStop`), and `AgentOutputTool` /
`BashOutputTool` (canonical `TaskOutput`). Of these, only `RunWorkflow` is also registered as a real
callable alias.

**`apply_patch` is lowercase, and most models never see it.** cocode does not keep a per-model table
of which edit tool to use. Instead it derives the answer from what the model actually has. A model
entry declares a diff against the universal tool baseline: extra tools it adds, baseline tools it
rejects. The gpt-5 family declares `apply_patch` as an extra and rejects both `Write` and `Edit`,
because the patch envelope's `*** Add File` and `*** Update File` hunks cover creating and editing
alike. Everything else keeps `Write` and `Edit` and never sees `apply_patch`.

Every prompt that needs to name a write or edit tool resolves it through the same rule, using the
tools available for that turn (`write_tool_for` for creation, `edit_tool_for` for modification, both
built on `file_mutation_tool`): if the native tool is present, use it; otherwise, if `apply_patch` is
present, use that; otherwise fall back to the native name. That means plan-mode reminders, subagent
prompt examples, and post-compaction plan references all name a tool the model can actually call,
with no per-model special-casing. A future model family that follows the same "drop native, add
`apply_patch`" shape works automatically.

**Goal tools are snake_case and only exist while a goal is live.** `get_goal` and `report_goal_turn`
appear only when the session has an active goal, which you create with [`/goal`](slash-commands.md).
When no goal is running, neither tool is in the model's tool list. Creating a goal is a user action,
not a model action.

**`StructuredOutput` is not part of any normal session.** It is not in the default tool set at all.
It is injected into a private registry only when a caller supplies a JSON schema — `--json-schema`
on a non-interactive run (`-p` print mode or the SDK), a workflow that spawns an agent with a
schema, or the hook-agent runner. The flag is ignored in the TUI, and the tool is never visible
there.

**Bash and PowerShell are mutually exclusive.** Both are registered, but only the active one is
visible. The choice comes from `shell.tool` in your settings: `auto` (the default) picks PowerShell
on Windows and bash everywhere else, and `bash` / `powershell` force one. Setting it to `disabled`
hides both, so the model has no shell tool at all. If the setting names a shell whose binary is not
installed, the session fails to start rather than silently exposing a broken tool.

## MCP tools

Tools published by [MCP servers](configuration.md) are registered dynamically once the server
connects, and are exposed to the model as `mcp__<server>__<tool>`. A server named `slack` publishing
a `send_message` tool appears as `mcp__slack__send_message`. The same qualified name is what you
write in permission rules and tool filters.

When a server needs authentication, cocode surfaces a per-server `mcp__<server>__authenticate`
pseudo-tool so the model can tell you exactly which server is stuck. It disappears on its own once
the real tools register after a successful reconnect.

All MCP tools are gated behind the `mcp` feature, which is on by default. Turning it off hides both
the MCP management tools and every dynamically registered server tool.

## Deferred tools and `ToolSearch`

Sending every tool's full JSON schema on every request is expensive, especially once you have MCP
servers connected. With the `tool_search` feature on (the default), tools that opt into deferral are
sent to the model name-only, and the model calls `ToolSearch` to pull in the full schema of the ones
it actually wants. On providers that support a native deferred-tool-reference flag, the expansion
happens server-side instead.

Turning `tool_search` off does two things at once: it hides the `ToolSearch` tool, and it
short-circuits the deferral filter so every visible tool gets its full schema in every request.
That is the right choice when token budget is not a concern and you would rather avoid the
`ToolSearch` round-trip.

The high-frequency plan and todo tools (`TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate`,
`TodoWrite`) deliberately never defer, so weaker models do not need a discovery round-trip before
their first call.

## Allowing and denying tools

There are two separate levers, and it matters which one you reach for.

### `--allowed-tools` / `--disallowed-tools`

These flags build the session's tool filter and take exact tool names, repeatable:

```bash
coco -p "summarize the test failures" \
  --allowed-tools Read Grep Glob \
  --disallowed-tools Bash
```

Supplying `--allowed-tools` turns the filter into a whitelist: only the named tools survive, and
everything else is hidden. `--disallowed-tools` always wins over `--allowed-tools` for the same
name. Names are matched exactly against the wire strings in the tables above, including the
qualified `mcp__server__tool` form.

Two limits worth knowing. First, these flags only take effect in non-interactive runs — `-p` print
mode and SDK sessions. The TUI ignores them; scope tool access there through permission rules
instead. Second, the filter matches whole tool names only. A scoped pattern like `Bash(git status:*)`
is not a tool name and will never match anything here.

### Permission rules

Scoping is a permission-rule concept, not a tool-filter one. Rules live in your settings and can
narrow a tool to specific arguments:

```jsonc
// ~/.cocode/settings.json
{
  "permissions": {
    "allow": [
      "Read",
      "Bash(git status:*)",   // this exact command shape, auto-approved
      "Bash(git diff:*)"
    ],
    "deny": [
      "Bash(rm:*)"            // never, regardless of mode
    ]
  }
}
```

A bare tool name in a rule covers every call to that tool. The `Tool(pattern)` form narrows it to
calls whose arguments match. Deny rules always beat allow rules. See
[permissions](permissions.md) for rule precedence, the permission modes, and how the classifier
decides on calls no rule covers.

Some built-in slash commands ship their own pre-approved rule sets so they can do their job without
interrupting you — `/commit` pre-allows `Bash(git add:*)`, `Bash(git status:*)`, and
`Bash(git commit:*)` for the duration of that turn. Those grants are scoped to the command's turn
and are not written to your settings.

## Related

- [Slash commands](slash-commands.md) — the `/` commands you type in the TUI
- [Permissions](permissions.md) — allow/ask/deny rules and permission modes
- [Configuration](configuration.md) — feature flags and settings layering
- [Subagents and teams](subagents-and-teams.md) — how `Agent` and `SendMessage` are used in practice
