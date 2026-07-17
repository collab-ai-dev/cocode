# Extending cocode

Three mechanisms let you add behavior to cocode without touching its source. **Skills** are reusable prompts the model can invoke. **Plugins** package skills, hooks, agents, commands, and MCP servers into a distributable unit. **Hooks** run your own code at defined points in the agent's lifecycle. This page covers all three.

For settings-file layering and feature flags, see [configuration](configuration.md).

## Skills

A skill is a markdown file containing a prompt, plus optional YAML frontmatter describing when and how to use it. The model invokes skills through the `Skill` tool, and you can invoke any of them yourself by typing `/<name>`.

### Format

A skill lives at `<skills-dir>/<skill-name>/SKILL.md`. **The skill's name comes from its directory name, not from the frontmatter.** A directory called `review-pr` gives you a skill named `review-pr`, invocable as `/review-pr`. The `SKILL.md` filename is matched case-insensitively.

```markdown
---
description: Review a pull request for correctness and style issues
when-to-use: Use when the user asks for a PR review or mentions reviewing changes
allowed-tools: [Bash, Read, Grep, Glob]
argument-hint: "<pr-number>"
---

Review pull request $1.

Start by fetching the diff with `gh pr diff $1`. Look for correctness bugs
first, then style. Report findings grouped by file, most severe first.
```

Frontmatter is entirely optional — a `SKILL.md` with nothing but a prompt body works. If you omit `description`, cocode extracts one from the first non-empty line of the body.

| Key | Aliases | Type | Meaning |
|-----|---------|------|---------|
| `description` | — | string | What the skill does. Shown to the model in its skill listing. |
| `name` | — | string | **Display label only.** Does not change the invocation name, which is always the directory name. |
| `when-to-use` | `when_to_use` | string | Guidance to the model on when to reach for this skill. |
| `allowed-tools` | `allowed_tools` | list or CSV string | Restrict the skill to these tools. |
| `argument-hint` | `argument_hint` | string | Usage hint shown in `/` autocomplete. |
| `arguments` | `argument-names`, `argument_names` | list or whitespace-separated string | Named parameters the skill accepts. |
| `aliases` | — | list or CSV string | Alternative invocation names. |
| `model` | — | string | Pin the skill to a specific model. |
| `model-role` | `model_role`, `modelRole` | string | Run the skill under a semantic model role instead of a fixed model. |
| `effort` | — | string | Reasoning-effort override. |
| `context` | — | `inline` \| `fork` | `inline` (default) expands the prompt into the current conversation; `fork` runs it as an isolated subagent. |
| `agent` | — | string | Agent type to use when `context: fork`. |
| `paths` | — | list or CSV string | Glob patterns. Makes the skill conditional — it surfaces only when a matching file is in play. |
| `disabled` | — | bool | Skip this skill at load time. |
| `user-invocable` | `user_invocable` | bool | Default `true`. Set `false` to hide it from `/` and expose it only to the model. |
| `disable-model-invocation` | `disable_model_invocation` | bool | Default `false`. Set `true` to make it user-only. |
| `version` | — | string | Semantic version. |
| `hooks` | — | object | Hook configuration scoped to this skill. |
| `shell` | — | object | Shell configuration scoped to this skill. |

List-valued keys accept either a YAML sequence (`[Bash, Read]`) or a comma-separated string (`Bash, Read`).

### Where skills are discovered

| Source | Location |
|--------|----------|
| Bundled | Compiled into the binary. No files on disk. |
| Managed | Enterprise policy directory. |
| User | `~/.cocode/skills/` |
| Project | `.cocode/skills/`, walked upward from the current directory and stopping at the git root or your home directory, whichever comes first. |
| Additional directories | `<dir>/.cocode/skills/` for each `--add-dir` root. |
| Plugin | Contributed by an installed plugin, namespaced as `plugin-name:skill-name`. |

Registration is **first-wins by name**: the first source in the order above to define a name keeps it. Because plugin skills are namespaced, they rarely collide with your own.

Put project-specific workflows in `.cocode/skills/` and commit them — everyone on the repo gets them. Put personal ones in `~/.cocode/skills/`.

Skills reload automatically. Editing a `SKILL.md` rebuilds the catalog and the slash-command registry without a restart.

### Invoking skills

Every user-invocable skill is registered as a slash command, so `/review-pr 1234` works. The model reaches the same skill through the `Skill` tool, which takes `skill` (required) and `args` (optional).

`/skills` opens a dialog listing every skill, including conditional ones, with its override state. It also accepts text subcommands: `/skills list`, `/skills show <name>`, and `/skills paths` (which prints the discovery order).

You can adjust any skill's visibility in settings without editing its file:

```jsonc
// ~/.cocode/settings.json
{
  "skill_overrides": {
    // "on" | "name-only" | "user-invocable-only" | "off"
    "noisy-skill": "name-only", // model sees the name, not the description
    "dangerous-skill": "user-invocable-only", // only runs if you type /dangerous-skill
    "unwanted-skill": "off"
  }
}
```

### Skill learning

> **Status: off by default, and under development.** The `skill_learning` feature is marked `UnderDevelopment` and defaults to false.

When enabled, a turn-end review fork distills sessions into new agent-authored skills, written under `~/.cocode/skills/.agent/`, and a periodic curator retires skills with a low success rate and promotes ones with a high one. Agent-authored skills can never shadow a human-written skill of the same name, and a file that declares itself agent-authored has its `allowed-tools`, `hooks`, and `shell` keys stripped at parse time.

This gate covers only the autonomous learning loop. It is unrelated to `/hunter`, which is a bundled skill behind a different flag.

## Plugins

A plugin is a directory with a `PLUGIN.toml` manifest at its root. Plugins bundle skills, hooks, agents, commands, MCP servers, LSP servers, and output styles into one installable unit.

### `PLUGIN.toml`

The manifest is **flat** — there is no `[plugin]` wrapper table. Only `name` is required. Unknown keys are ignored rather than rejected.

```toml
name = "acme-tools"
version = "1.2.0"
description = "Acme's internal review and deploy workflows"
homepage = "https://example.com/acme-tools"
repository = "https://github.com/acme/acme-tools"
license = "Apache-2.0"
keywords = ["review", "deploy"]

# Other plugins this one needs; "name" or "name@marketplace"
dependencies = ["acme-base"]

# Contribution paths, relative to the plugin root.
# Each accepts a single string or a list of strings.
skills = "skills"
agents = ["agents/reviewer.md"]
output_styles = "styles"

# Hooks: a path to a JSON file, an inline table, or a list of either.
hooks = "hooks.json"

# MCP servers the plugin brings with it, same shape as .mcp.json
[mcp_servers.acme-api]
command = "npx"
args = ["-y", "@acme/mcp-server"]

[author]
name = "Acme Engineering"
email = "eng@example.com"

# Options prompted for when the plugin is enabled
[user_config.api_region]
# see the plugin's own docs for option fields

# Host version compatibility
min_version = "1.0.0"
```

`plugin.json` with the same field names is accepted as an alternative to `PLUGIN.toml`.

The full set of top-level keys: `name`, `version`, `description`, `author`, `homepage`, `repository`, `license`, `keywords`, `dependencies`, `skills`, `hooks`, `agents`, `commands`, `mcp_servers`, `lsp_servers`, `output_styles`, `channels`, `user_config`, `settings`, `env_vars`, `min_version`, `max_version`.

### Installing and managing

Plugins are discovered in `~/.cocode/plugins/*/` and `<project>/.cocode/plugins/*/`. Each subdirectory must contain a manifest. Marketplace-installed plugins live in a versioned cache under `~/.cocode/plugins/cache/` rather than in those directories.

These CLI subcommands are fully implemented:

```bash
coco plugin list                 # installed plugins with version and source
coco plugin install ./my-plugin  # install from a local directory
coco plugin install foo@acme     # install from a registered marketplace
coco plugin uninstall foo
coco plugin validate ./my-plugin # check PLUGIN.toml, print contributed fields
```

`coco plugin install` picks its path from the argument: if it resolves to a local directory containing a manifest, it validates and copies it; otherwise it is treated as `<name>[@<marketplace>]` and installed from a marketplace.

Inside a session, `/plugin` (aliases: `/plugins`, `/marketplace`) opens a picker dialog. It also accepts `list`, `install <name>`, `uninstall <name>`, `info <name>`, `search <query>`, `enable <name>`, and `disable <name>`.

Enable state lives in settings and is what the loader actually reads:

```jsonc
// ~/.cocode/settings.json
{
  "enabled_plugins": {
    "acme-tools": { "enabled": true }
  }
}
```

### Marketplaces

A marketplace is a catalog of installable plugins, described by a `marketplace.json` at the root of the source (or under `.claude-plugin/marketplace.json`).

**Adding a marketplace is a separate operation from installing a plugin.** `coco plugin install` only ever takes a plugin id. To register a marketplace, use the slash command:

```
/plugin marketplace add <source>
```

The source is auto-detected from its shape:

| Form | Example |
|------|---------|
| GitHub shorthand | `acme/plugin-catalog` or `acme/plugin-catalog#main` |
| GitHub URL | `https://github.com/acme/plugin-catalog` |
| Git over HTTPS | `https://git.example.com/catalog.git` |
| Git over SSH | `git@example.com:acme/catalog.git` |
| Plain HTTP(S) JSON | `https://example.com/marketplace.json` |
| Local path | `./catalog`, `~/catalogs/acme`, or a path to a `.json` file |

The other marketplace subcommands are `/plugin marketplace list`, `/plugin marketplace update [<name>]`, and `/plugin marketplace remove <name>`.

### Hot reload

cocode watches both plugin directories and notices changes within about 300 ms, but **it does not apply them automatically**. You get a notification reading "Plugins changed. Run /reload-plugins to activate." Running `/reload-plugins` rescans the plugin and skill directories and atomically swaps in the new command registry.

This differs deliberately from skills, which do reload on their own: activating a plugin can register hooks and MCP servers, so it waits for you to say so.

## Hooks

A hook runs your code at a defined point in the agent's lifecycle. Hooks can observe, inject context, or block an action outright.

### Event types

There are 27 events. The wire format is PascalCase, exactly as written here.

| Group | Events |
|-------|--------|
| Tool lifecycle | `PreToolUse`, `PostToolUse`, `PostToolUseFailure` |
| Session lifecycle | `SessionStart`, `SessionEnd`, `Setup`, `Stop`, `StopFailure` |
| Subagent lifecycle | `SubagentStart`, `SubagentStop` |
| User interaction | `UserPromptSubmit`, `PermissionRequest`, `PermissionDenied`, `Notification`, `Elicitation`, `ElicitationResult` |
| Compaction | `PreCompact`, `PostCompact` |
| Task lifecycle | `TeammateIdle`, `TaskCreated`, `TaskCompleted` |
| Config and environment | `ConfigChange`, `InstructionsLoaded`, `CwdChanged`, `FileChanged` |
| Worktree | `WorktreeCreate`, `WorktreeRemove` |

### Configuration

Hooks are configured under the `hooks` key in `settings.json`, keyed by event name. **Each array element is a hook definition directly** — the handler fields sit on the same object as the matcher.

```jsonc
// ~/.cocode/settings.json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "type": "command",
        "command": "~/.cocode/bin/audit-bash.sh",
        "timeout": 10
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Edit|Write",
        "type": "command",
        "command": "cargo fmt --quiet"
      }
    ]
  }
}
```

Hooks are read from user, project (`.cocode/settings.json`), local (`.cocode/settings.local.json`), and enterprise policy settings, plus any installed plugin. Duplicates are collapsed, and higher-precedence scopes win.

Per-hook keys:

| Key | Default | Meaning |
|-----|---------|---------|
| `type` | `"command"` | `command`, `prompt`, `http` (or `webhook`), or `agent`. |
| `matcher` | none | Which values to fire on. Omit to match everything. |
| `if` | none | Permission-rule condition, e.g. `"Bash(git *)"`. Narrower than `matcher`. |
| `timeout` | 30 s for commands | Seconds. Applied to the handler when it has no `timeout_ms` of its own. |
| `priority` | `0` | Lower runs first. |
| `once` | `false` | Fire at most once per session. |
| `async` | `false` | Do not block the event. |
| `status_message` | none | Text shown while the hook runs. |

Handler-specific keys are `command` and `shell` for `command`; `prompt` and `model` for `prompt` and `agent`; `url`, `headers`, and `allowed_env_vars` for `http`.

Matchers are matched as an exact string, a pipe-separated list, a regex, or a glob, in that order of preference. `"*"` matches any present value.

Two settings act as global switches: `"disable_all_hooks": true` turns everything off, and `"allow_managed_hooks_only": true` keeps only enterprise-policy and session-scoped hooks.

### What a hook receives

Command hooks are spawned through a shell with the event payload written to **stdin as one JSON object**. The `hook_event_name` field discriminates the event; the rest of the fields depend on it.

```json
{
  "hook_event_name": "PreToolUse",
  "session_id": "01J...",
  "cwd": "/home/you/project",
  "transcript_path": "/home/you/.cocode/projects/.../transcript.jsonl",
  "permission_mode": "default",
  "tool_name": "Bash",
  "tool_input": { "command": "rm -rf build" },
  "tool_use_id": "toolu_..."
}
```

`transcript_path` is an empty string when the session is not persisting a transcript. `agent_id` and `agent_type` appear only when the hook fires inside a subagent.

These environment variables are also set: `HOOK_EVENT`, `HOOK_SESSION_ID`, `HOOK_CWD`, `HOOK_TOOL_NAME` (when a tool is in play), and `CLAUDE_PROJECT_DIR`.

### How a hook's output is interpreted

**Exit codes** are the simple path:

| Exit code | Meaning |
|-----------|---------|
| `0` | Success. Stdout is injected into the conversation as additional context. |
| `2` | **Blocking error.** The action is blocked and stderr is fed back to the model as the reason. |
| anything else | Non-blocking error. Stderr is surfaced for display and audit but never reaches the model. |

**JSON on stdout** is the precise path, and it takes precedence over the exit code. If stdout starts with `{` and parses against the hook output schema, it drives the outcome regardless of how the process exited. If it starts with `{` but is not valid JSON, it is treated as plain text; if it is valid JSON of the wrong shape, it becomes a non-blocking error and is not injected as context.

Common fields:

```json
{
  "continue": true,
  "suppressOutput": false,
  "stopReason": "string, when continue is false",
  "decision": "approve | block",
  "reason": "why",
  "systemMessage": "injected as a system message"
}
```

Event-specific fields go under `hookSpecificOutput`, tagged with `hookEventName`. If that name does not match the firing event, the nested fields are ignored with a warning. For `PreToolUse` the fields are `permissionDecision` (`allow`, `deny`, or `ask`), `permissionDecisionReason`, `updatedInput`, and `additionalContext`. Other events carry `additionalContext`, `watchPaths` (`SessionStart`, `CwdChanged`, `FileChanged`), `updatedMCPToolOutput` (`PostToolUse`), and `action`/`content` (`Elicitation`, `ElicitationResult`).

### A complete example

This `PreToolUse` hook blocks `rm -rf` outside the project directory and lets everything else through.

Create `~/.cocode/bin/guard-rm.sh` and make it executable (`chmod +x`):

```bash
#!/usr/bin/env bash
set -euo pipefail

payload=$(cat)
command=$(printf '%s' "$payload" | jq -r '.tool_input.command // ""')

if printf '%s' "$command" | grep -qE '\brm\b.*-[a-zA-Z]*r'; then
  if printf '%s' "$command" | grep -qE '(^|[[:space:]])/([[:space:]]|$)|\.\./'; then
    jq -n '{
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
        permissionDecisionReason: "Recursive rm outside the project directory is blocked by policy."
      }
    }'
    exit 0
  fi
fi

exit 0
```

Register it:

```jsonc
// ~/.cocode/settings.json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "type": "command",
        "command": "~/.cocode/bin/guard-rm.sh",
        "timeout": 5,
        "status_message": "Checking command safety"
      }
    ]
  }
}
```

The hook emits JSON, so it exits `0` and lets the JSON carry the decision. Exiting `2` with a message on stderr would have the same blocking effect with less control over the reason.

### Other handler types

`prompt` and `agent` hooks evaluate a natural-language condition with a model rather than running a process, and return a structured verdict — useful when the rule is fuzzy ("does this commit message follow our convention?").

`http` hooks POST the same JSON payload to a URL. The method is always POST. Requests are guarded against SSRF: private and link-local addresses are blocked, loopback is allowed. Environment variables are interpolated into headers only if you name them in the hook's `allowed_env_vars` list; any other `$VAR` reference resolves to empty.

### Inspecting hooks

`/hooks` lists configured hooks grouped by event. `/hooks reload` rebuilds the live registry from current settings without restarting the session.

> Note: the example printed by `/hooks` when no hooks are configured shows a nested `"hooks": [...]` array inside the matcher object. That shape does not parse. Use the flat shape documented above.

For SDK clients, `--include-hook-events` emits `HookStarted`, `HookProgress`, and `HookResponse` into the stream-json output. It is off by default.
