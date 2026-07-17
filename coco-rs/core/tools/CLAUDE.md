# coco-tools

Built-in tool implementations. Statically-typed tools (`type Input = SomeStruct`) plus two dynamic-schema `type Input = Value` tools: `McpTool` (wire schema from MCP server / SDK in-process transport) and `StructuredOutputTool` (user JSON Schema via `--json-schema` or workflow `agent(prompt, {schema})`). Each implements `coco_tool_runtime::Tool`; `coco-tool-runtime` defines the trait. The default set is whatever `register_all_tools` registers (`src/lib.rs`) — don't rely on a hardcoded count or list.

## Key Types

- `register_all_tools(&ToolRegistry)` — the default static-tool set (single source of truth; includes the goal tools)
- `register_core_tools(&ToolRegistry)` — Bash/Read/Write/Edit/Glob/Grep only (lightweight)
- `register_structured_output_tool(...)` — opt-in registration (headless print / SDK NDJSON paths, or a workflow child engine's private registry); `StructuredOutputTool` is intentionally excluded from `register_all_tools`
- `register_mcp_tools(registry, server_name, tools)` — dynamic registration after MCP server connect (idempotent; deregisters prior tools from the same server first); `deregister_mcp_server` on disconnect
- Tool input enums (`input_types.rs`): `GrepOutputMode`, `LspAction`
- One file per tool under `src/tools/` (`lsp_tool.rs` is suffixed because `lsp.rs` holds the shared DTOs + formatters the tool consumes)

## Cross-Cutting Helpers (crate-private)

- `record_file_read` / `record_file_edit` — updates `FileReadState` for @mention dedup + Read-tool `file_unchanged` detection
- `tools::read_loader` — side-effect-free file classification/loading shared by `ReadTool` and background changed-file scans
- `check_team_mem_secret` — blocks writes containing secrets into team-memory paths (layered detection: authoritative via `coco-memory::team_paths` + substring fallback, gated by `coco-secret-redact`)
- `track_nested_memory_attachment` — pushes read paths into `ctx.nested_memory_attachment_triggers` for next-turn CLAUDE.md loading
- `track_skill_discovery` — discovers `.coco/skills` in file ancestry, pushes to `ctx.dynamic_skill_dir_triggers`
- `track_file_edit` — records edits in `FileHistoryState` for checkpoint/rewind

## Architecture

- **Two dynamic-schema tools** (`type Input = Value`): the schema is
  runtime-supplied, so both build a `ToolInputSchema::from_value(...)` from the
  wire/user schema and return it from `runtime_validation_schema()` (`McpTool`
  in `mcp_tools.rs`, `StructuredOutputTool` in `structured_output.rs`). Never
  derive a schema from `Value` — that yields a `type: null` schema that strict
  OpenAI-compatible providers (DeepSeek etc.) reject. Any future
  `type Input = Value` tool inherits the same obligation. See
  `docs/internal/tool-schema-final-plan.md` and the Schema-ownership section in
  `core/tool-runtime/CLAUDE.md`.
- All file-mutation tools (Edit/Write/NotebookEdit/apply_patch/Bash) invoke the team-mem secret guard + file-history tracking helpers before touching disk.

### Task/Todo defer policy

`TaskCreate`, `TaskGet`, `TaskList`, `TaskUpdate`, `TaskStop`, and
`TodoWrite` intentionally return `should_defer() = false`, diverging from
upstream. They are high-frequency plan/todo tools, and keeping them
loaded avoids a ToolSearch round-trip before the first call on weaker
non-Anthropic providers. `TaskOutput` is the only deferred Task-family tool
because it is deprecated and low-frequency.

### LSP tool

`LspAction` wire format is **camelCase** (`goToDefinition` / `findReferences` /
`hover` / `documentSymbol` / `workspaceSymbol` / `goToImplementation` /
`prepareCallHierarchy` / `incomingCalls` / `outgoingCalls`) so the model's tool
calls validate identically across runtimes. Diagnostics are **not** an
`LspAction` — they flow through the passive system-reminder pipeline
(`coco-lsp::DiagnosticsStore` → `app/query::reminder_adapters`).

`LspTool::is_enabled` is double-gated: `Feature::Lsp` enabled **and**
`ctx.lsp.is_connected()` — without either, the tool is filtered out of the
model's tool list.

Dispatch invariants:

- Relative paths resolve against `ctx.cwd_override` (worktree-aware) → process cwd; `LspServerManager::get_client(path)` walks up to `.git` / `Cargo.toml` for per-worktree auto-routing.
- `validate_lsp_file` rejects UNC paths (`\\…` / `//…`, Windows NTLM safety) and files larger than 10MB.
- 1-based input `line`/`character` → 0-based LSP `Position`.
- `incomingCalls` / `outgoingCalls` run the two-step pattern: `prepareCallHierarchy` → pick first item → `callHierarchy/{incomingCalls,outgoingCalls}`.
- Location-returning ops (`goToDefinition` / `findReferences` / `goToImplementation` / `workspaceSymbol`) are filtered through `coco_file_ignore::PathChecker` (in-process unified ignore path).
- `Write` / `Edit` / `NotebookEdit` / `ApplyPatch` call `ctx.lsp.notify_save(path)` after a successful write: sends `textDocument/didSave` only if the file is already in the server's `opened` tracker AND clears the file's entries from `DiagnosticsStore.delivered_for_file`, so re-published diagnostics for the edited file are not suppressed by cross-turn dedup.

## Per-tool Result Persistence Thresholds

`Tool::max_result_size_bound()` overrides (`ResultSizeBound::{Bytes, Unbounded}`)
— read by the query tool outcome builder per Level 1 of the
[Tool Result Offload design](../../../docs/internal/tool-result-offload-v2-design.md).
Over-threshold results are windowed (head+tail) and the complete output is
persisted with a recoverable `<persisted-output>` pointer.
**Declared bounds are authoritative — no hidden global clamp.**

| Tool | Value | Note |
|---|---|---|
| BashTool | `Bytes(30_000)` | bursty shell output; also overrides `inline_window_budget()` = 30K so tail errors survive the window |
| PowerShellTool | `Bytes(30_000)` | same as Bash |
| GrepTool | `Bytes(20_000)` | match dumps grow superlinearly |
| GlobTool | `Bytes(100_000)` | path lists tolerate larger windows |
| WebFetchTool | `Bytes(102_000)` | self-bounds every arm via the offload seam; declared above the default so the preapproved-docs verbatim window (100K) passes Level 1 whole |
| FileReadTool | `Unbounded` | canonical content — opt out of persistence |
| Most other static tools | trait default `Bytes(50_000)` | |

Bash no longer persists to `temp_dir()`: `decode_capped` keeps the complete
output (up to `bash.max_output_bytes`, the RETAIN cap — default 2 MB) and the
generic offload seam windows it inline + persists it under the session
`tool-results/` directory. WebFetch offloads through the same seam with a
content-addressed `ArtifactKey::Named`.

## Divergences from upstream behavior

### WebSearchTool — client-side backends instead of Anthropic server tool

Upstream routes search as a passthrough to the Anthropic-only
`web_search_20250305` server tool (runs on Anthropic infrastructure, returns
`server_tool_use` + `web_search_tool_result` blocks with inline citations) —
no other provider has an equivalent. coco-rs must work against every provider,
so search is **client-side** with a pluggable backend via
`WebSearchConfig.provider` (`common/config`):

- **DuckDuckGo HTML scraping** (default) — no API key. POSTs to
  `html.duckduckgo.com/html/`, regex-parses anchors + snippets, decodes the
  `uddg=` redirect to the target URL.
- **Tavily REST API** — opt-in via `provider = "tavily"` + `api_key` (or
  `TAVILY_API_KEY` env). Structured JSON, no scraping.
- **OpenAI** variant currently falls back to DuckDuckGo (future expansion).

Trade-offs vs native passthrough: citations become a model-built `Sources:`
section (the prompt requires it); one blocking fetch instead of streamed
deltas; rate limits + domain filters are per-backend and client-side
(post-fetch host-suffix match); works anywhere the backend is reachable
(the native tool is US-only).

### Cache keys

The search cache is keyed on `(provider, max_results, query)` — not just
`query`. A DuckDuckGo result at `max_results=5` cannot be served to a
Tavily request at `max_results=20`. Error-classification wrapping via
`WebSearchErrorType` lets the model distinguish retryable (`TIMEOUT`,
`NETWORK_ERROR`) from non-retryable (`API_KEY_MISSING`, `PARSE_ERROR`)
failures via the `[TAG] message` prefix.
