# ToolSearch — TS-parity port

`ToolSearch` is the bounded lazy schema loader. The model calls it with a
query — `select:Name1,Name2` or `+required keyword optional` — and receives
only complete schemas that fit the per-entry and aggregate budgets. Matches
come from the exact immutable `ToolMaterialization` used to build the current
provider request; canonical `ToolId` / `WireToolName` identity prevents equal
bare names on different MCP servers from colliding.

For the MCP exposure policy, `use_tool` invocation, and dual provider/semantic
identity rules, see [`mcp-tool-exposure-design.md`](mcp-tool-exposure-design.md).

This doc captures the **TS-parity port** in `coco-rs`. TS source:
`tools/ToolSearchTool/{ToolSearchTool,prompt,constants}.ts` and
`utils/toolSearch.ts` in
[`claude-code-kim`](https://github.com/anthropics/claude-code-kim).

## Multi-provider divergence

TS routes matched names through Anthropic's `tool_reference` expansion.
coco-rs supports three schema-discovery transports plus a provider-neutral
`use_tool` floor:

- Anthropic `tool_reference` expansion;
- OpenAI Responses client-executed native `tool_search`;
- client-side promotion through the cross-turn discovery set;
- bounded client JSON followed by `use_tool` for `UseTool` placement.

Only the client-promotion transport mutates the shared discovery set:

```rust
// common/types/src/app_state.rs
pub struct ToolAppState {
    ...
    pub discovered_tool_names: std::collections::HashSet<String>,
}
```

The `ToolUseContext` carries an `Arc<HashSet<String>>` snapshot of this
set for the current turn:

```rust
// core/tool-runtime/src/context.rs
pub struct ToolUseContext {
    ...
    pub discovered_tool_names: Arc<HashSet<String>>,
}
```

`ToolMaterialization` upgrades a `Deferred` tool whose canonical name is in
`ctx.discovered_tool_names` into `Loaded` placement. MCP `alwaysLoad` may also
produce `Loaded`, but only under server exposure `defer`. `UseTool` targets are
never promoted:

```rust
// Conceptual placement rule in core/tool-runtime/src/registry.rs
Deferred if tool.should_defer()
         && !tool.always_load()
         && !ctx.discovered_tool_names.contains(canonical_name)
```

## Promotion flow

```
Turn N
  ┌───────────────────────────────────┐
  │ model: ToolSearch{select:WebFetch}│
  └─────────────────┬─────────────────┘
                    │
                    ▼
  ┌──────────────────────────────────────────┐
  │ ToolSearchTool::execute                  │
  │   • resolve names against deferred pool  │
  │   • emit AppStatePatch:                  │
  │       discovered_tool_names += {WebFetch}│
  └─────────────────┬────────────────────────┘
                    │ executor applies patch
                    ▼
Turn N+1
  ┌──────────────────────────────────────────┐
  │ engine_prompt::build_tool_definitions    │
  │   • stub_for_filtering                   │
  │     .with_discovered_tool_names(set)     │
  │   • loaded_tools includes WebFetch       │
  │   • WebFetch's full schema → request     │
  └──────────────────────────────────────────┘
  ┌──────────────────────────────────────────┐
  │ engine_turn_reminders                    │
  │   • deferred = registry.deferred(ctx)    │
  │   • loaded   = registry.loaded(ctx)      │
  │   • compute_tools_delta(deferred,        │
  │       loaded, last_announced) → None     │
  │     (WebFetch left deferred but is in    │
  │      loaded — silent move per TS)        │
  └──────────────────────────────────────────┘
```

`compute_tools_delta` follows TS `getDeferredToolsDelta` semantics:

- `added = current_deferred - last_announced` — newly searchable names
  (e.g., MCP server just connected, or first turn after session start).
- `removed = last_announced - (current_deferred ∪ current_loaded)` —
  names gone entirely (MCP disconnect). A name that moves
  deferred → loaded (model discovered it) **stays silently** in the
  announced pool; the schema in the next request is the announcement.

## Query DSL (TS-parity)

`ToolSearchTool::execute` mirrors TS `ToolSearchTool.ts:186-406`:

1. **`select:Name1,Name2,...`** — case-insensitive prefix; comma-
   separated names; whitespace tolerant; missing names silently
   dropped; names resolving in the loaded pool (not deferred) return
   harmlessly to avoid retry churn.
2. **Keyword search** — anything else. Three fast paths run first:
   - Exact name match → return that single tool.
   - `mcp__<server>` prefix → return up to `max_results` MCP tools
     whose qualified name starts with the query.
   - Otherwise: tokenize, weighted scoring.
3. **`+keyword`** — required term. Candidates must match ALL `+terms`
   in `parsed.parts`, description, or `search_hint`. Other tokens
   contribute to scoring without filtering.

### Scoring (per term, per candidate)

| Match | Score (regular) | Score (MCP) |
|-------|-----------------|-------------|
| `parsed.parts` exact element | +10 | +12 |
| substring of any `parsed.parts` | +5 | +6 |
| `parsed.full.contains(term)` (fallback, only if score still 0) | +3 | +3 |
| `search_hint` word-boundary regex match | +4 | +4 |
| description word-boundary regex match | +2 | +2 |

`parsed = parse_tool_name(name)`:

- MCP (`mcp__server__action_subaction`) → split prefix-stripped name on
  `__`, then each part on `_`; `is_mcp = true`.
- Regular (`CamelCase` / `snake_case`) → split on `[a-z][A-Z]`
  boundaries and `_`, lowercased; `is_mcp = false`.

## Deferred tool catalog

Tools that override `should_defer() -> true` (and a matching
`search_hint`):

| Category | Tools |
|----------|-------|
| Web | `WebFetch`, `WebSearch` |
| Notebook | `NotebookEdit` |
| Tasks | `TaskOutput` |
| Swarm | `SendMessage`, `TeamCreate`, `TeamDelete` |
| Scheduling | `CronCreate`, `CronDelete`, `CronList`, `RemoteTrigger` |
| Worktree | `EnterWorktree`, `ExitWorktree` |
| Plan mode | `EnterPlanMode`, `ExitPlanMode` |
| Settings | `Config`, `LSP` |
| MCP | `ListMcpResources`, `ReadMcpResource`, all `McpTool` instances |
| Shell (internal) | `PowerShell`, `Sleep`, `SyntheticOutput` |

Plus every MCP tool: `McpTool::should_defer() = true` mirrors TS
`Tool.isMcp = true`. The `anthropic/alwaysLoad` and provider-neutral
`alwaysLoad` `_meta` opt-outs are parsed into `McpToolAnnotations` and apply
only to `defer` exposure; they cannot override explicit `use_tool`.

**Eager-loaded divergence:** `TaskCreate`, `TaskGet`, `TaskList`,
`TaskUpdate`, `TaskStop`, and `TodoWrite` intentionally stay loaded in
coco-rs even though `claude-code-kim` defers them. These high-frequency
plan/todo tools need to work on the first call across weaker non-Anthropic
providers without a ToolSearch round-trip. `TaskOutput` remains deferred
because it is deprecated and low-frequency.

## Bounds and intentional differences

- Results are capped at five; queries at 512 UTF-8 bytes; descriptions at 512
  bytes; each complete projected schema at 4 KiB; and the fully wrapped result
  at 8 KiB. Oversized schemas are omitted, never truncated or promoted.
- Server-controlled JSON embedded in `<function>` framing escapes `<`, `>`,
  and `&`; pending-server retry hints are independently count/byte bounded.
- Native provider expansion is used only for `Deferred` placement.
  `UseTool` placement always renders bounded client JSON and directs the model
  to `use_tool`.
- **Per-call `<available-deferred-tools>` legacy meta-message** — TS
  Path B prepends this on every request. coco-rs only implements the
  delta-attachment Path A — the `DeferredToolsDeltaGenerator`
  system-reminder.
- **`tst-auto` token-threshold mode** — TS auto-enables `ToolSearch`
  only when deferred-tool tokens exceed `auto:N%` of the context
  window. coco-rs always defers the canonical catalog regardless of
  context size; the gate would re-introduce the per-provider token
  math that the seam layer is designed to avoid.
- **Beta header injection** — `tool-search-tool-2025-10-19` /
  `advanced-tool-use-2025-11-20`. Provider crates don't need them.
- **GrowthBook / `USER_TYPE=ant` gates** — coco-rs is settings-only.

## File index

| File | Role |
|------|------|
| `core/tools/src/tools/tool_search.rs` | The tool itself: scoring, `+` parsing, `select:` parsing, promotion patch |
| `core/tools/src/tools/tool_search.test.rs` | Unit tests for parse + render + execute |
| `core/tool-runtime/src/registry.rs` | `loaded_tools` / `deferred_tools` filters consult `discovered_tool_names` |
| `core/tool-runtime/src/context.rs` | `ToolUseContext::discovered_tool_names` + `with_discovered_tool_names` |
| `common/types/src/app_state.rs` | `ToolAppState::discovered_tool_names` field |
| `app/query/src/tool_context.rs` | `ToolContextFactory::build` snapshots state → ctx |
| `app/query/src/engine_prompt.rs` | `build_tool_definitions` threads discovered set into stub |
| `app/query/src/engine_turn_reminders.rs` | partitions registry into deferred + loaded for the delta |
| `app/query/src/engine_helpers.rs` | `compute_tools_delta(deferred, loaded, announced)` |
| `core/system-reminder/src/generators/deferred_tools_delta.rs` | Emits the `<system-reminder>` from `DeferredToolsDeltaInfo` |
