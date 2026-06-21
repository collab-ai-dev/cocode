//! Team memory sync subsystem (HTTP-backed).
//!
//! Round-9-deep-port skeleton: types are complete (`types.rs`); the
//! HTTP push/pull pipeline + watcher live in this module's
//! `service.rs` follow-up. Until the HTTP wiring lands, callers can
//! still use the typed structs to model server responses or stage
//! their own offline tooling.

pub mod secret_scanner;
pub mod service;
pub mod types;
pub mod watcher;

pub use secret_scanner::scan_for_secrets;
pub use service::PushEntry;
pub use service::apply_pulled_content;
pub use service::compute_content_hash;
pub use service::endpoint;
pub use service::pull;
pub use service::push;
pub use types::SkippedSecretFile;
pub use types::SyncState;
pub use types::TeamMemoryContent;
pub use types::TeamMemoryData;
pub use types::TeamMemoryHashesResult;
pub use types::TeamMemorySyncFetchResult;
pub use types::TeamMemorySyncPushResult;
pub use types::TeamMemorySyncUploadResult;
pub use types::{MAX_FILE_SIZE_BYTES, MAX_PUT_BODY_BYTES, SYNC_TIMEOUT_MS};
pub use watcher::WatcherConfig;
pub use watcher::run_watch_loop;
pub use watcher::spawn_watcher;
