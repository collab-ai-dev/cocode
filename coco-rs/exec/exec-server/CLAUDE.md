# coco-exec-server

Minimal filesystem abstraction so `coco-apply-patch` can consume the same `ExecutorFileSystem` trait + `LOCAL_FS` static that `codex-rs` uses.

## Source

Ported from `codex-rs/exec-server` (NOT from claude-code TS). Only the filesystem subset is ported — the process-execution RPC and sandbox-policy-aware variants from codex-rs are intentionally omitted. coco-rs has its own sandbox stack (`coco-sandbox`); the only current consumer (`coco-apply-patch`) uses the non-sandbox methods exclusively.

## Key Types

- `ExecutorFileSystem` trait — async, tokio-backed
- `LocalFileSystem`, `LOCAL_FS` static
- Companion types: `CreateDirectoryOptions`, `RemoveOptions`, `CopyOptions`, `FileMetadata`, `ReadDirectoryEntry`, `FileSystemResult`
