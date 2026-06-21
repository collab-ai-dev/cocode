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

## Architecture

```
src/
├── facade.rs              RetrievalFacade — primary entry point (build_index, search, generate_repomap)
├── config.rs              RetrievalConfig — ~/.coco/retrieval.toml + workdir overrides
├── context.rs             RetrievalContext + RetrievalFeatures (presets: NONE/MINIMAL/STANDARD/FULL)
├── error.rs               RetrievalErr (structured; is_retryable, suggested_retry_delay_ms)
├── traits.rs              Indexer, Searcher, EmbeddingProvider, ChunkStore (all Send+Sync, #[async_trait])
├── types.rs               CodeChunk, SearchResult, SearchQuery, SourceFileId, ScoreType
├── events.rs              RetrievalEvent (isolated from CoreEvent — see below)
├── event_emitter.rs       EventEmitter + ScopedEventCollector
├── metrics.rs             CodeMetrics
├── health.rs              Health probes
│
├── indexing/              IndexManager, FileWatcher, walker, change_detector (SHA256)
├── chunking/              AST-aware splitter (tree-sitter) + fallback + token-budget collapser
├── embeddings/            fastembed (local ONNX) + openai + batched queue
├── search/                BM25 (k1=0.8, b=0.5), HybridSearcher, RRF fusion, RecentFilesCache, SnippetSearcher
├── storage/               SqliteStore (metadata + FTS5), SqliteVecStore / LanceDbStore (vectors), snippet storage
├── query/                 LLM-based query rewrite (CN/EN bilingual) + preprocessor
├── reranker/              RuleBasedReranker + local (fastembed) + remote (Cohere/Voyage)
├── repomap/               Dependency graph + PageRank + token-budgeted renderer
├── tags/                  tree-sitter symbol extraction
├── services/              IndexService, SearchService, RecentFilesService, SearchRequest builder
├── tui/                   Stand-alone TUI for `retrieval` binary
└── bin/                   `retrieval` binary (build/search/tui)
```

## Primary API

```rust
use coco_retrieval::{RetrievalFacade, FacadeBuilder, RetrievalFeatures, SearchRequest};

// Create facade for a workspace
let facade = FacadeBuilder::new(config)
    .features(RetrievalFeatures::STANDARD)
    .workspace(workdir)
    .build()
    .await?;

// Simple search (hybrid mode)
let results = facade.search("query").await?;

// Fluent builder for advanced modes
let results = facade.search_service().execute(
    SearchRequest::new("query").bm25().limit(10)
).await?;

// Or: .vector(), .hybrid(), .snippet()

// Operations
facade.build_index(mode, cancel_token).await?;    // Receiver<IndexProgress>
facade.generate_repomap(request).await?;           // RepoMapResult
```

Convenience: `create_manager(cwd, coco_home) -> Option<Arc<RetrievalFacade>>`
(honors `config.enabled`).

## Event Integration

`RetrievalEvent` is **intentionally isolated** from the main agent `CoreEvent`
stream (see `event-system-design.md` §1.7 and plan WS-7). Subscribe via
`EventEmitter::subscribe()`; do not bridge the full taxonomy into
`coco_types::ServerNotification`. If a slash command ever needs retrieval
progress in the agent stream, add a single aggregate variant via an optional
sink (pattern: `TaskManager::with_event_sink()`).

## Error Handling

Uses `RetrievalErr` (not `anyhow`). Key variants:
- `NotEnabled` — retrieval not configured
- `NotReady` — index building (retryable)
- `SqliteLockedTimeout` — concurrent access (retryable)
- `EmbeddingFailed`, `SearchFailed` — operation failures

Always check `is_retryable()` / `suggested_retry_delay_ms()` for transient errors.

## Supported Languages (AST)

| Language | Symbol Extraction | Chunking |
|----------|-------------------|----------|
| Go | yes | yes |
| Rust | yes | yes |
| Python | yes | yes |
| Java | yes | yes |

TypeScript / JavaScript / C++ are not yet supported for AST features (BM25
+ vector still work on any language).

## Configuration

`{workdir}/.codex/retrieval.toml` → `~/.codex/retrieval.toml` (workdir wins).
Sections: `indexing`, `chunking`, `search`, `embedding`, `query_rewrite`,
`extended_reranker`, `repo_map`.

CLI and TUI both use the facade — no direct access to `IndexManager`,
`SqliteStore`, or `FileWatcher`.
