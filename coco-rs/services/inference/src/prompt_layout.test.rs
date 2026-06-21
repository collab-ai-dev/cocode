use coco_llm_types::LlmMessage;
use coco_llm_types::ProviderOptions;
use coco_llm_types::TextPart;
use coco_llm_types::UserContentPart;
use coco_types::ProviderApi;
use pretty_assertions::assert_eq;
use vercel_ai_provider::Warning;

use super::*;

fn sys(text: &str) -> LlmMessage {
    LlmMessage::System {
        content: vec![UserContentPart::Text(TextPart {
            text: text.to_string(),
            provider_metadata: None,
        })],
        provider_options: None,
    }
}

fn user(text: &str) -> LlmMessage {
    LlmMessage::User {
        content: vec![UserContentPart::Text(TextPart {
            text: text.to_string(),
            provider_metadata: None,
        })],
        provider_options: None,
    }
}

#[test]
fn test_put_take_layout_options_round_trips_namespace_fields() {
    let layout = PromptLayoutOptions {
        instructions: Some("base".to_string()),
        system_blocks: Some(vec![AnthropicSystemBlock {
            text: "system".to_string(),
            cache_control: Some(AnthropicCacheControl {
                type_name: "ephemeral".to_string(),
                ttl: Some("5m".to_string()),
            }),
        }]),
        system_instruction: Some("gemini".to_string()),
        layout_warnings: vec![Warning::other("dropped part")],
        prompt_hash_inputs: Some(PromptHashInputs {
            system_text_hash: 1,
            cache_control_hash: Some(2),
            developer_text_hash: Some(3),
            contextual_user_text_hash: 4,
            contextual_user_char_count: 5,
            tools_hash: 6,
            per_tool_hashes: vec![("read".to_string(), 7)],
        }),
    };

    let mut opts = ProviderOptions::default();
    put_layout_options(&mut opts, &layout);

    let namespace = opts
        .get(PROMPT_LAYOUT_NAMESPACE)
        .expect("prompt layout namespace");
    assert!(namespace.contains_key("instructions"));
    assert!(namespace.contains_key("system_blocks"));
    assert!(namespace.contains_key("system_instruction"));
    assert!(namespace.contains_key("layout_warnings"));
    assert!(namespace.contains_key("prompt_hash_inputs"));

    assert_eq!(take_layout_options(&opts), Some(layout));
}

#[test]
fn test_put_layout_options_omits_absent_slots() {
    let layout = PromptLayoutOptions {
        instructions: Some("base".to_string()),
        ..Default::default()
    };

    let mut opts = ProviderOptions::default();
    put_layout_options(&mut opts, &layout);

    let namespace = opts
        .get(PROMPT_LAYOUT_NAMESPACE)
        .expect("prompt layout namespace");
    assert!(namespace.contains_key("instructions"));
    assert!(!namespace.contains_key("system_blocks"));
    assert!(!namespace.contains_key("system_instruction"));
    assert!(!namespace.contains_key("prompt_hash_inputs"));
}

#[test]
fn test_build_layout_openai_routes_system_to_instructions_only() {
    let prompt = vec![sys("you are coco"), user("hi")];
    let layout = build_prompt_layout_from_prompt(&prompt, ProviderApi::Openai, None);
    assert_eq!(layout.instructions.as_deref(), Some("you are coco"));
    assert!(layout.system_blocks.is_none());
    assert!(layout.system_instruction.is_none());
    let hashes = layout.prompt_hash_inputs.expect("hash inputs");
    assert!(hashes.system_text_hash != 0);
    assert!(hashes.contextual_user_text_hash != 0);
    assert!(hashes.contextual_user_char_count > 0);
}

#[test]
fn test_build_layout_anthropic_routes_to_system_blocks_only() {
    let prompt = vec![sys("you are coco"), user("hi")];
    let layout = build_prompt_layout_from_prompt(&prompt, ProviderApi::Anthropic, None);
    assert!(layout.instructions.is_none());
    let blocks = layout.system_blocks.expect("system_blocks");
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].text, "you are coco");
    assert!(blocks[0].cache_control.is_none());
    assert!(layout.system_instruction.is_none());
}

#[test]
fn test_build_layout_gemini_routes_to_system_instruction_only() {
    let prompt = vec![sys("you are coco"), user("hi")];
    let layout = build_prompt_layout_from_prompt(&prompt, ProviderApi::Gemini, None);
    assert!(layout.instructions.is_none());
    assert!(layout.system_blocks.is_none());
    assert_eq!(layout.system_instruction.as_deref(), Some("you are coco"));
}

#[test]
fn test_build_layout_compat_provider_skips_top_level_slots() {
    // OpenAI-compatible providers don't carry a separate top-level
    // slot — the System message stays in the chat stream and the
    // namespace omits all three slots.
    let prompt = vec![sys("ident"), user("hi")];
    let layout = build_prompt_layout_from_prompt(&prompt, ProviderApi::OpenaiCompat, None);
    assert!(layout.instructions.is_none());
    assert!(layout.system_blocks.is_none());
    assert!(layout.system_instruction.is_none());
    // Hash inputs are still populated so the cache detector has the
    // same view across all providers.
    assert!(layout.prompt_hash_inputs.is_some());
}

#[test]
fn test_build_layout_empty_system_text_skips_slot_population() {
    let prompt = vec![user("just user text")];
    let layout = build_prompt_layout_from_prompt(&prompt, ProviderApi::Openai, None);
    assert!(layout.instructions.is_none());
    let hashes = layout.prompt_hash_inputs.expect("hash inputs");
    assert_eq!(hashes.system_text_hash, djb2_hash(b""));
    assert!(hashes.contextual_user_text_hash != 0);
}
