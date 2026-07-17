# coco-subagent

Pure-logic subagent rules: definition catalog, source precedence, AgentTool
prompt rendering, tool filter planning, validation diagnostics.

## Key Types

| Type | Purpose |
|------|---------|
| `AgentDefinitionStore` | Loads built-ins + per-source markdown agents; exposes a snapshot |
| `AgentCatalogSnapshot` | Immutable per-turn view of active / all definitions; returned as `Arc<...>` for cheap sharing |
| `AgentLoadReport` | Diagnostics from the most recent load |
| `BuiltinAgentCatalog` | Toggle set for optional built-ins (Explore/Plan, verification, coco-guide, noninteractive disable) |
| `AgentToolPromptRenderer` | AgentTool prompt string builder |
| `AgentToolFilter` + `ToolFilterPlan` | Pure tool filter computation; host/coordinator assembly applies the plan to the child `ToolRegistry` |
| `AllowedAgentTypes` + `parse_allowed_agent_types` | Parse `Agent(...)` / `Task(...)` permission entries |
| `AgentDefinitionValidator` | Structural validation (required `name` / `description`) |
| `parse_agent_markdown` | Frontmatter → `AgentDefinition` (camelCase + snake_case keys) |

Constants: `ONE_SHOT_BUILTIN_AGENT_TYPES = ["Explore", "Plan"]` (case-sensitive),
`EMPTY_AGENT_OUTPUT_MARKER = "(Subagent completed but returned no output.)"`.

## Conventions

- **Canonical case is contract.** `Explore` and `Plan` are PascalCase
  everywhere — output, lookup, the one-shot set. `general-purpose`,
  `statusline-setup`, `verification`, `coco-guide` are kebab-case lowercase.
  Aliases like `explore` exist only on input parsing; serialization always
  emits canonical case.
- **Source precedence (later wins):** `built-in < plugin < userSettings <
  projectSettings < flagSettings < policySettings`. Same `agent_type` from a
  higher source overrides lower.
- **Snapshots are deterministic.** `AgentCatalogSnapshot` keys by `def.name`
  (= `frontmatter['name']`; for built-ins = the canonical `agent_type`).
  Every model-facing string (listing, deny filter, `find_active` lookup,
  error surfaces) keys on `def.name` so aliased custom agents stay
  consistent. Iteration is alphabetical for stable prompt rendering.

## Layer Rule (DO NOT BREAK)

This crate is **pure logic**. Its own `Cargo.toml` must NOT add:

- `tokio`, `tokio-util`, `mpsc`, watcher infrastructure
- `coco-tool`, `coco-tools` — would invert the thin AgentTool boundary
- `coco-query`, `coco-agent-host`, `coco-commands` — those consume the catalog,
  not the other way round

Filesystem access is sync `std::fs` triggered by `AgentDefinitionStore::load()`
/ `reload()`. Reload orchestration lives in `coco-agent-host`.

**Caveat — transitive tokio:** `cargo tree -p coco-subagent` shows tokio in
the graph because `coco-types` (a required dep) pulls it for
`AppStateReadHandle`. The crate itself uses no tokio APIs and adds none of
its own; cleanly removing it requires splitting `AppStateReadHandle` out of
`coco-types` (tracked separately). Do not add tokio APIs here in the meantime.

## Known limitations

- **`extra_allow_list`** on `ToolFilterContext` is a coco-rs extension,
  reserved for slash-command tool intersection. Pass `None` for default behavior.
- **`coco-guide` agent — dynamic context sections deferred**: only the static
  base prompt is emitted (see `builtin_prompts::coco_guide_system_prompt` doc
  comment). The session-specific block (custom skills / agents / MCP servers /
  plugin commands / settings snapshot) belongs on the spawn-time prompt
  assembler in `coordinator::agent_handle`, not the catalog entry.
- **Built-in `whenToUse` and `system_prompt` strings** — tool-name placeholders
  (`${BASH_TOOL_NAME}` etc.) are resolved via [`coco_types::ToolName`] so a future
  rename flows through automatically. Snapshot tests in `builtin_prompts.test.rs`
  enforce the output strings.
