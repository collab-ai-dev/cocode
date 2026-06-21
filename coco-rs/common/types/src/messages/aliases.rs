//! Version-agnostic LLM type aliases — shielded through `coco-llm-types`.
//!
//! All Message-family structs in this `types/` tree reference these aliases;
//! never reach for `vercel_ai_provider` directly. Upgrading the underlying
//! SDK only requires changing `common/llm-types/src/lib.rs` — this file
//! stays unchanged.

pub use coco_llm_types::AssistantContentPart as AssistantContent;
pub use coco_llm_types::DataContent;
pub use coco_llm_types::FilePart as FileContent;
pub use coco_llm_types::LlmMessage;
pub use coco_llm_types::LlmPrompt;
pub use coco_llm_types::ReasoningPart as ReasoningContent;
pub use coco_llm_types::TextPart as TextContent;
pub use coco_llm_types::ToolCallPart as ToolCallContent;
pub use coco_llm_types::ToolContentPart as ToolContent;
pub use coco_llm_types::ToolResultContent as ToolResultOutput;
pub use coco_llm_types::ToolResultContentPart;
pub use coco_llm_types::ToolResultPart as ToolResultContent;
pub use coco_llm_types::UserContentPart as UserContent;

/// Construct a `ToolResultContentPart` that carries an Anthropic
/// `tool_reference` content block. The Anthropic API server expands
/// the block into inline `<functions>...</functions>` markup before
/// the prompt reaches the model — the client-side `tools` array is
/// not modified, preserving prompt-cache prefix.
///
/// Encoded via the `Custom` variant + `provider_options.anthropic`
/// shape that `vercel-ai-anthropic::convert_tool_result_content_part`
/// already recognizes:
/// ```json
/// { "type": "custom",
///   "providerOptions": {
///     "anthropic": {
///       "type": "tool-reference",
///       "toolName": "WebFetch"
///     }
///   }}
/// ```
///
/// Non-Anthropic providers will emit a `Warning::Other` and skip the
/// block; callers must gate on the model's
/// `Capability::ServerSideToolReference` before using this builder.
///
/// Skipping multi-provider concerns is intentional —
/// only the Anthropic adapter knows how to unwrap them, and the
/// engine arranges to call this helper only when capable.
#[must_use]
pub fn tool_reference_content_part(tool_name: impl Into<String>) -> ToolResultContentPart {
    use std::collections::HashMap;
    let mut anthropic = HashMap::new();
    anthropic.insert(
        "type".to_string(),
        serde_json::Value::String("tool-reference".to_string()),
    );
    anthropic.insert(
        "toolName".to_string(),
        serde_json::Value::String(tool_name.into()),
    );
    let mut map = HashMap::new();
    map.insert("anthropic".to_string(), anthropic);
    ToolResultContentPart::Custom {
        provider_options: Some(coco_llm_types::ProviderOptions(map)),
    }
}
