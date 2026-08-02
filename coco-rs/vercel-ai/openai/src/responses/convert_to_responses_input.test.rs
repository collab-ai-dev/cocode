use super::*;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;
use vercel_ai_provider::ToolResultPart;

fn projected_call_id(id: &str) -> String {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: id.to_string(),
            tool_name: "test".into(),
            input: serde_json::Value::Null,
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];
    ResponsesCallIdProjector::for_prompt(&prompt)
        .project(id)
        .to_string()
}

#[test]
fn converts_system_as_developer() {
    let prompt = vec![LanguageModelV4Message::system("Be helpful")];
    let (items, warnings) =
        convert_to_openai_responses_input(&prompt, SystemMessageMode::Developer);
    assert!(warnings.is_empty());
    assert_eq!(items[0]["role"], "developer");
    assert_eq!(items[0]["content"], "Be helpful");
}

#[test]
fn from_tools_routes_apply_patch_by_id_not_name() {
    use std::collections::HashMap;
    use vercel_ai_provider::LanguageModelV4ProviderTool;

    // coco's freeform apply_patch: id "openai.custom", name "apply_patch".
    // It must be treated as a CUSTOM tool (→ custom_tool_call), NOT the
    // @ai-sdk built-in apply_patch path — routing keys on id, not name.
    let custom = LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool {
        id: "openai.custom".to_string(),
        name: "apply_patch".to_string(),
        args: HashMap::new(),
    });
    let flags = ProviderToolFlags::from_tools(&Some(vec![custom]));
    assert!(
        !flags.has_apply_patch,
        "custom (openai.custom) apply_patch must not trip the built-in path"
    );
    assert!(
        flags.custom_tool_names.contains("apply_patch"),
        "custom apply_patch must be a custom tool"
    );

    // The @ai-sdk built-in apply_patch (id "openai.apply_patch") keeps its path.
    let builtin = LanguageModelV4Tool::Provider(LanguageModelV4ProviderTool {
        id: "openai.apply_patch".to_string(),
        name: "apply_patch".to_string(),
        args: HashMap::new(),
    });
    let flags = ProviderToolFlags::from_tools(&Some(vec![builtin]));
    assert!(flags.has_apply_patch);
    assert!(flags.custom_tool_names.is_empty());
}

#[test]
fn converts_developer_message() {
    let prompt = vec![LanguageModelV4Message::developer_text("Follow app policy")];
    let (items, warnings) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert!(warnings.is_empty());
    assert_eq!(items[0]["role"], "developer");
    assert_eq!(items[0]["content"], "Follow app policy");
}

#[test]
fn converts_user_text() {
    let prompt = vec![LanguageModelV4Message::User {
        content: vec![UserContentPart::Text(TextPart {
            text: "Hello".into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert_eq!(items[0]["role"], "user");
    assert_eq!(items[0]["content"][0]["type"], "input_text");
    assert_eq!(items[0]["content"][0]["text"], "Hello");
}

#[test]
fn converts_assistant_with_tool_call() {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![
            AssistantContentPart::Text(TextPart {
                text: "Let me check".into(),
                provider_metadata: None,
            }),
            AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id: "call_1".into(),
                tool_name: "get_weather".into(),
                input: serde_json::json!({"city": "SF"}),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            }),
        ],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    // First item: assistant text message
    assert_eq!(items[0]["role"], "assistant");
    // Second item: function_call
    assert_eq!(items[1]["type"], "function_call");
    assert_eq!(items[1]["name"], "get_weather");
}

fn reasoning_meta(encrypted: &str) -> vercel_ai_provider::ProviderMetadata {
    let mut openai = serde_json::Map::new();
    openai.insert(
        "encryptedContent".into(),
        serde_json::Value::String(encrypted.into()),
    );
    let mut meta = vercel_ai_provider::ProviderMetadata::default();
    meta.0
        .insert("openai".into(), serde_json::Value::Object(openai));
    meta
}

#[test]
fn assistant_reasoning_round_trips_encrypted_content() {
    // The encrypted chain-of-thought blob must be re-sent so store=false
    // reasoning continuity survives the tool-call turn.
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "Thinking about it".into(),
                provider_metadata: Some(reasoning_meta("ENC_BLOB")),
            },
        )],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert_eq!(items[0]["type"], "reasoning");
    assert_eq!(items[0]["summary"][0]["type"], "summary_text");
    assert_eq!(items[0]["summary"][0]["text"], "Thinking about it");
    assert_eq!(items[0]["encrypted_content"], "ENC_BLOB");
}

#[test]
fn assistant_encrypted_only_reasoning_emits_empty_summary() {
    // No summary text (encrypted-only): the summary array is empty but the
    // chain blob still rides back.
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: String::new(),
                provider_metadata: Some(reasoning_meta("ENC2")),
            },
        )],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert_eq!(items[0]["type"], "reasoning");
    assert_eq!(items[0]["summary"], serde_json::json!([]));
    assert_eq!(items[0]["encrypted_content"], "ENC2");
}

#[test]
fn assistant_reasoning_without_metadata_omits_encrypted_content() {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "plain".into(),
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert_eq!(items[0]["type"], "reasoning");
    assert!(items[0].get("encrypted_content").is_none());
}

#[test]
fn assistant_raw_reasoning_content_is_stripped_on_sendback() {
    // Raw reasoning (reasoningType="text", no encrypted blob) is display-only
    // and must NOT round-trip — the server rehydrates it from encrypted_content.
    let mut openai = serde_json::Map::new();
    openai.insert(
        "reasoningType".into(),
        serde_json::Value::String("text".into()),
    );
    let mut meta = vercel_ai_provider::ProviderMetadata::default();
    meta.0
        .insert("openai".into(), serde_json::Value::Object(openai));

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "raw chain of thought".into(),
                provider_metadata: Some(meta),
            },
        )],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert!(
        items.iter().all(|it| it["type"] != "reasoning"),
        "raw reasoning content must not round-trip as input"
    );
}

#[test]
fn assistant_compaction_round_trips() {
    // Server-side compaction state must round-trip on the turn after
    // compaction or the `context_management` feature silently loses context.
    let mut openai = serde_json::Map::new();
    openai.insert(
        "type".into(),
        serde_json::Value::String("compaction".into()),
    );
    openai.insert("itemId".into(), serde_json::Value::String("cmp_1".into()));
    openai.insert(
        "encryptedContent".into(),
        serde_json::Value::String("CMP_ENC".into()),
    );
    let mut meta = vercel_ai_provider::ProviderMetadata::default();
    meta.0
        .insert("openai".into(), serde_json::Value::Object(openai));

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Custom(
            vercel_ai_provider::CustomPart::new("openai-compaction").with_provider_metadata(meta),
        )],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert_eq!(items[0]["type"], "compaction");
    assert_eq!(items[0]["encrypted_content"], "CMP_ENC");
}

#[test]
fn converts_tool_result() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_1".into(),
            tool_name: "get_weather".into(),
            output: ToolResultContent::Text {
                value: "72F".into(),
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert_eq!(items[0]["type"], "function_call_output");
    assert_eq!(items[0]["call_id"], "call_1");
    assert_eq!(items[0]["output"], "72F");
}

#[test]
fn responses_call_id_preserves_ids_up_to_the_limit() {
    let id = "x".repeat(64);
    assert_eq!(projected_call_id(&id), id);
}

#[test]
fn long_responses_call_id_is_stable_and_shared_by_call_and_output() {
    let long_id = format!("codex_mcp__exec-{}", "x".repeat(100));
    let prompt = vec![
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id: long_id.clone(),
                tool_name: "exec".into(),
                input: serde_json::json!({"command": "true"}),
                provider_executed: None,
                provider_metadata: None,
                invalid: false,
                invalid_reason: None,
            })],
            provider_options: None,
        },
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart {
                tool_call_id: long_id,
                tool_name: "exec".into(),
                output: ToolResultContent::Text {
                    value: "ok".into(),
                    provider_options: None,
                },
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
    ];

    let (items, warnings) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert!(warnings.is_empty());
    let call_id = items[0]["call_id"].as_str().expect("assistant call id");
    assert!(call_id.starts_with("call_"));
    assert!(call_id.chars().count() <= MAX_RESPONSES_CALL_ID_CHARS);
    assert_eq!(items[1]["call_id"], call_id);
}

#[test]
fn long_responses_call_id_hashes_the_full_utf8_value() {
    let first = "工".repeat(65);
    let second = format!("{}具", "工".repeat(64));
    assert_ne!(projected_call_id(&first), projected_call_id(&second));
}

#[test]
fn long_call_id_never_shadows_a_caller_provided_short_id() {
    let long_id = format!("tool-call-{}", "x".repeat(100));
    let digest = format!("{:x}", sha2::Sha256::digest(long_id.as_bytes()));
    let colliding_short_id = format!("call_{}", &digest[..32]);
    let make_call = |id: String| {
        AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: id,
            tool_name: "exec".into(),
            input: serde_json::json!({}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })
    };
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![make_call(colliding_short_id.clone()), make_call(long_id)],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    assert_eq!(items[0]["call_id"], colliding_short_id);
    assert_ne!(items[0]["call_id"], items[1]["call_id"]);
    assert!(
        items[1]["call_id"]
            .as_str()
            .expect("projected call id")
            .chars()
            .count()
            <= MAX_RESPONSES_CALL_ID_CHARS
    );
}

#[test]
fn converts_tool_search_result_to_native_output() {
    use std::collections::HashMap;
    use vercel_ai_provider::LanguageModelV4ProviderTool;

    let flags = ProviderToolFlags::from_tools(&Some(vec![LanguageModelV4Tool::Provider(
        LanguageModelV4ProviderTool {
            id: "openai.tool_search".into(),
            name: "tool_search".into(),
            args: HashMap::new(),
        },
    )]));
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "ts_1".into(),
            tool_name: "tool_search".into(),
            output: ToolResultContent::Text {
                value: serde_json::json!({
                    "tools": [{
                        "type": "function",
                        "name": "Read",
                        "parameters": { "type": "object" },
                    }]
                })
                .to_string(),
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (items, _) =
        convert_to_openai_responses_input_with_flags(&prompt, SystemMessageMode::System, &flags);
    assert_eq!(items[0]["type"], "tool_search_output");
    assert_eq!(items[0]["execution"], "client");
    assert_eq!(items[0]["status"], "completed");
    assert_eq!(items[0]["call_id"], "ts_1");
    assert_eq!(items[0]["tools"][0]["name"], "Read");
}

#[test]
fn tool_result_content_image_data_passes_through_as_input_image() {
    // Responses API natively supports images in tool results via
    // `input_image` with a `data:` URL. Pre-refactor the FileData branch
    // didn't exist on this conversion path.
    use vercel_ai_provider::ToolResultContentPart;
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_img".into(),
            tool_name: "FileRead".into(),
            output: ToolResultContent::Content {
                value: vec![ToolResultContentPart::FileData {
                    data: "iVBOR...".into(),
                    media_type: "image/png".into(),
                    filename: None,
                    provider_options: None,
                }],
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    let output = &items[0]["output"];
    assert_eq!(output[0]["type"], "input_image");
    let url = output[0]["image_url"].as_str().unwrap();
    assert!(
        url.starts_with("data:image/png;base64,iVBOR"),
        "expected data URL, got: {url}"
    );
}

#[test]
fn tool_result_content_image_url_passes_through_as_input_image() {
    use vercel_ai_provider::ToolResultContentPart;
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_url".into(),
            tool_name: "FileRead".into(),
            output: ToolResultContent::Content {
                value: vec![ToolResultContentPart::FileUrl {
                    url: "https://example.com/cat.png".into(),
                    media_type: "image/png".into(),
                    provider_options: None,
                }],
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    let output = &items[0]["output"];
    assert_eq!(output[0]["type"], "input_image");
    assert_eq!(output[0]["image_url"], "https://example.com/cat.png");
}

#[test]
fn tool_result_content_pdf_data_degrades_to_input_text_marker() {
    // Responses API only accepts images in tool_result blocks — PDFs and
    // other documents are degraded to an explicit text marker rather
    // than silently dropped.
    use vercel_ai_provider::ToolResultContentPart;
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_pdf".into(),
            tool_name: "FileRead".into(),
            output: ToolResultContent::Content {
                value: vec![ToolResultContentPart::FileData {
                    data: "JVBER...".into(),
                    media_type: "application/pdf".into(),
                    filename: None,
                    provider_options: None,
                }],
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    let output = &items[0]["output"];
    assert_eq!(output[0]["type"], "input_text");
    let text = output[0]["text"].as_str().unwrap();
    assert!(text.contains("application/pdf"), "got: {text}");
    assert!(
        text.contains("only accepts images"),
        "expected document degradation marker, got: {text}"
    );
}

#[test]
fn tool_result_content_mixed_text_and_image_emits_both_parts() {
    use vercel_ai_provider::ToolResultContentPart;
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(ToolResultPart {
            tool_call_id: "call_mix".into(),
            tool_name: "FileRead".into(),
            output: ToolResultContent::Content {
                value: vec![
                    ToolResultContentPart::Text {
                        text: "explanation".into(),
                        provider_options: None,
                    },
                    ToolResultContentPart::FileData {
                        data: "iVBOR...".into(),
                        media_type: "image/png".into(),
                        filename: None,
                        provider_options: None,
                    },
                ],
                provider_options: None,
            },
            is_error: false,
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (items, _) = convert_to_openai_responses_input(&prompt, SystemMessageMode::System);
    let output = &items[0]["output"];
    assert_eq!(output[0]["type"], "input_text");
    assert_eq!(output[0]["text"], "explanation");
    assert_eq!(output[1]["type"], "input_image");
}

/// The projector runs on every Responses request, including long prompts,
/// while an over-limit `call_id` is vanishingly rare. It must decide "nothing
/// to do" without building any map.
#[test]
fn a_prompt_with_only_short_call_ids_allocates_no_projection() {
    let make_call = |id: &str| {
        AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: id.into(),
            tool_name: "exec".into(),
            input: serde_json::json!({}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })
    };
    let prompt = vec![
        LanguageModelV4Message::Assistant {
            content: vec![make_call("call_1"), make_call(&"x".repeat(64))],
            provider_options: None,
        },
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(ToolResultPart {
                tool_call_id: "call_1".into(),
                tool_name: "exec".into(),
                output: vercel_ai_provider::ToolResultContent::text("ok"),
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
    ];

    let projector = ResponsesCallIdProjector::for_prompt(&prompt);
    assert!(projector.by_original.is_empty());
    assert_eq!(projector.project("call_1"), "call_1");
}

/// Multi-byte IDs must not be misjudged by the cheap byte-length pre-filter:
/// 64 CJK characters are 192 bytes but still within the character limit.
#[test]
fn the_byte_prefilter_does_not_misclassify_multibyte_ids() {
    let id = "工".repeat(64);
    assert!(!ResponsesCallIdProjector::is_over_limit(&id));
    assert_eq!(projected_call_id(&id), id);

    let over = "工".repeat(65);
    assert!(ResponsesCallIdProjector::is_over_limit(&over));
    assert_ne!(projected_call_id(&over), over);
}
