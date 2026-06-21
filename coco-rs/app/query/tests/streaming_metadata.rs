//! End-to-end test that per-part `provider_metadata` survives the
//! streaming path into persisted assistant history.
//!
//! The bug this guards against: prior to plan v6, the streaming
//! reconstruction at `engine.rs:1521,1544` hardcoded `provider_metadata: None`
//! on `ToolCallPart` and `TextPart`. Gemini-3 `thoughtSignature`, which
//! rides on `ToolCall` (and sometimes `Text`) parts, was lost — the
//! next turn's request omitted the signature, breaking Gemini-3's
//! thinking continuity.
//!
//! The fix routes per-part metadata through `AssistantTurnSnapshot`
//! and rebuilds the assistant message from it at `StreamEvent::Finish`.
//! These tests fail on the pre-fix engine and pass on the fixed one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

use coco_inference::AISdkError;
use coco_inference::LanguageModel;
use coco_inference::LanguageModelCallOptions;
use coco_inference::LanguageModelGenerateResult;
use coco_inference::LanguageModelStreamResult;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::FinishReason;
use coco_llm_types::ProviderMetadata;
use coco_llm_types::ReasoningPart;
use coco_llm_types::StopReason;
use coco_llm_types::TextPart;
use coco_llm_types::ToolCallPart;
use coco_llm_types::Usage;
use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_query::QueryEngine;
use coco_query::QueryEngineConfig;
use coco_tool_runtime::ToolRegistry;
use coco_types::PermissionMode;
use tokio_util::sync::CancellationToken;

/// Construct a single-key `ProviderMetadata` (e.g. `google.thoughtSignature: T1`).
fn meta(provider: &str, key: &str, value: &str) -> ProviderMetadata {
    let mut outer: HashMap<String, serde_json::Value> = HashMap::new();
    outer.insert(provider.into(), serde_json::json!({ key: value }));
    ProviderMetadata::from_map(outer)
}

fn read_meta(pm: &ProviderMetadata, provider: &str, key: &str) -> Option<String> {
    pm.0.get(provider)?
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// A `LanguageModel` mock that returns a single, fully-scripted assistant
/// turn whose `AssistantContentPart`s carry caller-supplied
/// `provider_metadata`. Used to verify metadata survives the streaming
/// path into history.
struct MetadataMock {
    content: Vec<AssistantContentPart>,
    call_count: AtomicI32,
}

impl MetadataMock {
    fn new(content: Vec<AssistantContentPart>) -> Self {
        Self {
            content,
            call_count: AtomicI32::new(0),
        }
    }
}

#[async_trait::async_trait]
impl LanguageModel for MetadataMock {
    fn provider(&self) -> &str {
        "metadata-mock"
    }
    fn model_id(&self) -> &str {
        "metadata-mock-1"
    }
    async fn do_generate(
        &self,
        _options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelGenerateResult, AISdkError> {
        // Turn 1: scripted multi-part content. Turn 2+: a short text
        // reply so the agent loop terminates without hitting the
        // "no more responses" sentinel.
        let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
        let content = if idx == 0 {
            self.content.clone()
        } else {
            vec![AssistantContentPart::Text(TextPart {
                text: "done".into(),
                provider_metadata: None,
            })]
        };
        let finish = if idx == 0
            && content
                .iter()
                .any(|p| matches!(p, AssistantContentPart::ToolCall(_)))
        {
            StopReason::ToolUse
        } else {
            StopReason::EndTurn
        };
        Ok(LanguageModelGenerateResult {
            content,
            usage: Usage::new(10, 5),
            finish_reason: FinishReason::new(finish),
            warnings: vec![],
            provider_metadata: None,
            request: None,
            response: None,
        })
    }
    async fn do_stream(
        &self,
        options: &LanguageModelCallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelStreamResult, AISdkError> {
        let result = self.do_generate(options, None).await?;
        Ok(coco_inference::synthetic_stream_from_content(
            result.content,
            result.usage,
            result.finish_reason,
        ))
    }
}

/// Run a single engine turn with the given mock content and return the
/// persisted assistant message's `AssistantContent` vector for inspection.
async fn capture_assistant_content(content: Vec<AssistantContentPart>) -> Vec<AssistantContent> {
    let model = Arc::new(MetadataMock::new(content));
    let client = coco_query::test_support::model_runtime_registry(model);
    let tools = Arc::new(ToolRegistry::new());
    let cancel = CancellationToken::new();
    let config = QueryEngineConfig {
        model_id: "metadata-mock-1".into(),
        permission_mode: PermissionMode::BypassPermissions,
        max_turns: Some(2),
        ..Default::default()
    };
    let engine = QueryEngine::new(config, client, tools, cancel, None);
    let result = engine.run("hello").await.unwrap();

    // Find the first persisted assistant message in history.
    result
        .final_messages
        .into_iter()
        .find_map(|m| match m.as_ref() {
            Message::Assistant(a) => match &a.message {
                LlmMessage::Assistant { content, .. } => Some(content.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("assistant message expected in history")
}

/// Reasoning + ToolCall both carry distinct signatures round-trip.
#[tokio::test]
async fn metadata_survives_reasoning_and_tool_call() {
    let reasoning_meta = meta("anthropic", "signature", "S1");
    let tool_meta = meta("google", "thoughtSignature", "T1");
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "thinking...".into(),
            provider_metadata: Some(reasoning_meta.clone()),
        }),
        AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "call_1".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            provider_executed: None,
            provider_metadata: Some(tool_meta.clone()),
            invalid: false,
            invalid_reason: None,
        }),
    ];

    let persisted = capture_assistant_content(content).await;
    // Reasoning part metadata.
    let reasoning_found = persisted.iter().any(|c| match c {
        AssistantContent::Reasoning(r) => {
            r.provider_metadata
                .as_ref()
                .and_then(|pm| read_meta(pm, "anthropic", "signature"))
                == Some("S1".into())
        }
        _ => false,
    });
    assert!(
        reasoning_found,
        "Reasoning.provider_metadata['anthropic']['signature']==S1 missing in persisted: {persisted:#?}"
    );
    // ToolCall part metadata.
    let tool_found = persisted.iter().any(|c| match c {
        AssistantContent::ToolCall(tc) => {
            tc.provider_metadata
                .as_ref()
                .and_then(|pm| read_meta(pm, "google", "thoughtSignature"))
                == Some("T1".into())
        }
        _ => false,
    });
    assert!(
        tool_found,
        "ToolCall.provider_metadata['google']['thoughtSignature']==T1 missing in persisted: {persisted:#?}"
    );
}

/// Text-only assistant message carries its signature.
#[tokio::test]
async fn metadata_survives_text_only_turn() {
    let text_meta = meta("google", "thoughtSignature", "Tx");
    let content = vec![AssistantContentPart::Text(TextPart {
        text: "Hello.".into(),
        provider_metadata: Some(text_meta.clone()),
    })];

    let persisted = capture_assistant_content(content).await;
    let text_found = persisted.iter().any(|c| match c {
        AssistantContent::Text(t) => {
            t.provider_metadata
                .as_ref()
                .and_then(|pm| read_meta(pm, "google", "thoughtSignature"))
                == Some("Tx".into())
        }
        _ => false,
    });
    assert!(
        text_found,
        "Text.provider_metadata['google']['thoughtSignature']==Tx missing: {persisted:#?}"
    );
}

/// Interleaved order is preserved: persisted content has Text → ToolCall → Text
/// in that exact sequence. The pre-fix reconstruction always emitted
/// `[Reasoning?, Text(combined), ToolCall*]` regardless of original order.
#[tokio::test]
async fn order_is_preserved_for_text_tool_text_interleaving() {
    let content = vec![
        AssistantContentPart::Text(TextPart {
            text: "before".into(),
            provider_metadata: Some(meta("google", "thoughtSignature", "M1")),
        }),
        AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "call_1".into(),
            tool_name: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            provider_executed: None,
            provider_metadata: Some(meta("google", "thoughtSignature", "MT")),
            invalid: false,
            invalid_reason: None,
        }),
        AssistantContentPart::Text(TextPart {
            text: "after".into(),
            provider_metadata: Some(meta("google", "thoughtSignature", "M2")),
        }),
    ];

    let persisted = capture_assistant_content(content).await;
    // Filter down to the load-bearing parts and check shape + ordering.
    let kinds: Vec<&'static str> = persisted
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(_) => Some("text"),
            AssistantContent::ToolCall(_) => Some("tool"),
            AssistantContent::Reasoning(_) => Some("reasoning"),
            _ => None,
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["text", "tool", "text"],
        "interleaved Text→ToolCall→Text not preserved: {persisted:#?}"
    );

    // Both Text parts must carry their own distinct metadata.
    let texts: Vec<_> = persisted
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => t
                .provider_metadata
                .as_ref()
                .and_then(|pm| read_meta(pm, "google", "thoughtSignature")),
            _ => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec!["M1", "M2"],
        "per-segment text metadata lost: {persisted:#?}"
    );
}

/// Two reasoning segments with distinct signatures both survive.
/// Anthropic interleaved thinking and OpenAI Responses multi-item
/// reasoning produce this shape.
#[tokio::test]
async fn multiple_reasoning_segments_preserve_individual_signatures() {
    let content = vec![
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "first thought".into(),
            provider_metadata: Some(meta("anthropic", "signature", "S1")),
        }),
        AssistantContentPart::Reasoning(ReasoningPart {
            text: "second thought".into(),
            provider_metadata: Some(meta("anthropic", "signature", "S2")),
        }),
        AssistantContentPart::Text(TextPart {
            text: "final answer".into(),
            provider_metadata: None,
        }),
    ];

    let persisted = capture_assistant_content(content).await;
    let signatures: Vec<_> = persisted
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Reasoning(r) => r
                .provider_metadata
                .as_ref()
                .and_then(|pm| read_meta(pm, "anthropic", "signature")),
            _ => None,
        })
        .collect();
    assert_eq!(
        signatures,
        vec!["S1", "S2"],
        "multi-reasoning signatures lost or merged: {persisted:#?}"
    );
}
