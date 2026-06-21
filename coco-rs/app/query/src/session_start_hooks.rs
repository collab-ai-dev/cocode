//! Query-layer bridge for SessionStart hook side effects produced during
//! post-compact context restoration.
//!
//! The compact flow executes SessionStart hooks inside `coco-query`, but
//! runtime side effects such as FileChanged watch registration belong to
//! the app layer. This trait keeps the dependency one-way.

use std::sync::Arc;

/// Runtime side effects from SessionStart hook output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStartHookSideEffects {
    /// Optional user message requested by a SessionStart hook.
    pub initial_user_message: Option<String>,
    /// File paths that should be registered with the FileChanged watcher.
    pub watch_paths: Vec<String>,
}

impl From<&coco_hooks::orchestration::AggregatedHookResult> for SessionStartHookSideEffects {
    fn from(result: &coco_hooks::orchestration::AggregatedHookResult) -> Self {
        Self {
            initial_user_message: result
                .initial_user_message
                .as_ref()
                .map(|m| m.trim())
                .filter(|m| !m.is_empty())
                .map(str::to_string),
            watch_paths: result.watch_paths.clone(),
        }
    }
}

/// Sink installed by the app runtime to apply side effects that
/// `coco-query` cannot own directly.
#[async_trait::async_trait]
pub trait SessionStartHookSideEffectSink: Send + Sync {
    async fn handle_session_start_hook_side_effects(&self, effects: SessionStartHookSideEffects);
}

pub type SessionStartHookSideEffectSinkRef = Arc<dyn SessionStartHookSideEffectSink>;
