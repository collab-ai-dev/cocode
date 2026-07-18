//! The sidechat structural read-only tool boundary.
//!
//! A sidechat child may direct only read-only tools: builtin `Read`, `Glob`,
//! `Grep`, and `Bash` commands accepted by `coco_shell::read_only`. The gate
//! keys off the resolved [`ToolId`] (never the wire string, which could be an
//! alias), and returns `Ask` for permitted reads — deliberately NOT `Allow`,
//! so the tool's own permission evaluator still runs (explicit denies,
//! sensitive-path checks, approval prompts). Every other tool — mutating
//! builtins, MCP, custom, unknown — is denied before execution.
//!
//! Installed with `require_can_use_tool = true` so a `PreToolUse` hook that
//! auto-approves cannot bypass this boundary. See
//! `docs/internal/sidechat-architecture.md` §7.

use std::sync::Arc;

use async_trait::async_trait;
use coco_tool_runtime::{
    CanUseToolCallContext, CanUseToolDecision, CanUseToolHandle, CanUseToolHandleRef,
    DecisionReason,
};
use coco_types::{ToolId, ToolName};
use serde_json::Value;

/// Build the sidechat read-only tool gate.
pub fn side_chat_read_only_handle() -> CanUseToolHandleRef {
    Arc::new(SideChatReadOnlyHandle)
}

#[derive(Debug, Clone, Copy, Default)]
struct SideChatReadOnlyHandle;

impl SideChatReadOnlyHandle {
    /// Permitted read: fall through to the tool's built-in permission checks.
    fn ask(reason: &'static str) -> CanUseToolDecision {
        CanUseToolDecision::Ask {
            decision_reason: DecisionReason::Other {
                reason: reason.into(),
            },
        }
    }

    /// Blocked: short-circuit with a denial the model can read.
    fn deny(message: &'static str) -> CanUseToolDecision {
        CanUseToolDecision::Deny {
            message: message.into(),
            decision_reason: DecisionReason::Other {
                reason: "sidechat_read_only".into(),
            },
        }
    }
}

#[async_trait]
impl CanUseToolHandle for SideChatReadOnlyHandle {
    async fn check(
        &self,
        tool_id: &ToolId,
        _tool_name: &str,
        input: &Value,
        _ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        match tool_id {
            ToolId::Builtin(ToolName::Read | ToolName::Glob | ToolName::Grep) => {
                Self::ask("sidechat read-only: read tool permitted; normal permissions still apply")
            }
            ToolId::Builtin(ToolName::Bash) => {
                let command = input
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if coco_shell::read_only::is_read_only_command(command) {
                    Self::ask("sidechat read-only: read-only Bash permitted")
                } else {
                    Self::deny(
                        "sidechat is read-only: only read-only Bash commands are allowed here. \
                         Ask in the main conversation to run mutating commands.",
                    )
                }
            }
            _ => Self::deny(
                "sidechat is read-only: only Read, Glob, Grep, and read-only Bash are available. \
                 Ask in the main conversation for anything else.",
            ),
        }
    }
}

#[cfg(test)]
#[path = "side_chat_tool_gate.test.rs"]
mod tests;
