//! Message-bucket helpers for `ToolCallRunner`.
//!
//! One tool call produces messages in six distinct buckets. The flatten
//! template depends on whether the lifecycle reached:
//!
//! - `Success` — full pre-hook + execute + post-hook path. Non-MCP and MCP
//!   tools emit the post-hook bucket in **different** positions relative
//!   to `new_messages`.
//! - `Failure` — `tool.execute()` threw. Post-hook is `PostToolUseFailure`,
//!   no prevent-continuation, no MCP defer.
//! - `EarlyReturn` — unknown tool, schema failure, pre-hook stop,
//!   permission denial. No post-hook ran; only a synthetic error
//!   `tool_result` plus any pre-hook messages already emitted.
//!
//! This module is a runner-local helper. Nothing outside `app/query`
//! inspects `ToolMessageBuckets` — it collapses into the pre-flattened
//! `ordered_messages: Vec<Message>` that the scheduler surfaces.
//!
//! Scaffolding note: the `ToolCallRunner::run_one` implementation in
//! Phase 4d is the first consumer. Dead-code warnings are suppressed
//! on the whole module because the helper is already exercised by the
//! companion `tool_message.test.rs` and will be wired into the runner
//! in the next step.
#![allow(dead_code)]

use coco_messages::Message;
use coco_tool_runtime::DynTool;

/// Which lifecycle path produced this bucket set.
///
/// The flatten algorithm branches on this — MCP deferred-post-hook
/// ordering only applies to `Success`. `Failure` and `EarlyReturn` use
/// a single canonical order regardless of `is_mcp`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ToolMessagePath {
    /// `tool.execute()` ran to completion. Post-hook is `PostToolUse`.
    /// Non-MCP and MCP success paths differ per I5.
    #[default]
    Success,
    /// `tool.execute()` threw. Post-hook is `PostToolUseFailure`.
    /// Returns `[error tool_result, ...failure hook messages]` —
    /// MCP deferred logic does not apply.
    Failure,
    /// Unknown tool / schema / pre-hook stop / permission denied.
    /// No post-hook ran; only pre-hook (if any emitted) + synthetic
    /// error `tool_result`. JSON parse failure is **not** here — it
    /// is a pre-commit drop handled by the streaming accumulator
    /// before any bucket exists.
    EarlyReturn,
}

/// Selects the success-path flatten template.
///
/// Non-MCP emits post-hook inline between `tool_result` and
/// `new_messages`; MCP defers post-hook to AFTER `new_messages` +
/// `prevent_continuation`.
///
/// Use a typed enum (not `bool`) so call sites cannot accidentally
/// invert true/false; matches "Prefer Typed Results Over Booleans".
/// Only consulted when `path == Success`; Failure/EarlyReturn use a
/// single canonical order regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolMessageOrder {
    NonMcp,
    Mcp,
}

impl ToolMessageOrder {
    /// Resolve from the running tool itself. Using `Tool::is_mcp()`
    /// rather than `ToolId::Mcp` keeps the MCP branch tied to the
    /// same predicate the runner uses elsewhere and avoids drift
    /// between registry key and runtime tool metadata.
    pub(crate) fn for_tool(tool: &dyn DynTool) -> Self {
        if tool.is_mcp() {
            Self::Mcp
        } else {
            Self::NonMcp
        }
    }
}

/// Six-bucket message payload for a single tool call (pre-flatten).
///
/// Buckets are runner-local; the scheduler never inspects them. They
/// collapse into a `Vec<Message>` via [`ToolMessageBuckets::flatten`]
/// while the runner still holds the `Arc<dyn DynTool>` (so the MCP
/// predicate agrees with the tool implementation).
#[derive(Debug, Default)]
pub(crate) struct ToolMessageBuckets {
    /// PreToolUse hook-emitted `message` events. Pushed before
    /// permission / execution.
    pub(crate) pre_hook: Vec<Message>,
    /// The tool_result itself (or synthetic error). Always exactly one
    /// entry; wrapped in `Option` only so the builder can accumulate
    /// before sealing. Treat `None` as a programming error — enforced
    /// at `flatten` time.
    pub(crate) tool_result: Option<Message>,
    /// `ToolResult::new_messages` emitted by the tool. Empty on failure
    /// / early-return.
    pub(crate) new_messages: Vec<Message>,
    /// PostToolUse (Success) or PostToolUseFailure (Failure) hook
    /// output — additional_contexts etc. Empty on EarlyReturn.
    pub(crate) post_hook: Vec<Message>,
    /// Synthetic `hook_stopped_continuation` attachment, only on the
    /// Success path. Always `None` when
    /// `path == Failure`/`EarlyReturn` — see I5.
    pub(crate) prevent_continuation_attachment: Option<Message>,
    /// Which lifecycle produced this bucket set. Drives the flatten
    /// template; also surfaced as `ToolMessagePath` in
    /// `UnstampedToolCallOutcome` for telemetry.
    pub(crate) path: ToolMessagePath,
}

impl ToolMessageBuckets {
    /// Flatten in the correct order.
    ///
    /// | Path         | Flatten order                                                   |
    /// |--------------|-----------------------------------------------------------------|
    /// | Success, NonMcp | `pre_hook, tool_result, post_hook, new_messages, prevent`    |
    /// | Success, Mcp    | `pre_hook, tool_result, new_messages, prevent, post_hook`    |
    /// | Failure         | `pre_hook, tool_result, post_hook_failure` (prevent ignored) |
    /// | EarlyReturn     | `pre_hook, tool_result`                                      |
    ///
    /// # Panics
    ///
    /// Debug-panics if `tool_result` is `None` — the runner MUST set it
    /// before flattening. In release builds a missing `tool_result` is
    /// silently skipped (defensive: better to lose a single call's
    /// result than corrupt the whole batch).
    pub(crate) fn flatten(self, order: ToolMessageOrder) -> Vec<Message> {
        let Self {
            pre_hook,
            tool_result,
            new_messages,
            post_hook,
            prevent_continuation_attachment,
            path,
        } = self;

        debug_assert!(
            tool_result.is_some(),
            "ToolMessageBuckets::flatten called without a tool_result — I1 \
             requires every committed call to produce one result"
        );

        let mut out =
            Vec::with_capacity(pre_hook.len() + 1 + new_messages.len() + post_hook.len() + 1);
        out.extend(pre_hook);

        if let Some(result) = tool_result {
            out.push(result);
        }

        match path {
            ToolMessagePath::Success => match order {
                ToolMessageOrder::NonMcp => {
                    // Non-MCP: post_hook messages emit INLINE between
                    // tool_result and new_messages.
                    out.extend(post_hook);
                    out.extend(new_messages);
                    if let Some(prevent) = prevent_continuation_attachment {
                        out.push(prevent);
                    }
                }
                ToolMessageOrder::Mcp => {
                    // MCP: post_hook DEFERRED to after new_messages + prevent.
                    out.extend(new_messages);
                    if let Some(prevent) = prevent_continuation_attachment {
                        out.push(prevent);
                    }
                    out.extend(post_hook);
                }
            },
            ToolMessagePath::Failure => {
                // Returns `[error tool_result, ...hookMessages]`. The
                // success-block prevent append is bypassed on exception;
                // MCP defer does not apply.
                debug_assert!(
                    prevent_continuation_attachment.is_none(),
                    "Failure path must never carry a prevent_continuation \
                     attachment — success-block append is bypassed on exception"
                );
                out.extend(post_hook);
            }
            ToolMessagePath::EarlyReturn => {
                // No post-hook ran; prevent requires a successful
                // pre-hook → execute path. Just pre_hook + tool_result.
                debug_assert!(
                    post_hook.is_empty(),
                    "EarlyReturn path must not carry post-hook messages"
                );
                debug_assert!(
                    prevent_continuation_attachment.is_none(),
                    "EarlyReturn path must never carry a prevent_continuation \
                     attachment"
                );
            }
        }

        out
    }
}

#[cfg(test)]
#[path = "tool_message.test.rs"]
mod tests;
