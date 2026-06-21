# coco-file-search

Fuzzy file search over a walked directory tree (nucleo + `ignore::WalkBuilder` via coco-file-ignore).

## Key Types
| Type | Purpose |
|------|---------|
| `run` | One-shot blocking search → `FileSearchResults` |
| `create_session` | Long-lived session with incremental `update_query` and streaming snapshots |
| `FileSearchOptions` | `limit`, `exclude`, `threads`, `compute_indices`, `respect_gitignore` |
| `FileMatch`, `FileSearchSnapshot`, `FileSearchResults` | Result shapes (score, path, root, optional match indices) |
| `SessionReporter` | `on_update` / `on_complete` callbacks for streaming mode |
| `FileIndex`, `SharedFileIndex`, `create_shared_index`, `discover_files` | Cached directory index (re-exported from `index`) |
| `Cli` | `clap` args for the standalone binary |
