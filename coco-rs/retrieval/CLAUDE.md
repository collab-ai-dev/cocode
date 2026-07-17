# coco-retrieval

Code retrieval subsystem: BM25 full-text + vector semantic + AST symbol
extraction + PageRank repo-map. Single `RetrievalFacade` entry point for
agents, TUI, and CLI.

**Note**: This crate is a new coco-rs subsystem, not a port of any upstream
component. It is owned directly by coco-rs (see `../CLAUDE.md` for workspace
conventions).

## Feature Flags

| Feature | Description | Dependencies |
|---------|-------------|--------------|
| `local-embeddings` | Local embeddings via fastembed (ONNX: nomic-embed-text, bge-*, MiniLM-*) | `fastembed` |
| `neural-reranker` | Local neural reranker via fastembed (bge-reranker, jina-reranker) | `fastembed` |
| `local` | Both of the above | `fastembed` |

Default build is lightweight — BM25 + OpenAI embeddings / reranker API only.
BM25 is tuned for code: `k1 = 0.8`, `b = 0.5` (`search/bm25_index.rs`).

## Primary API

**Everything goes through `RetrievalFacade`** — CLI, TUI, and agents alike.
No direct access to `IndexManager`, `SqliteStore`, or `FileWatcher`.

- Build: `FacadeBuilder::new(config).features(RetrievalFeatures::STANDARD).workspace(dir).build().await?`
  — feature presets `NONE` / `MINIMAL` / `STANDARD` / `FULL`.
- Search: `facade.search("query")` (hybrid); advanced modes via
  `facade.search_service().execute(SearchRequest::new("q").bm25().limit(10))`
  — or `.vector()` / `.hybrid()` / `.snippet()`.
- Ops: `facade.build_index(mode, cancel_token)` (returns `Receiver<IndexProgress>`),
  `facade.generate_repomap(request)`.
- Convenience: `create_manager(cwd, coco_home) -> Option<Arc<RetrievalFacade>>`
  (honors `config.enabled`).

## Event Integration

`RetrievalEvent` is **intentionally isolated** from the main agent `CoreEvent`
stream (see `event-system-design.md` §1.7 and plan WS-7). Subscribe via
`EventEmitter::subscribe()`; do not bridge the full taxonomy into
`coco_types::ServerNotification`. Callers that need coarse progress can install
`EventEmitter::set_aggregate_sink(...)`; the sink receives only
`RetrievalAggregateEvent` summaries for started/progress/completed/error and
leaves the detailed retrieval event stream isolated.

## Unified Coordinator

`UnifiedCoordinator` owns independent Index and Tag pipelines. Readiness
methods are async all the way down; do not call `futures::executor::block_on`
inside coordinator async methods. Watcher/timer dispatch may push the same file
change to both pipelines concurrently with `tokio::join!`, but each pipeline
keeps its own sequence allocation, queue ordering, lag tracker, and batch
tracker.

## Error Handling

Uses `RetrievalErr` (not `anyhow`). Key variants:
- `NotEnabled` — retrieval not configured
- `NotReady` — index building (retryable)
- `SqliteLockedTimeout` — concurrent access (retryable)
- `EmbeddingFailed`, `SearchFailed` — operation failures

Always check `is_retryable()` / `suggested_retry_delay_ms()` for transient errors.

## Languages (AST)

TypeScript / JavaScript / C++ are not yet supported for AST features (symbol
extraction, AST-aware chunking); BM25 + vector search work on any language.

## Configuration

`{workdir}/.cocode/retrieval.toml` → `{coco_home}/retrieval.toml` (project
wins; neither present → disabled default). Sections: `indexing`, `chunking`,
`search`, `embedding`, `query_rewrite`, `extended_reranker`, `repo_map`.
