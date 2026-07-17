# Configuration

How cocode finds its configuration, in what order the layers merge, which feature gates exist and what they default to, and which environment variables you are likely to need. Everything here describes the `coco` binary's actual behavior; see the [CLI reference](cli-reference.md) for the flags that feed into it.

## Config home

cocode keeps machine-level state in a config home directory. By default that is `~/.cocode/`. Setting the environment variable `$COCO_CONFIG_DIR` relocates it to any absolute path you choose, which is the supported way to isolate a test rig, run several profiles side by side, or move state off a home directory:

```bash
export COCO_CONFIG_DIR=/srv/cocode-profiles/staging
```

An empty `$COCO_CONFIG_DIR` is treated as unset and falls back to `~/.cocode/`.

The config home holds the following. Not all of it exists on a fresh install — most entries are created on first use.

| Path | What it is |
|------|-----------|
| `settings.json` | Your user-level settings. This is the file you edit most. |
| `providers.json` | Provider catalog. See [providers and authentication](providers-and-auth.md). |
| `models.json` | Provider-agnostic model catalog. See [models and MoA](models-and-moa.md). |
| `keybindings.json` | Keyboard shortcut overrides for the terminal UI. |
| `theme.json` | Terminal UI theme. |
| `auth/` | Stored provider credentials, one JSON file per provider, mode `0600`. Only used when the credential store resolves to the file backend. |
| `agents/` | Your user-level agent definitions as markdown files with YAML frontmatter. |
| `logs/` | Rotating log files. See [Logging](#logging) below. |
| `plans/` | Saved plans from plan mode. Relocatable per project via the `plans_directory` setting, which must resolve inside the project root. |
| `teams/` | Agent team state and mailboxes. Relocatable via `$COCO_TEAMS_DIR`. |
| `sessions/` | Session transcripts and the live PID registry that backs `coco ps`. |
| `plugins/` | Installed plugins and the marketplace cache. |
| `output-styles/` | Custom output style definitions. |
| `tasks/` | Durable task-list state. |
| `hub/` | Embedded Event Hub database. Only present when running `--serve-hub`. |

### The global config file is not in the config home

This is the one path in cocode that reliably surprises people, so it is worth stating plainly.

There are two distinct things with confusingly similar names:

- **User settings** live at `<config home>/settings.json` — inside the directory, as you would expect.
- **Global config** is a *separate* file holding per-user, per-machine state: onboarding status, theme name, per-project session pointers, cost tracking, and the credential-store choice.

The global config file's location depends on whether `$COCO_CONFIG_DIR` is set, and the two cases do not agree:

| `$COCO_CONFIG_DIR` | Global config path |
|--------------------|--------------------|
| Unset (default) | `~/.cocode.json` — **a file next to the directory, not inside it** |
| Set | `$COCO_CONFIG_DIR/global.json` — inside the directory |

So on a default install you have both `~/.cocode/` (a directory) and `~/.cocode.json` (a file), and they are different things. The file is easy to miss when you copy or back up your config, and easy to delete by accident when clearing out `~/.cocode`. If you set `$COCO_CONFIG_DIR`, the file moves inside and is renamed to `global.json` — so a config home populated under `$COCO_CONFIG_DIR` is not layout-compatible with a default one.

You rarely need to edit the global config by hand. Note that it is where `auth_credential_store` lives, deliberately: it is a user-and-machine decision, and keeping it out of `settings.json` means no project's settings file can quietly downgrade your credential storage.

## Project configuration

Per-project configuration lives in a `.cocode/` directory at your project root — not `.claude/`. Running `coco init` creates it with an empty `settings.json`.

| Path | What it is |
|------|-----------|
| `.cocode/settings.json` | Project settings. Intended to be committed and shared with your team. |
| `.cocode/settings.local.json` | Your personal overrides for this project. **Add this to `.gitignore`.** |
| `.cocode/rules/` | Markdown rule files injected into the agent's context. Files without a `paths:` frontmatter key load unconditionally; files with one load lazily when a matching file is touched. |
| `.cocode/agents/` | Project-scoped agent definitions. |

Add the local file to your `.gitignore`, since it is the layer meant for machine-specific and personal settings:

```gitignore
.cocode/settings.local.json
```

Committing `.cocode/settings.json` while gitignoring `.cocode/settings.local.json` gives you the intended split: shared team defaults in version control, personal deviations local to your checkout.

## Settings merge order

Settings come from six ranked layers. They merge in this order, with each layer overriding the ones before it:

1. **Plugin** — plugin-contributed configuration. Lowest priority.
2. **User** — `<config home>/settings.json`.
3. **Project** — `.cocode/settings.json`.
4. **Local** — `.cocode/settings.local.json`.
5. **Flag** — the file passed to `--settings <path>`, or settings supplied inline by an SDK client.
6. **Policy** — enterprise/MDM managed settings. Highest priority; nothing overrides it.

Four of these are files you can edit: User, Project, Local, and Policy, plus whatever you point `--settings` at. The Plugin tier is the ranking floor and is where plugin-contributed hooks and permission rules are attributed; installed plugins do not currently contribute a `settings.json` layer of their own, so in practice the merge starts at User.

A missing file at any layer is simply skipped. The result is a single merged `Settings` object, but cocode also tracks which layer each individual setting came from, and it uses that provenance to enforce security rules — for example, a project's settings file is not permitted to configure `api_key_helper`, auto-mode behavior, bypass mode, or MCP server approval (`enable_all_project_mcp_servers` / `allowed_mcp_servers`), because those would let a checked-out repository escalate its own privileges. The one deliberate inversion is `denied_mcp_servers`, which is honored from every layer — a deny only narrows what can run.

### Restricting layers with `--setting-sources`

`--setting-sources` takes a comma-separated list of layer names and restricts which layers participate:

```bash
coco --setting-sources project,local -p "..."
```

Valid tokens are `user`, `project`, `local`, `flag`, and `policy`. Unknown tokens are silently ignored. When the flag is omitted entirely, `user`, `project`, and `local` all load.

**Flag and Policy always load and cannot be disabled.** They are added to the enabled set unconditionally, whatever you pass. This is deliberate: Policy is administrator-controlled and must not be evadable from the command line, and Flag is something you supplied on that same command line, so disabling it would be incoherent. Listing `flag` or `policy` in the value is therefore redundant but harmless. Passing an empty string does not disable everything — it disables `user`, `project`, and `local`, and leaves Flag and Policy in place.

The practical use is CI, where you want a build to honor the repository's checked-in settings without picking up whatever the build agent's home directory happens to contain:

```bash
coco --setting-sources project -p "Run the tests and summarize failures"
```

### Settings files are JSONC

Settings files are parsed as JSONC, so comments and trailing commas are both accepted. Use them — a settings file that explains itself is worth the two extra characters:

```jsonc
// ~/.cocode/settings.json
{
  // Bind the Main role to a specific provider and model.
  "models": {
    "main": "anthropic/claude-sonnet-4-6",
  },
  "features": {
    // Off by default; we want sandboxed shell execution here.
    "sandbox": true,
  },
}
```

One naming gotcha: the top-level key is `models.main`, and a bare `model` key is **rejected** with an explicit error rather than ignored. Unknown top-level keys are rejected the same way, so a typo fails loudly at startup instead of silently doing nothing.

### Reload semantics

Settings are read into memory when a session starts. Writes that happen afterward do not retroactively change the running session.

This matters most for slash commands that persist a setting — `/voice` writing `features.voice`, for instance. Those commands write to disk correctly, but **the live runtime keeps its existing in-memory settings**. The change takes effect when you start a new session, or when the settings file watcher's debounce fires and reloads. If you toggle a setting via a slash command and nothing appears to happen, that is why; restart the session.

Editing a settings file by hand while cocode is running has the same property: the file watcher may pick it up, but do not count on an immediate effect within the current session.

## Feature gates

A feature gate is a **coarse capability switch** for a whole subsystem, not a knob for tuning one. Each gate turns an entire capability on or off — the sandbox, retrieval, voice input, agent teams. Finer-grained settings are not features; they live inside their subsystem's own config section. For example, whether the sandbox runs at all is the `sandbox` feature, but which sandbox mode it uses is `sandbox.mode` in settings. Do not go looking for a feature key to tune a sub-behavior; it will not be there by design.

Feature keys are set under a `features` object in settings:

```jsonc
{
  "features": {
    "sandbox": true,
    "retrieval": true,
    // Turn off a default-on feature to reclaim its token budget.
    "web_search": false,
  },
}
```

Each feature carries a lifecycle stage. **Stable** features are production-ready. **Experimental** features are user-facing and appear in the `/experimental` menu. **Under development** features are not shown in menus and are not announced; they work, but they are not finished, and you should expect rough edges. Stage is independent of the default: several Stable features default to off for risk or cost reasons, and that is intentional rather than an oversight.

<!-- BEGIN GENERATED: features -->

| Key | Stage | Default | What it does |
|-----|-------|---------|--------------|
| `web_search` | Stable | on | Exposes the `web_search` tool to the model. |
| `web_fetch` | Stable | on | Exposes the `web_fetch` tool to the model. |
| `mcp` | Stable | on | Exposes MCP management tools and dynamic MCP server tool wrappers to the model. |
| `mcp_skills` | Under development | off | Discovers skills published by connected MCP servers and surfaces them as skills and slash commands. Requires `mcp`. |
| `notebook_edit` | Stable | off | Exposes the `notebook_edit` tool to the model. |
| `task_v2` | Stable | on | V2 task tooling (`TaskCreate` / `TaskGet` / `TaskList` / `TaskUpdate`). When off, the V1 `TodoWrite` tool is exposed instead. |
| `tool_search` | Stable | on | Lazy tool-schema loading via the `ToolSearch` tool. Deferrable tools are sent name-only on the first turn and discovered on demand, saving a large share of the tools-array token budget. When off, every enabled tool ships its full schema in every request. |
| `dynamic_model_card` | Stable | on | Refreshes the model-card catalog from OpenRouter in a non-blocking startup task. The bundled snapshot remains the fallback. |
| `output_rewrite` | Stable | on | Compresses Bash dev-tool output (git, cargo, test runners, linters, docker) before it reaches the model. Permission rules and sandbox decisions always evaluate the original command. Silently no-ops with no backend available. |
| `sandbox` | Stable | off | Runs shell commands inside a sandbox. Default off for risk-conservatism, not immaturity. See [sandbox](sandbox.md). |
| `plan_mode` | Stable | on | Plan-mode subsystem: the `EnterPlanMode` / `ExitPlanMode` tools and the plan-mode context reminder. Turn off to reclaim the reminder tokens and tool schema. |
| `workflow` | Stable | on | Dynamic local workflow scripts. |
| `auto_memory` | Under development | off | Auto-memory subsystem: extraction, team sync, and relevant-memory injection. |
| `skill_learning` | Under development | off | Autonomous skill-learning loop that distills sessions into agent-owned skills, plus the periodic curator. Off by default because it auto-writes executable artifacts. |
| `retrieval` | Under development | off | Retrieval subsystem: BM25, vector, AST, RepoMap, and reranker. |
| `agent_teams` | **Experimental** | off | Persistent agent teams and teammate orchestration: spawn addressable teammates and coordinate via `SendMessage`. |
| `worktree` | Under development | off | Worktree tools (`EnterWorktree` / `ExitWorktree`). |
| `lsp` | Under development | off | LSP-backed code intelligence tool. |
| `voice` | Under development | off | Voice input (speech-to-text dictation): microphone capture and STT, surfaced through `/voice` and `/voice-config`. Off by default because microphone access and outbound audio to a third party are privacy- and cost-sensitive. |
| `proactive` | Under development | off | Autonomous, tick-driven assistant loop helpers. |
| `kairos_brief` | Under development | off | Brief user-message channel (`SendUserMessage`). |
| `agent_triggers` | Stable | on | Local scheduling tools (`Cron*`, `ScheduleWakeup`, `Monitor`) and the `/loop` skill. |
| `agent_triggers_remote` | Under development | off | The `/schedule` skill for remote agent scheduling. |
| `building_claude_apps` | Under development | off | The `/claude-api` skill. |
| `kairos_dream` | Under development | off | The `/dream` skill for memory consolidation. |
| `review_artifact` | Under development | off | The `/hunter` bug-finding review skill. |
| `run_skill_generator` | Under development | off | The `/run-skill-generator` skill. |
| `tool_use_summary` | Under development | off | Short label emitted after each tool batch via an extra Fast-role call. Off by default: it costs a call per tool-using turn and degrades to nothing on reasoning-class Fast models. |
| `claude_in_chrome` | Under development | off | Auto-detects a Claude in Chrome installation. |
| `new_init` | Under development | off | The newer multi-phase `/init` prompt instead of the single-prompt version. |
| `reactive_compact` | Under development | off | Reactive compaction strategy instead of summarize-all. |
| `prompt_cache_break_detection` | Under development | off | Prompt-cache break detection wiring during compaction. |
| `speculation` | **Experimental** | off | Pre-executes accepted prompt suggestions in an overlay sandbox and injects the result instantly on accept. |

<!-- END GENERATED: features -->

### Toggle precedence

A feature's effective value is resolved once, by applying four sources in order. Each one overrides the last:

1. **Registry defaults** — the Default column above.
2. **`settings.json`** — the `features` object, after the full settings merge described earlier. So a Policy-layer `features` block beats a user one, by the normal layer rules.
3. **Environment** — `COCO_FEATURE_<KEY>` variables.
4. **Programmatic overrides** — set by an embedding host or SDK client.

Unknown feature keys are silently ignored at every layer, so a typo in a `features` block leaves the default in place rather than erroring.

### The `COCO_FEATURE_*` environment form

Any feature can be toggled from the environment by prefixing its key with `COCO_FEATURE_`. The matching rule is worth being precise about: cocode takes everything after the `COCO_FEATURE_` prefix, **lowercases it**, and matches that against the feature key. Underscores in the key are preserved as-is.

Because the remainder is lowercased, the conventional shouty form and the literal-key form both work and are equivalent:

```bash
COCO_FEATURE_SANDBOX=1 coco          # remainder "SANDBOX" -> "sandbox"
COCO_FEATURE_sandbox=1 coco          # remainder "sandbox" -> "sandbox"
COCO_FEATURE_AUTO_MEMORY=1 coco      # remainder "AUTO_MEMORY" -> "auto_memory"
COCO_FEATURE_TOOL_SEARCH=0 coco      # turns a default-on feature off
```

The value is parsed as a boolean. A value that parses as neither true nor false causes the variable to be ignored entirely, falling through to the settings layer rather than guessing.

## Managed and enterprise settings

Managed settings are the Policy layer: the highest-priority source, intended for administrators and MDM deployment. The file is read-only from cocode's perspective and cannot be disabled by `--setting-sources`.

| OS | Managed settings path |
|----|----------------------|
| macOS | `/Library/Application Support/cocode/managed-settings.json` |
| Linux | `/etc/cocode/managed-settings.json` |
| Windows | `C:\Program Files\cocode\managed-settings.json` |

Drop-in files are also loaded at Policy priority, which lets you compose policy from several files rather than maintaining one large one. They go in a `managed-settings.d/` directory next to the managed settings file — for example `/etc/cocode/managed-settings.d/` on Linux. Only `*.json` files are read, and they merge in sorted filename order after `managed-settings.json` itself.

### Managed memory uses a different directory

Managed *memory* — policy-level `CLAUDE.md` / `AGENTS.md` files and an unconditional rules directory, injected into every session's context at the lowest attention tier — does **not** live under the `cocode` path above. It is read from:

```
/etc/coco/CLAUDE.md
/etc/coco/AGENTS.md
/etc/coco/rules/
```

This is `/etc/coco`, not `/etc/cocode`, and it is hardcoded to that literal path: it does not follow the product name, does not respond to `$COCO_CONFIG_DIR`, and has no macOS or Windows equivalent. On non-Linux hosts no managed memory loads at all. If you are deploying policy memory, use `/etc/coco/` and expect it to apply on Linux only. If you are deploying policy *settings*, use the per-OS `cocode` paths in the table above. The two are unrelated directories despite serving adjacent purposes.

## Environment variables

cocode owns a large number of `COCO_*` variables; most are internal tuning knobs. These are the ones worth knowing about.

| Variable | Effect |
|----------|--------|
| `COCO_CONFIG_DIR` | Relocates the config home. Also changes the global config file to `$COCO_CONFIG_DIR/global.json` — see [above](#the-global-config-file-is-not-in-the-config-home). |
| `COCO_FEATURE_<KEY>` | Toggles one feature gate. See [the env form](#the-coco_feature_-environment-form). |
| `COCO_BARE_MODE` | Truthy value skips session-start and per-turn background housekeeping: auto-dream, memory extraction, prompt suggestion, and stale-directory sweeps. Equivalent to the `--bare` flag, which sets this variable for the process. |
| `COCO_MODEL_MAIN` | Overrides the Main model role. Deliberately environment-only — it is the escape hatch that must work before `settings.json` is parsed. Per-role models go through `settings.models.*`. |
| `COCO_AUTH_CREDENTIAL_STORE` | Credential backend: `auto` (keychain first, file fallback), `file` (only `<config home>/auth/*.json`, mode `0600`), `keyring` (keychain only, error if unavailable), or `ephemeral` (in-memory, nothing persists). Case-insensitive. An unrecognized value logs a warning and falls through to `global.json`'s `auth_credential_store`, then to a build-provenance default. |
| `COCO_EVENT_HUB_URL` | Event Hub WebSocket endpoint. Must start with `ws://` or `wss://` or startup fails. Overridden by `--event-hub-url`; overrides `settings.event_hub_url`. |
| `COCO_TEAMS_DIR` | Relocates the teams and mailbox tree. Default `<config home>/teams/`. |
| `COCO_MEMORY_PATH_OVERRIDE` | Full-path override for the auto-memory directory, replacing the computed per-project path. Intended for deployments where the working directory varies per session and would otherwise produce a different project key each time. |
| `COCO_LOG` | Tracing filter directive. See [Logging](#logging). |
| `COCO_LOG_FORMAT` | `pretty`, `compact`, or `json`. |
| `COCO_LOG_FILE` | Overrides the log file path. |
| `COCO_LOG_STDERR` | Truthy value adds a stderr log layer. |
| `COCO_LOG_LOCATION` | Truthy or falsy; forces source `file:line` and thread names on or off. |
| `COCO_LOG_TIMEZONE` | `local` or `utc` for log timestamps. |

### Logging

The log filter resolves through this chain, first match winning: `--log-level`, then `COCO_LOG`, then `RUST_LOG`, then `settings.log.level`, then the built-in default of `coco=debug,info`. `RUST_LOG` is honored because it is the tracing ecosystem's convention, and it sits above `settings.log.level` because it is still an environment override.

A bare level like `debug` expands to `coco=debug,debug`, keeping cocode's own crates verbose without flooding you with third-party output. Anything that is not a bare level is passed through as a full `EnvFilter` directive.

Each `--log-*` flag has a `COCO_LOG_*` counterpart at lower priority, which in turn sits above the corresponding `settings.log.*` key. So for any log knob the order is flag, then environment, then settings.

Log files go to `<config home>/logs/` and are **per process**: the filename carries the PID, in the form `coco.<pid>.log.<date>`. This keeps concurrent sessions from interleaving into one file, at the cost of accumulating files, so cocode sweeps anything in that directory older than seven days on startup. Because the sweep is by modification time and the directory is cocode-owned, do not keep anything else in it.

Stdout is reserved in the TUI (the rendered screen) and in SDK mode (NDJSON), so logs never land there. In headless mode cocode prints the resolved log path to stderr on startup so a piped run can find its own file among concurrent sessions.

Two more details worth knowing: source location and thread names auto-enable when the resolved filter turns on `debug` or `trace` for a `coco*` target, unless you explicitly set `--log-location` or `COCO_LOG_LOCATION` either way. And subcommands that run in Skip mode never install a logging subscriber at all, which is what keeps their stdout clean enough to pipe into `jq`.

## See also

- [CLI reference](cli-reference.md) — the flags that feed this configuration.
- [Providers and authentication](providers-and-auth.md) — `providers.json`, credentials, and the `auth/` directory.
- [Models and MoA](models-and-moa.md) — `models.json`, model roles, and Mixture-of-Agents presets.
- [Permissions](permissions.md) — permission modes and rules.
- [Sandbox](sandbox.md) — the `sandbox` feature and its modes.
