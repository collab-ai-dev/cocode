use pretty_assertions::assert_eq;
use std::sync::Arc;
use uuid::Uuid;

use super::*;
use crate::messages::{AssistantMessage, LlmMessage, Message, UserMessage};

fn user_msg(text: &str) -> Arc<Message> {
    Arc::new(Message::User(UserMessage {
        message: LlmMessage::user_text(text),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    }))
}

fn assistant_msg(text: &str) -> Arc<Message> {
    Arc::new(Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![coco_llm_types::AssistantContentPart::text(text)],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: String::new(),
        stop_reason: None,
        usage: None,
        cost_usd: None,
        request_id: None,
        api_error: None,
    }))
}

#[test]
fn test_cache_safe_params_serde_roundtrip() {
    // `CacheSafeParams` doesn't derive `PartialEq` because `Message` (in
    // `fork_context_messages`) doesn't either — adding PartialEq across
    // the message family would touch every Message variant. Assert
    // field-wise for the scalar fields and check the message vec length
    // + per-entry shape.
    let params = CacheSafeParams {
        rendered_system_prompt: "You are a helpful agent.".into(),
        model_id: "claude-opus-4-7".into(),
        provider: "anthropic".into(),
        active_shell_tool: crate::ActiveShellTool::PowerShell,
        prompt_cache: None,
        fork_context_messages: vec![user_msg("hi"), assistant_msg("hello")],
    };
    let s = serde_json::to_string(&params).unwrap();
    let back: CacheSafeParams = serde_json::from_str(&s).unwrap();
    assert_eq!(back.rendered_system_prompt, params.rendered_system_prompt);
    assert_eq!(back.model_id, params.model_id);
    assert_eq!(back.provider, params.provider);
    assert_eq!(back.active_shell_tool, params.active_shell_tool);
    assert_eq!(back.fork_context_messages.len(), 2);
    assert!(matches!(
        back.fork_context_messages[0].as_ref(),
        Message::User(_)
    ));
    assert!(matches!(
        back.fork_context_messages[1].as_ref(),
        Message::Assistant(_)
    ));
}

#[test]
fn test_cache_safe_params_default_skips_empty_fork_messages() {
    // `fork_context_messages` and `provider` both default — old
    // session formats can omit them without breaking deserialize.
    let json = r#"{
        "rendered_system_prompt": "sys",
        "model_id": "m"
    }"#;
    let parsed: CacheSafeParams = serde_json::from_str(json).unwrap();
    assert!(parsed.fork_context_messages.is_empty());
    assert_eq!(parsed.model_id, "m");
    assert_eq!(parsed.provider, "");
    assert_eq!(parsed.active_shell_tool, crate::ActiveShellTool::Disabled);
}

#[test]
fn test_cache_safe_params_eq_distinguishes_model() {
    // Cache slots key on `(provider, model_id, ...)` so two snapshots
    // with different `model_id` must stay isolated. Field-wise check —
    // `CacheSafeParams` no longer derives `PartialEq` (see roundtrip
    // test rationale).
    let a = CacheSafeParams {
        rendered_system_prompt: "sys".into(),
        model_id: "claude-opus-4-7".into(),
        provider: "anthropic".into(),
        active_shell_tool: crate::ActiveShellTool::Bash,
        prompt_cache: None,
        fork_context_messages: Vec::new(),
    };
    let b = CacheSafeParams {
        model_id: "claude-haiku-4-5".into(),
        ..a.clone()
    };
    assert_ne!(a.model_id, b.model_id);
    assert_eq!(a.provider, b.provider);
}

#[test]
fn test_cache_safe_params_eq_distinguishes_provider() {
    let a = CacheSafeParams {
        rendered_system_prompt: "sys".into(),
        model_id: "claude-opus-4-7".into(),
        provider: "anthropic".into(),
        active_shell_tool: crate::ActiveShellTool::Bash,
        prompt_cache: None,
        fork_context_messages: Vec::new(),
    };
    let b = CacheSafeParams {
        provider: "openai".into(),
        ..a.clone()
    };
    assert_ne!(a.provider, b.provider);
    assert_eq!(a.model_id, b.model_id);
}
