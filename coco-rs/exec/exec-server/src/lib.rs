//! Minimal shim providing the filesystem abstraction that `coco-apply-patch`
//! consumes from codex-rs. Only the `ExecutorFileSystem` trait and a
//! `LocalFileSystem` implementation backed by `tokio::fs` are exposed.
//!
//! Codex's `codex-exec-server` additionally exposes process-execution RPC
//! and sandbox-policy-aware variants; those are intentionally not ported
//! here because coco-rs has its own sandbox stack and the other surface area
//! is not required by apply-patch.

mod file_system;
mod local_file_system;

pub use file_system::CopyOptions;
pub use file_system::CreateDirectoryOptions;
pub use file_system::ExecutorFileSystem;
pub use file_system::FileMetadata;
pub use file_system::FileSystemResult;
pub use file_system::ReadDirectoryEntry;
pub use file_system::RemoveOptions;
pub use local_file_system::LOCAL_FS;
