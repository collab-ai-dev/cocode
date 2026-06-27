# coco-lsp

AI-friendly LSP client — queries by symbol name + kind instead of exact line/column. Rust-native LSP core (server manager, lifecycle, JSON-RPC, symbol cache, incremental sync) + diagnostic store + plugin extensions.

## Key Types

- `LspServerManager`, `ServerConfigInfo`, `ServerStatus`, `ServerStatusInfo` — top-level manager
- `LspClient` — per-connection AI-friendly operations + caching
- `LspServersConfig`, `LspServerConfig`, `BuiltinServer`, `BUILTIN_SERVERS`, `ConfigLevel`, `LifecycleConfig`, `LSP_SERVERS_CONFIG_FILE`, `command_exists`
- `SymbolKind`, `ResolvedSymbol`, `SymbolMatch`, `find_matching_symbols`, `flatten_symbols`
- `DiagnosticsStore`, `DiagnosticEntry`, `DiagnosticSeverityLevel`
- `ServerLifecycle`, `ServerHealth`, `ServerStats`
- `LspInstaller`, `InstallEvent`, `InstallerType`
- `TimeoutConfig`, `LspErr`
- `create_manager()` — convenience constructor

## AI-Friendly Symbol Resolution

Query by name+kind instead of position:
```rust
client.definition(path, "Config", Some(SymbolKind::Struct)).await?;
client.references(path, "process", Some(SymbolKind::Function), true).await?;
```
Position-based variants available with `_at_position` suffix. `SymbolKind::from_str_loose()` accepts `fn`/`func`/`function`, `trait`/`interface`, `var`/`let`/`variable`, etc.

## Built-in Language Servers

| Server | Extensions | Install |
|--------|------------|---------|
| rust-analyzer | `.rs` | `rustup component add rust-analyzer` |
| gopls | `.go` | `go install golang.org/x/tools/gopls@latest` |
| pyright | `.py`, `.pyi` | `npm install -g pyright` |
| typescript-language-server | `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`, `.cjs` | `npm install -g typescript-language-server typescript` |

## Configuration

Config files: `~/.coco/lsp_servers.json` (user) → `.coco/lsp_servers.json` (project overrides). Adding a built-in: extend `BUILTIN_SERVERS` in `config.rs` with `id`, `extensions`, `commands`, `install_hint`, `languages`.

## Constants

| Constant | Value | Purpose |
|----------|-------|---------|
| `MAX_OPENED_FILES` | 500 | File tracking limit |
| `MAX_SYMBOL_CACHE_SIZE` | 100 | Symbol cache entries per file |
| `MAX_INCREMENTAL_CONTENT_SIZE` | 1MB | Incremental sync threshold (Myers diff; falls back to full sync above) |
| `LRU_EVICTION_PERCENT` | 25% | Cache eviction batch size |
| `HEALTH_CHECK_TIMEOUT_SECS` | 5 | Health probe timeout |

## Lifecycle Management

`ServerLifecycle` tracks crashes with max restarts + exponential backoff. Health check tries `workspace/symbol`, falls back to `hover` on any open file.

## Tool Layer Integration

The agent-facing `LspTool` lives in `coco-tools` and dispatches via the
`coco_tool_runtime::LspHandle` trait. The concrete adapter
(`coco_cli::lsp_handle_adapter::LspManagerAdapter`) wraps an
`Arc<LspServerManager>` + the manager's `DiagnosticsStore`. Integration seam:

| Operation | Implementation |
|-----------|---------------|
| LSP tool | `coco-tools::tools::lsp_tool::LspTool` |
| Formatters | `coco-tools::tools::lsp::format_*` |
| `isLspConnected()` | `LspHandle::is_connected()` (sync; backed by `LspServerManager::has_configured_servers` + adapter `has_active` AtomicBool) |
| `initialize()` | `LspManagerAdapter::prewarm(project_root)` — parallel `join_all` over deduped `(server_id)` set |
| Save + clear diagnostics | `LspHandle::notify_save(path)` (adapter calls both) |
| Open/change/save/close file | `LspClient::sync_file/update_file/notify_save/...` (via raw JSON-RPC) |

`LspHandle::send_request(file, method, params)` is the universal dispatch
path — it auto-routes via `find_project_root(file)` so files in a git
worktree (`.git` is a file, not a dir; both pass `Path::exists()`) are
served by an LSP rooted at the worktree.

## Notes

- **Does NOT follow the `*_ext.rs` extension pattern** — direct file modifications preferred.
- Symbol cache invalidation is version-tracked per file.
- Re-exports `lsp_types::{CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, Location, SymbolInformation}`.
- `LspClient::send_raw_request(method, params)` is the protocol-level passthrough used by the tool-layer adapter; in-crate callers should prefer the typed wrappers (`definition`/`references`/`hover`/…).
