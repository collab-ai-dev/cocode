//! `HookEventsSource` impls for the per-turn reminder pipeline.
//!
//! Two impls live here:
//!
//! - `AsyncHookRegistry` — bridges completed async-hook responses.
//!   Each call to [`HookEventsSource::drain`] delegates to
//!   `AsyncHookRegistry::collect_responses()` — which marks responses
//!   delivered — and maps each response to a `HookEvent::AsyncResponse`.
//!
//! - [`CombinedHookEventsSource`] — wraps an `AsyncHookRegistry` and a
//!   [`SyncHookEventBuffer`] so both async drain output and sync hook
//!   results (pushed by orchestration in `execute_session_start` /
//!   `execute_user_prompt_submit`) reach the reminder pipeline through
//!   a single trait object. Drain order: async first, sync second.
//!   This is the impl `SessionRuntime::wire_engine` installs.

use std::sync::Arc;

use async_trait::async_trait;

use crate::async_registry::AsyncHookRegistry;
use crate::async_registry::AsyncHookResponse;
use crate::sync_hook_buffer::SyncHookEventBuffer;
use coco_system_reminder::HookEvent;
use coco_system_reminder::HookEventsSource;

#[async_trait]
impl HookEventsSource for AsyncHookRegistry {
    async fn drain(&self, _agent_id: Option<&str>) -> Vec<HookEvent> {
        // `collect_responses` drains (marks delivered), so repeat
        // calls don't re-emit.
        self.collect_responses()
            .await
            .into_iter()
            .map(to_hook_event)
            .collect()
    }
}

/// `HookEventsSource` that fans out to both the async hook registry and
/// the sync hook buffer. Cloning is cheap — each field is already
/// reference-counted internally.
#[derive(Debug, Clone)]
pub struct CombinedHookEventsSource {
    async_registry: Arc<AsyncHookRegistry>,
    sync_buffer: SyncHookEventBuffer,
}

impl CombinedHookEventsSource {
    pub fn new(async_registry: Arc<AsyncHookRegistry>, sync_buffer: SyncHookEventBuffer) -> Self {
        Self {
            async_registry,
            sync_buffer,
        }
    }
}

#[async_trait]
impl HookEventsSource for CombinedHookEventsSource {
    async fn drain(&self, agent_id: Option<&str>) -> Vec<HookEvent> {
        // Async first so the model sees background-hook output before
        // the per-turn sync events that fired in the current iteration.
        // Order is observable in tests and in the rendered prompt.
        let mut out = self.async_registry.drain(agent_id).await;
        out.extend(self.sync_buffer.drain().await);
        out
    }
}

/// Map a completed async-hook response to the `async_hook_response`
/// reminder shape. `stdout` becomes `system_message`; `stderr` flows
/// into `additional_context` when non-empty. Empty fields are `None`
/// so the generator can short-circuit.
fn to_hook_event(r: AsyncHookResponse) -> HookEvent {
    let system_message = if r.stdout.is_empty() {
        None
    } else {
        Some(r.stdout)
    };
    let additional_context = if r.stderr.is_empty() {
        None
    } else {
        // Prefix with hook name so the model knows which hook surfaced
        // the stderr.
        Some(format!("[{}] {}", r.hook_name, r.stderr))
    };
    HookEvent::AsyncResponse {
        system_message,
        additional_context,
    }
}

#[cfg(test)]
#[path = "reminder_source.test.rs"]
mod tests;
