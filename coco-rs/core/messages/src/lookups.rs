//! Pre-computed O(1) lookup maps for message collections.
//!
//! Builds indices to avoid O(n²) scans per render cycle.

use crate::AssistantContent;
use crate::LlmMessage;
use crate::Message;
use std::collections::HashMap;
use uuid::Uuid;

/// Pre-computed lookup indices for a message list.
#[derive(Debug, Default)]
pub struct MessageLookups {
    /// tool_call_id → tool_use_id of sibling tool calls in the same assistant message.
    pub sibling_tool_use_ids: HashMap<String, Vec<String>>,
    /// tool_use_id → index of corresponding tool result message.
    pub tool_result_ids: HashMap<String, usize>,
    /// tool_use_id → indices of progress messages.
    pub progress_by_tool_use: HashMap<String, Vec<usize>>,
    /// Message UUID → index.
    pub message_by_uuid: HashMap<Uuid, usize>,
}

/// Build lookup maps from a message list.
/// Call once after message list changes, then use for O(1) lookups.
pub fn build_message_lookups(messages: &[Message]) -> MessageLookups {
    let mut lookups = MessageLookups::default();

    for (i, msg) in messages.iter().enumerate() {
        // Index by UUID
        if let Some(uuid) = msg.uuid() {
            lookups.message_by_uuid.insert(*uuid, i);
        }

        match msg {
            Message::Assistant(a) => {
                // Collect sibling tool call IDs from this assistant message
                let tool_call_ids: Vec<String> = match &a.message {
                    LlmMessage::Assistant { content, .. } => content
                        .iter()
                        .filter_map(|c| match c {
                            AssistantContent::ToolCall(tc) => Some(tc.tool_call_id.clone()),
                            _ => None,
                        })
                        .collect(),
                    _ => vec![],
                };
                for id in &tool_call_ids {
                    lookups
                        .sibling_tool_use_ids
                        .insert(id.clone(), tool_call_ids.clone());
                }
            }
            Message::ToolResult(tr) => {
                lookups.tool_result_ids.insert(tr.tool_use_id.clone(), i);
            }
            Message::Progress(p) => {
                lookups
                    .progress_by_tool_use
                    .entry(p.tool_use_id.clone())
                    .or_default()
                    .push(i);
            }
            _ => {}
        }
    }

    lookups
}

#[cfg(test)]
#[path = "lookups.test.rs"]
mod tests;
