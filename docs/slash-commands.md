# Slash commands

Slash commands are the `/`-prefixed commands you type into the cocode prompt to control the session
without talking to the model. This page lists every built-in one, what it does, and what arguments it
takes.

## How they work

Type `/` in the TUI and a typeahead list appears; keep typing to filter it, and press Enter to run
the highlighted command. Anything you type after the command name is passed to it as a single
argument string, so `/model gpt-5-4` hands `gpt-5-4` to the `/model` command.

Commands behave in one of three ways:

- **Inline commands** print a result straight into the transcript and end there. `/cost` and
  `/version` are like this.
- **Overlay commands** open a full-screen panel you interact with and then dismiss. `/model` with no
  argument opens the model picker; `/help`, `/context`, `/diff`, and `/resume` all take over the
  screen. Most overlay commands also accept arguments, in which case they skip the overlay and print
  text instead — useful in scripts and in the SDK, where there is no screen to take over.
- **Prompt commands** do not do anything themselves. They assemble a prompt (often with live git
  context baked in) and hand it to the model as if you had typed it. `/commit`, `/review`, and
  `/security-review` work this way. Some of them pre-approve the specific tool calls they need so
  the turn does not stop to ask you for permission on every step.

Many commands have aliases. `/perms` is `/permissions`, `/fork` is `/branch`, `/?` is `/help`. Both
spellings are equivalent.

Beyond the built-ins listed here, your [skills](configuration.md) and installed
[plugins](configuration.md) contribute their own slash commands, which show up in the same typeahead.
A built-in always wins a name collision with a skill or plugin command.

## Core

| Command | Aliases | What it does | Arguments |
|---|---|---|---|
| `/help` | `/h`, `/?` | Show available commands and help | `[command]` |
| `/clear` | `/reset`, `/new` | Clear conversation history and start fresh | — |
| `/compact` | — | Compact conversation to reduce context usage | `[instructions]` |
| `/status` | — | Show current session status and model info | — |
| `/plan` | `/planning` | Toggle plan mode or view the current plan | `[open\|<description>]` |
| `/goal` | — | Set a goal the agent checks before stopping | `[<condition> \| clear]` |
| `/btw` | — | Ask a quick side question without interrupting the main conversation | `<question>` |
| `/exit` | `/quit` | Exit the REPL | — |
| `/version` | — | Show version info | — |
| `/feedback` | — | Prepare a cocode GitHub feedback issue | `[--with-logs] <report>` |

## Configuration

| Command | Aliases | What it does | Arguments |
|---|---|---|---|
| `/config` | `/settings` | Show or modify configuration | `[key] [value]` |
| `/model` | — | Switch the current model; opens a picker with no argument | `[model]` |
| `/moa` | — | Run one prompt through the default MoA preset | `<prompt>` |
| `/provider` | — | Add or configure an LLM provider (opens a wizard) | — |
| `/login` | — | Log in to a provider subscription via OAuth; opens a picker with no argument | `[provider]` |
| `/logout` | — | Clear a provider subscription credential | `[provider]` |
| `/permissions` | `/perms`, `/allowed-tools` | Manage allow and deny tool permission rules | `[allow\|deny] [tool]` |
| `/theme` | — | Change the theme (always opens the live-preview picker) | — |
| `/color` | — | Set the prompt bar color for this session | `<color\|default>` |
| `/vim` | — | Toggle between Vim and Normal editing modes | `[on\|off\|toggle]` |
| `/voice` | — | Enable or disable voice input (speech-to-text dictation) | `[on\|off\|toggle]` |
| `/voice-config` | — | Inspect or edit voice-input settings (backend, language, model) | `[lang\|backend\|remote\|local\|download] ...` |
| `/keybindings` | — | Open or create your keybindings configuration file | — |
| `/sandbox` | — | Configure sandbox mode | `[none\|readonly\|strict]` |

`/login` and `/logout` are not listed in `/help`'s built-in text, but they are registered and work.
`/sandbox`'s hint is out of date: the modes it actually accepts are `read_only`, `workspace_write`,
`full_access`, and `external_sandbox`, plus the `exclusions`, `exclude <pattern>`, and
`unexclude <pattern>` subcommands. See [sandbox](sandbox.md).

## Session

| Command | Aliases | What it does | Arguments |
|---|---|---|---|
| `/session` | `/remote` | Manage sessions (list, resume, delete) | `[list\|delete\|info] [id]` |
| `/resume` | — | Resume a previous conversation | `[session-id]` |
| `/branch` | `/fork` | Branch the current conversation into a new session | `[title]` |
| `/rename` | — | Rename the current conversation | `[name]` |
| `/tag` | — | Toggle a searchable tag on the session | `<name>` |
| `/context` | — | Show context window usage breakdown | — |
| `/cost` | — | Show total cost and duration of this session | — |
| `/stats` | — | Show usage statistics and activity | — |
| `/export` | — | Export the conversation to a file (format inferred from extension) | `[filename]` |
| `/copy` | — | Copy last assistant response to clipboard | — |
| `/rewind` | — | Rewind to a previous turn | — |
| `/memory` | — | Open the memory file selector | — |
| `/dream` | — | Force auto-memory consolidation now (skips the three-gate scheduler) | — |
| `/summary` | — | Force a 9-section session-memory update now | — |

`/rename` with no argument generates a name for you using the Fast model role. `/export` accepts
either a filename, in which case the format comes from the extension, or a bare format keyword
(`markdown`, `json`, `text`), in which case it writes a timestamped file in the session's original
working directory; with no argument it opens the format picker. `/dream` and `/summary` only exist
when the `auto_memory` feature is on — see
[conditional commands](#commands-that-are-not-always-there).

## Development

| Command | Aliases | What it does | Arguments |
|---|---|---|---|
| `/diff` | — | Show git diff of current changes | — |
| `/commit` | — | Create a git commit | `[additional guidance]` |
| `/commit-push-pr` | — | Commit, push, and open a pull request — orchestrated | `[additional instructions]` |
| `/review` | — | Review a GitHub pull request | `[pr number]` |
| `/security-review` | — | Complete a security review of the pending changes on the current branch | — |
| `/pr-comments` | — | Get comments from a GitHub pull request | — |
| `/init` | — | Initialize a CLAUDE.md (and optional skills/hooks) for this repo | — |

`/commit` and `/commit-push-pr` resolve your git status, diff, log, and branch inline before handing
the prompt to the model, and pre-approve the exact `git` (and, for the PR flow, `gh`) command shapes
they need. `/review` runs at medium thinking effort and is scoped to pull requests.

## Tools and plugins

| Command | Aliases | What it does | Arguments |
|---|---|---|---|
| `/agents` | — | List, show, validate, or reload agent definitions | `[list\|show <name>\|paths\|validate\|reload]` |
| `/skills` | — | List discovered skills; opens a dialog with no argument | `[list\|show <name>\|paths]` |
| `/workflow` | `/workflows` | Run a local workflow script | `[name\|scriptPath\|task]` |
| `/tasks` | `/bashes` | List and manage active tasks | — |
| `/hooks` | — | View hook configurations for tool events | — |
| `/lsp` | — | Manage LSP servers (status, install, enable/disable, add/remove) | `[list\|install\|enable\|disable\|add\|remove] [server]` |
| `/mcp` | — | Manage MCP servers | `[list\|add\|remove\|enable\|disable] [name]` |
| `/plugin` | `/plugins`, `/marketplace` | Manage installed plugins | `[list\|install\|uninstall] [name]` |
| `/reload-plugins` | — | Reload plugin definitions | — |
| `/files` | — | List git-tracked files in this repository | — |

## System and misc

| Command | Aliases | What it does | Arguments |
|---|---|---|---|
| `/doctor` | — | Diagnose and verify installation and settings | — |
| `/upgrade` | — | Check for updates | — |
| `/usage` | — | Show plan usage limits | — |
| `/add-dir` | — | Add a new working directory | `<path>` |
| `/ide` | — | Manage IDE integrations | — |
| `/statusline` | — | Set up the status line UI | — |
| `/insights` | — | Surface session insights, costs, and notable activity | — |
| `/env` | `/environment` | Show runtime environment (cwd, model, shell, version) | — |
| `/debug-tool-call` | — | Emit debug info for a pending tool call | `[call-id]` |
| `/output-style` | — | Deprecated: use `/config` to change output style | — |

`/usage` currently reports that plan usage information is not available and points you at `/cost`.
`/upgrade` always reports that you are on the latest version.

## Commands that are not always there

Three built-ins are conditional, and it is worth knowing why so their absence does not look like a
bug.

**`/dream` and `/summary` only exist when the `auto_memory` feature is on**, and that feature is off
by default. They are the entry points to the auto-memory subsystem, and registering them
unconditionally would put commands in the typeahead that silently do nothing. Turn the feature on to
get them:

```jsonc
// ~/.cocode/settings.json
{
  "features": {
    "auto_memory": true
  }
}
```

**`/compact` is hidden when its kill switch is set.** Setting `COCO_COMPACT_DISABLE` to a truthy
value hard-disables all compaction, automatic and manual, and removes `/compact` from the command
list. If you only want to stop *automatic* compaction while keeping manual `/compact` working, use
`COCO_COMPACT_DISABLE_AUTO` instead.

## Hidden but invocable

Three commands are deliberately kept out of the `/` typeahead while remaining fully callable if you
type the name. `/env` prints the resolved runtime environment — working directory, model, shell,
version — and `/debug-tool-call` dumps internals for a pending tool call. Both are debugging aids
that would only be noise for most users.

`/output-style` is a different case: it is a deprecation stub. It takes no action and prints a
message telling you to use `/config` instead, or to set the style in your settings file. It is
hidden so it does not advertise itself, but it still answers if you invoke it, so anyone with the
old muscle memory gets a pointer rather than an "unknown command" error.

## The ones you will actually use

### `/model`

The main way to change which model you are talking to. With no argument it opens the provider-grouped
picker overlay, which has a role pill so you can edit any model role slot, not just the main one.
With an argument it skips the overlay: `/model gpt-5-4` resolves the id against the built-in registry
and persists it to `models.main` in your user settings.

You can target a specific role by prefixing the argument with the role name — `/model fast gemini-3-flash`
writes `models.fast` and leaves `models.main` alone. The recognized roles are `main`, `plan`, `fast`,
`explore`, `review`, `subagent`, `memory`, and `hook_agent`. If the first word is not a role name,
the whole argument is treated as a model id for `main`.

A role can also be bound to a Mixture-of-Agents preset instead of a single model by passing
`moa/<preset>`, where `<preset>` names an entry under `moa.presets` in your settings —
`/model moa/default` binds the main role to the `default` preset. See
[models and MoA](models-and-moa.md).

### `/moa`

Runs a single prompt through the MoA preset named in `moa.default_preset`, without touching any of
your model role bindings. Use it when you want one hard question answered by the ensemble but do not
want to switch the whole session over. It needs a prompt: `/moa why is this test flaky?`.

### `/goal`

Sets a persistent goal that the agent checks before it stops, which is what drives autonomous
multi-turn work. `/goal <condition>` sets it (conditions are capped at 4000 characters), and bare
`/goal` reports current status. `/goal pause` halts autonomous work while keeping the goal,
`/goal resume` restarts it, and `/goal clear` drops the goal entirely — `stop`, `off`, `reset`,
`none`, and `cancel` all work as synonyms for clearing. While a goal is live, the model gets the
`get_goal` and `report_goal_turn` [tools](tools.md).

### `/plan`

Plan mode makes the assistant design an approach and get your approval before it writes anything.
Bare `/plan` shows the current plan file for the session, `/plan open` opens it in `$EDITOR`, and
`/plan <description>` asks the model to enter plan mode and plan for that task. Plans are saved
under `~/.cocode/plans/`.

### `/login`

Signs you into a provider subscription over OAuth. Bare `/login` opens a picker of configured
providers; `/login openai` goes straight in. It opens your browser, and prints the URL into the
transcript as a fallback if the browser does not open. After a successful login it refreshes the
provider status list and discovers the provider's live model catalog in the background, so
subscription-only models show up in `/model` without a restart. Outside the TUI there is no browser
to drive, so the command just points you at `coco login <provider>`. See
[providers and auth](providers-and-auth.md).

### `/provider`

Opens the add-provider wizard. There is no argument form — it is interactive only. Use this to wire
up a provider that is not one of the built-ins, or to point an OpenAI-compatible endpoint at a
custom base URL.

### `/permissions`

Bare `/permissions` (or `/permissions list`) opens the tabbed rule editor for the allow and deny
lists that decide which tool calls run without asking you. The mutating subcommands skip the
overlay: `/permissions allow <tool>` and `/permissions deny <tool>` add a rule for this session, and
`/permissions reset` clears session rules. Rules can be bare tool names or scoped to argument
patterns like `Bash(git status:*)`. `reset` only affects session rules — rules written in your
settings files are untouched, so edit those directly to change persistent rules. See
[permissions](permissions.md).

### `/compact`

Summarizes the conversation so far and replaces the history with the summary, reclaiming context
window. Pass instructions to steer what the summary keeps: `/compact focus on the failing
migration`. Compaction also happens automatically as you approach the context limit; this command is
the manual trigger.

### `/context`

Opens a breakdown of what is currently occupying your context window — system prompt, memory files,
attachments, tool results, conversation. This is the command to reach for when you want to know *why*
you are close to a compaction, not just that you are.

### `/resume`

Opens the session picker so you can jump back into a previous conversation, or takes a session id
directly to skip the picker. Related: `/branch` forks the current conversation into a new session
and switches to it, leaving the original intact, and `/rewind` steps the current session back to an
earlier turn.

### `/agents`

Your window into subagents. With no argument it opens a two-tab overlay showing agents currently
running and the library of agent definitions available. The subcommands stay text-only for scripts
and the SDK: `list`, `show <name>`, `paths` (where definitions are being read from), `validate`
(check definitions parse and their tool lists resolve), and `reload`. See
[subagents and teams](subagents-and-teams.md).

### `/workflow`

Launches a local workflow script through the `Workflow` tool. Bare `/workflow` opens a picker of the
workflows it discovered. With an argument — a workflow name, a script path, or just a description of
the task — it becomes a prompt command: it pre-approves the `Workflow` tool and hands the request to
the model rather than running anything directly.

## Related

- [Tools](tools.md) — what the model can call, and how to allow or deny it
- [Models and MoA](models-and-moa.md) — model roles, aliases, and Mixture-of-Agents presets
- [Providers and auth](providers-and-auth.md) — OAuth subscriptions and API keys
- [Permissions](permissions.md) — allow/ask/deny rules and permission modes
- [Subagents and teams](subagents-and-teams.md) — agent definitions and teammate coordination
