# coco-subagent

Pure-logic subagent rules: definition catalog, source precedence, AgentTool
prompt rendering, tool filter planning, validation diagnostics.

## Key Types

| Type | Purpose |
|------|---------|
| `AgentDefinitionStore` | Loads built-ins + per-source markdown agents; exposes a snapshot |
| `AgentCatalogSnapshot` | Immutable per-turn view of active / all definitions; returned as `Arc<...>` for cheap sharing |
| `AgentLoadReport` | Diagnostics from the most recent load |
| `BuiltinAgentCatalog` | Toggle set for optional built-ins (Explore/Plan, verification, coco-guide, SDK disable) |
| `AgentToolPromptRenderer` | AgentTool prompt string builder |
| `AgentToolFilter` + `ToolFilterPlan` | Pure tool filter computation; `app/state` applies the plan to the child `ToolRegistry` |
| `AllowedAgentTypes` + `parse_allowed_agent_types` | Parse `Agent(...)` / `Task(...)` permission entries |
| `AgentDefinitionValidator` | Structural validation (required `name` / `description`) |
| `parse_agent_markdown` | Frontmatter → `AgentDefinition` (camelCase + snake_case keys) |

Constants: `ONE_SHOT_BUILTIN_AGENT_TYPES = ["Explore", "Plan"]` (case-sensitive),
`EMPTY_AGENT_OUTPUT_MARKER = "(Subagent completed but returned no output.)"`.

## Conventions

- **Canonical case is contract.** `Explore` and `Plan` are PascalCase
  everywhere — output, lookup, the one-shot set. `general-purpose`,
  `statusline-setup`, `verification`, `coco-guide` are kebab-case
  lowercase. Aliases like `explore` only exist on input parsing; serialization
  always emits canonical case.
- **Source precedence (later wins):** `built-in < plugin < userSettings <
  projectSettings < flagSettings < policySettings`. Same `agent_type` from a
  higher source overrides lower.
- **Snapshots are deterministic.** `AgentCatalogSnapshot` keys by `def.name`
  (= TS `agentType` = `frontmatter['name']`; for built-ins this equals the
  canonical `agent_type`). Every model-facing string (listing, deny filter,
  `find_active` lookup, error surfaces) keys on `def.name` so they stay
  consistent for aliased custom agents. Iteration is alphabetical for stable
  prompt rendering.

## Layer Rule (DO NOT BREAK)

This crate is **pure logic**. Its own `Cargo.toml` must NOT add:

- `tokio`, `tokio-util`, `mpsc`, watcher infrastructure
- `coco-tool`, `coco-tools` — would invert the thin AgentTool boundary
- `coco-query`, `coco-state`, `coco-commands` — those consume the catalog,
  not the other way round

Filesystem access is sync `std::fs` triggered by `AgentDefinitionStore::load()`
/ `reload()`. The watcher that calls `reload()` lives in `app/state`.

**Caveat — transitive tokio:** `cargo tree -p coco-subagent` shows tokio in
the graph because `coco-types` (a required dep) depends on tokio for
`AppStateReadHandle`. The crate itself uses no tokio APIs and adds none of
its own; the transitive pull is structural and predates this crate. Cleanly
removing tokio from the graph requires splitting `AppStateReadHandle` out of
`coco-types`, tracked separately. Do not add tokio APIs here in the meantime.

## Known limitations

- **`extra_allow_list`** on `ToolFilterContext` is a coco-rs extension,
  reserved for slash-command tool intersection. Pass `None` for default behavior.
- **`coco-guide` agent — dynamic context sections deferred**. The agent
  should append a session-specific block (custom skills / agents / MCP servers /
  plugin commands / settings.json snapshot) to the static base prompt at spawn
  time. coco-rs currently emits only the static base — see
  `builtin_prompts::coco_guide_system_prompt` doc comment. Wiring the dynamic
  block belongs on the spawn-time prompt assembler in `coordinator::agent_handle`,
  not the catalog entry.
- **Built-in `whenToUse` and `system_prompt` strings** — tool-name placeholders
  (`${BASH_TOOL_NAME}` etc.) are resolved via [`coco_types::ToolName`] so a future
  rename flows through automatically. Snapshot tests in `builtin_prompts.test.rs`
  enforce the output strings.

### Resolved gaps (history)

The following entries appeared in earlier audits but verification confirmed
they are NOT gaps in the current code:

- ~~No consumer wiring~~ — `AgentTool`, `app/state`, `commands` all consume
  the catalog. Legacy `agent_spawn.rs`/`agent_advanced.rs` are gone (one
  comment reference in `app/cli/src/paths.rs`, no active code).
- ~~No nested-directory walking~~ — `collect_md_paths` walks 2 levels
  (root + 1 nested, depth=2). 1 MiB size cap is enforced via the existing
  metadata check.
- ~~`mcpServers` inline form not parsed~~ — `parse_mcp_servers`
  (`frontmatter.rs:216-259`) handles all three shapes: string list, mixed
  sequence, pure inline mapping list.
- ~~Wildcard `tools: ['*']` skips memory injection~~ — both "missing field"
  AND `['*']` collapse to empty Vec, and the inject site gates on
  `if def.allowed_tools.is_empty() { return; }` in `inject_memory_tools`.
