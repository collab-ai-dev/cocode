//! Hot-reload loop wiring [`coco_file_watch::FileWatcher`] →
//! [`coco_config::RuntimePublisher`].
//!
//! Lives in its own crate so `coco-config` (L1) does not require a
//! Tokio runtime. This crate sits at L2 alongside `coco-inference`.

mod reloader;

pub use reloader::ConfigChange;
pub use reloader::ConfigReloadError;
pub use reloader::ReloadOptions;
pub use reloader::RuntimeReloader;
pub use reloader::TrackedKind;
