//! Crate-wide test synchronization for process-global state.
//!
//! Tests that mutate `COCO_CONFIG_DIR` must share this one lock. Per-module
//! locks do not protect tests running concurrently in the same `cargo test`
//! process.

use std::sync::LazyLock;

pub(crate) static CONFIG_ENV_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
