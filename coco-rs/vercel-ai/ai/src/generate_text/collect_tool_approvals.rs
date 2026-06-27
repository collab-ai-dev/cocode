//! Collect tool approvals.
//!
//! This module provides functionality for collecting tool approvals
//! when tools require explicit user confirmation before execution.

use std::sync::Arc;

use crate::error::InvalidToolApprovalError;
use crate::types::ToolRegistry;

use super::generate_text_result::ToolCall;

/// Tool approval status.
#[derive(Debug, Clone)]
pub enum ToolApprovalStatus {
    /// Tool is approved for execution.
    Approved,
    /// Tool is denied - will not be executed.
    Denied {
        /// Reason for denial.
        reason: Option<String>,
    },
    /// Tool requires modification before execution.
    Modified {
        /// Modified tool call.
        tool_call: ToolCall,
    },
}

/// A tool approval request.
#[derive(Debug, Clone)]
pub struct ToolApprovalRequest {
    /// The tool call requiring approval.
    pub tool_call: ToolCall,
    /// The tool definition (if available).
    pub tool_description: Option<String>,
}

impl ToolApprovalRequest {
    /// Create a new approval request.
    pub fn new(tool_call: ToolCall) -> Self {
        Self {
            tool_call,
            tool_description: None,
        }
    }

    /// Add a tool description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.tool_description = Some(description.into());
        self
    }
}

/// A collected tool approval.
#[derive(Debug, Clone)]
pub struct ToolApproval {
    /// The tool call ID.
    pub tool_call_id: String,
    /// The approval status.
    pub status: ToolApprovalStatus,
}

impl ToolApproval {
    /// Create an approved tool approval.
    pub fn approved(tool_call_id: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            status: ToolApprovalStatus::Approved,
        }
    }

    /// Create a denied tool approval.
    pub fn denied(tool_call_id: impl Into<String>, reason: Option<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            status: ToolApprovalStatus::Denied { reason },
        }
    }

    /// Create a modified tool approval.
    pub fn modified(tool_call_id: impl Into<String>, tool_call: ToolCall) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            status: ToolApprovalStatus::Modified { tool_call },
        }
    }

    /// Check if the approval is approved.
    pub fn is_approved(&self) -> bool {
        matches!(self.status, ToolApprovalStatus::Approved)
    }

    /// Check if the approval is denied.
    pub fn is_denied(&self) -> bool {
        matches!(self.status, ToolApprovalStatus::Denied { .. })
    }
}

/// Trait for collecting tool approvals.
#[async_trait::async_trait]
pub trait ToolApprovalCollector: Send + Sync {
    /// Collect approvals for the given tool calls.
    ///
    /// # Arguments
    ///
    /// * `requests` - The tool approval requests.
    ///
    /// # Returns
    ///
    /// A vector of tool approvals.
    async fn collect_approvals(
        &self,
        requests: Vec<ToolApprovalRequest>,
    ) -> Result<Vec<ToolApproval>, InvalidToolApprovalError>;
}

/// A simple approval collector that auto-approves all tools.
#[derive(Default)]
pub struct AutoApproveCollector;

#[async_trait::async_trait]
impl ToolApprovalCollector for AutoApproveCollector {
    async fn collect_approvals(
        &self,
        requests: Vec<ToolApprovalRequest>,
    ) -> Result<Vec<ToolApproval>, InvalidToolApprovalError> {
        Ok(requests
            .into_iter()
            .map(|req| ToolApproval::approved(req.tool_call.tool_call_id))
            .collect())
    }
}

/// A collector that prompts for approval.
pub struct PromptApprovalCollector {
    /// A function that prompts the user for approval.
    prompt_fn: Arc<dyn Fn(Vec<ToolApprovalRequest>) -> Vec<ToolApproval> + Send + Sync>,
}

impl PromptApprovalCollector {
    /// Create a new prompt approval collector.
    pub fn new<F>(prompt_fn: F) -> Self
    where
        F: Fn(Vec<ToolApprovalRequest>) -> Vec<ToolApproval> + Send + Sync + 'static,
    {
        Self {
            prompt_fn: Arc::new(prompt_fn),
        }
    }
}

#[async_trait::async_trait]
impl ToolApprovalCollector for PromptApprovalCollector {
    async fn collect_approvals(
        &self,
        requests: Vec<ToolApprovalRequest>,
    ) -> Result<Vec<ToolApproval>, InvalidToolApprovalError> {
        Ok((self.prompt_fn)(requests))
    }
}

/// Collect tool approvals for tool calls.
///
/// This function checks if tools require approval and collects
/// approvals from the provided collector.
///
/// # Arguments
///
/// * `tool_calls` - The tool calls to check for approval.
/// * `tools` - The tool registry.
/// * `collector` - The approval collector.
///
/// # Returns
///
/// A vector of tool approvals.
pub async fn collect_tool_approvals(
    tool_calls: &[ToolCall],
    tools: &Arc<ToolRegistry>,
    collector: &dyn ToolApprovalCollector,
) -> Result<Vec<ToolApproval>, InvalidToolApprovalError> {
    // Build approval requests for tools that need approval
    let requests: Vec<ToolApprovalRequest> = tool_calls
        .iter()
        .filter_map(|tc| {
            // Check if tool exists
            if let Some(tool) = tools.get(&tc.tool_name) {
                let desc = tool.definition().description.clone();
                Some(
                    ToolApprovalRequest::new(tc.clone()).with_description(desc.unwrap_or_default()),
                )
            } else {
                None
            }
        })
        .collect();

    if requests.is_empty() {
        // No tools need approval
        return Ok(tool_calls
            .iter()
            .map(|tc| ToolApproval::approved(&tc.tool_call_id))
            .collect());
    }

    collector.collect_approvals(requests).await
}

/// Check if all approvals are approved.
pub fn all_approved(approvals: &[ToolApproval]) -> bool {
    approvals.iter().all(ToolApproval::is_approved)
}

/// Get denied approvals.
pub fn get_denied_approvals(approvals: &[ToolApproval]) -> Vec<&ToolApproval> {
    approvals.iter().filter(|a| a.is_denied()).collect()
}

/// Apply approvals to tool calls.
///
/// This function filters and modifies tool calls based on approvals.
/// Denied tools are removed, modified tools are updated.
pub fn apply_approvals(tool_calls: Vec<ToolCall>, approvals: &[ToolApproval]) -> Vec<ToolCall> {
    let approval_map: std::collections::HashMap<&str, &ToolApproval> = approvals
        .iter()
        .map(|a| (a.tool_call_id.as_str(), a))
        .collect();

    tool_calls
        .into_iter()
        .filter_map(|tc| {
            let approval = approval_map.get(tc.tool_call_id.as_str())?;
            match &approval.status {
                ToolApprovalStatus::Approved => Some(tc),
                ToolApprovalStatus::Denied { .. } => None,
                ToolApprovalStatus::Modified { tool_call } => Some(tool_call.clone()),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "collect_tool_approvals.test.rs"]
mod tests;
