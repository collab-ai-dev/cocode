//! Multi-turn conversation builders for integration tests.

use coco_messages::Message;
use coco_types::ToolName;
use uuid::Uuid;

use crate::messages;

/// Build a simple alternating user→assistant conversation.
pub fn simple(turns: usize) -> Vec<Message> {
    let mut msgs = Vec::with_capacity(turns * 2);
    for i in 0..turns {
        msgs.push(messages::user(&format!("User message {}", i + 1)));
        msgs.push(messages::assistant(&format!(
            "Assistant response {}",
            i + 1
        )));
    }
    msgs
}

/// Build an agentic conversation: 1 user message, then N assistant rounds
/// (each with a unique UUID) with tool_result pairs between them.
///
/// This is the key pattern for testing API-round grouping — single user prompt
/// but multiple assistant rounds with different UUIDs.
pub fn agentic(rounds: usize) -> Vec<Message> {
    let mut msgs = Vec::with_capacity(1 + rounds * 3);
    msgs.push(messages::user("Implement the feature"));

    for i in 0..rounds {
        let uuid = Uuid::new_v4();
        msgs.push(messages::assistant_with_uuid(
            &format!("Working on step {}", i + 1),
            uuid,
        ));
        msgs.push(messages::tool_result(
            ToolName::Bash,
            &format!("call_{i}"),
            &format!("output from step {}", i + 1),
        ));
    }
    msgs
}

/// Build a conversation with N tool result rounds of a specific size.
///
/// Produces: user → (assistant → tool_result) × count
pub fn with_tool_results(tool: ToolName, count: usize, content_size: usize) -> Vec<Message> {
    let mut msgs = Vec::with_capacity(1 + count * 2);
    msgs.push(messages::user("Process these items"));

    for i in 0..count {
        let tool_call_id = format!("call_{tool}_{i}", tool = tool.as_str());
        msgs.push(messages::assistant_with_tool_call_id(
            &tool_call_id,
            tool.as_str(),
            serde_json::json!({"id": i}),
        ));
        msgs.push(messages::tool_result_large(
            tool,
            &tool_call_id,
            content_size,
        ));
    }
    msgs
}

/// Build a conversation with approximately `target_tokens` total tokens.
///
/// Each user+assistant pair is ~200 tokens. Adds pairs until target reached.
pub fn large(target_tokens: i64) -> Vec<Message> {
    let tokens_per_pair = 200; // ~400 chars user + ~400 chars assistant
    let pairs = ((target_tokens / tokens_per_pair) as usize).max(1);
    let mut msgs = Vec::with_capacity(pairs * 2);

    for i in 0..pairs {
        let text = format!(
            "Message {i} with padding: {}",
            "lorem ipsum dolor sit amet ".repeat(5)
        );
        msgs.push(messages::user(&text));
        msgs.push(messages::assistant(&text));
    }
    msgs
}
