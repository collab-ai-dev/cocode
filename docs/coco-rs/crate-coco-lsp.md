# coco-lsp — Crate Plan

TS source: `services/lsp/` (7 files, ~3K LOC)
cocode-rs source: `lsp/` (516K LOC) — HYBRID strategy (cocode-rs base + TS diagnostic tracking + plugin discovery)

cocode-rs provides the production-ready LSP infrastructure:
- `LspServerManager` (multi-server lifecycle, 5 restart backoff)
- `LspClient` (AI-friendly query-by-name: `client.definition(path, "Config", Some(SymbolKind::Struct))`)
- `DiagnosticsStore` (300ms debounce)
- Built-in servers: rust-analyzer, gopls, pyright, typescript-language-server
- `ServerLifecycle` (max 5 restarts, exponential backoff)

TS adds on top: diagnostic dedup LRU, plugin-based discovery, passive feedback.

## Dependencies

```
coco-lsp depends on:
  - coco-types (SymbolKind), coco-config (Settings)
  - tokio (async server lifecycle)

coco-lsp does NOT depend on:
  - coco-tool, coco-tools, coco-query (standalone service)
```

## Strategy: HYBRID

cocode-rs has the core LSP client with AI-friendly symbol queries. TS adds:
1. Diagnostic tracking with LRU deduplication (LSPDiagnosticRegistry)
2. Plugin-based server discovery
3. Passive feedback system
4. Crash recovery with restart limits

## Server Lifecycle (6 states)

```rust
pub enum LspServerState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
    Restarting,  // transient: error → starting
}

/// State transitions:
/// Stopped → Starting → Running
/// Running → Stopping → Stopped
/// Any → Error (on crash/init failure)
/// Error → Restarting → Starting (auto-recovery, max 3 restarts)
```

## Diagnostic Registry (from `LSPDiagnosticRegistry.ts`)

```rust
/// Cross-turn diagnostic deduplication using LRU cache.
/// Prevents re-delivering identical diagnostics across agent turns.
pub struct DiagnosticRegistry {
    delivered: LruCache<DiagnosticKey, ()>,  // capacity: 500
}

pub struct DiagnosticKey {
    pub message: String,
    pub severity: DiagnosticSeverity,
    pub range: Range,
    pub source: Option<String>,
    pub code: Option<String>,
}

/// Volume limits:
/// MAX_DIAGNOSTICS_PER_FILE: 10
/// MAX_TOTAL_DIAGNOSTICS: 30
/// Sorted by severity: Error(1) > Warning(2) > Info(3) > Hint(4)
```

## 9 LSP Operations

```rust
pub enum LspOperation {
    DidOpen,           // textDocument/didOpen
    DidChange,         // textDocument/didChange
    DidClose,          // textDocument/didClose
    DidSave,           // textDocument/didSave
    Definition,        // textDocument/definition
    Hover,             // textDocument/hover
    References,        // textDocument/references
    DocumentSymbol,    // textDocument/documentSymbol
    CallHierarchy,     // textDocument/callHierarchy
}
```

## Crash Recovery

```rust
/// Exponential backoff retry on LSP_ERROR_CONTENT_MODIFIED (-32801):
/// 500ms → 1000ms → 2000ms
/// Max restarts: 3 (configurable). After max, stays in Error state.
pub struct CrashRecovery {
    pub restart_count: i32,
    pub max_restarts: i32,  // default: 3
}
```

## Plugin Discovery

```rust
/// LSP server configs loaded from plugins via plugin system.
/// Extension → language → server mapping via extensionToLanguage config.
/// Parallel loading per plugin; later plugins override earlier ones.
pub async fn get_all_lsp_servers(
    settings: &Settings,
    plugins: &[InstalledPlugin],
) -> HashMap<String, LspServerConfig>;
```

## TS → Rust File Mapping

| TS File | Purpose | Strategy |
|---------|---------|----------|
| `LSPServerManager.ts` | Server lifecycle, multi-server | cocode-rs base + TS states |
| `LSPClient.ts` | LSP protocol client | cocode-rs (already superior) |
| `LSPServerInstance.ts` | Per-server state tracking | cocode-rs base |
| `LSPDiagnosticRegistry.ts` | LRU dedup, volume limiting | **TS** (new in coco-lsp) |
| `config.ts` | Plugin-based server discovery | **TS** (new in coco-lsp) |
| `diagnosticTracking.ts` | IDE diagnostic tracking via MCP | **TS** (397 LOC) |
| `passiveFeedback.ts` | Passive user feedback system | **TS** (optional) |
