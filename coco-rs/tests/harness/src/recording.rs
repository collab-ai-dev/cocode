//! Recording test-doubles: spy-grade observability for trait callbacks.
//!
//! Ported from opencode's testing convention ("avoid mocks; test the real
//! implementation"): a fake is a *real* implementation of the trait that
//! records every call into a shared buffer for assertion, and routes any
//! method a test must NOT reach to a loud panic (opencode wires those to
//! `Effect.die("unused")`). This gives negative assertions
//! (`handle.calls.is_empty()`) and turns an unexpected call into a hard
//! failure instead of a silent no-op — without any mocking framework.
//!
//! `coco-tool-runtime` already ships `NoOp*` handles, but they are silent.
//! [`RecordingHookHandle`] is the observable counterpart; [`Recorder`] is the
//! reusable capture primitive behind any hand-written double.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use coco_tool_runtime::HookHandle;
use coco_tool_runtime::PostToolUseOutcome;
use coco_tool_runtime::PreToolUseOutcome;
use serde_json::Value;

/// A cloneable, thread-safe capture buffer. Clones share one backing `Vec`, so
/// a double can hold one handle while the test asserts on another.
pub struct Recorder<T>(Arc<Mutex<Vec<T>>>);

impl<T> Clone for Recorder<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T> Default for Recorder<T> {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }
}

impl<T> std::fmt::Debug for Recorder<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.0.lock().map(|v| v.len()).unwrap_or(0);
        f.debug_struct("Recorder").field("count", &count).finish()
    }
}

impl<T: Clone> Recorder<T> {
    /// New empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one observed call.
    pub fn record(&self, item: T) {
        self.0.lock().expect("recorder mutex poisoned").push(item);
    }

    /// Clone out every recorded call, in order.
    pub fn snapshot(&self) -> Vec<T> {
        self.0.lock().expect("recorder mutex poisoned").clone()
    }

    /// Drain every recorded call, leaving the buffer empty.
    pub fn take(&self) -> Vec<T> {
        std::mem::take(&mut self.0.lock().expect("recorder mutex poisoned"))
    }

    /// Number of recorded calls.
    pub fn count(&self) -> usize {
        self.0.lock().expect("recorder mutex poisoned").len()
    }

    /// `true` when nothing has been recorded — the negative-assertion path.
    pub fn is_empty(&self) -> bool {
        self.count() == 0
    }

    /// Forget all recorded calls (reset between sub-cases).
    pub fn clear(&self) {
        self.0.lock().expect("recorder mutex poisoned").clear();
    }
}

/// Fail loudly for a trait method a test double must never reach — the Rust
/// analog of opencode's `Effect.die("unused")`. Use inside a hand-written
/// double for capabilities the test asserts are unused.
#[track_caller]
pub fn unexpected_call(method: &str) -> ! {
    panic!(
        "unexpected call to `{method}` on a test double (it was wired to reject unexpected use)"
    );
}

/// One captured [`HookHandle`] invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum HookCall {
    /// `run_pre_tool_use`.
    Pre {
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
    },
    /// `run_post_tool_use` (success path).
    Post {
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
        tool_response: Value,
    },
    /// `run_post_tool_use_failure`.
    PostFailure {
        tool_name: String,
        tool_use_id: String,
        tool_input: Value,
        error_message: String,
    },
}

/// A [`HookHandle`] that records every call (returning default, no-op outcomes)
/// so a test can assert exactly which hooks the engine fired, with what
/// arguments, in what order. Unlike `NoOpHookHandle`, it is observable.
#[derive(Debug, Clone, Default)]
pub struct RecordingHookHandle {
    /// Every observed call, in order. Clone-shared with returned handles.
    pub calls: Recorder<HookCall>,
}

#[async_trait]
impl HookHandle for RecordingHookHandle {
    async fn run_pre_tool_use(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
    ) -> PreToolUseOutcome {
        self.calls.record(HookCall::Pre {
            tool_name: tool_name.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_input: tool_input.clone(),
        });
        PreToolUseOutcome::default()
    }

    async fn run_post_tool_use(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
        tool_response: &Value,
    ) -> PostToolUseOutcome {
        self.calls.record(HookCall::Post {
            tool_name: tool_name.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_input: tool_input.clone(),
            tool_response: tool_response.clone(),
        });
        PostToolUseOutcome::default()
    }

    async fn run_post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_use_id: &str,
        tool_input: &Value,
        error_message: &str,
    ) -> PostToolUseOutcome {
        self.calls.record(HookCall::PostFailure {
            tool_name: tool_name.to_string(),
            tool_use_id: tool_use_id.to_string(),
            tool_input: tool_input.clone(),
            error_message: error_message.to_string(),
        });
        PostToolUseOutcome::default()
    }
}

#[cfg(test)]
#[path = "recording.test.rs"]
mod tests;
