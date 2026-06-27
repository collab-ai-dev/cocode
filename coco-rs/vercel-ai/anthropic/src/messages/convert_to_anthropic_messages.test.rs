use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::LanguageModelV4Message;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::TextPart;
use vercel_ai_provider::ToolCallPart;

use super::*;

// ---------------------------------------------------------------------------
// Basic conversions (existing tests updated for new API)
// ---------------------------------------------------------------------------

#[test]
fn converts_system_message() {
    let prompt = vec![LanguageModelV4Message::system(
        "You are a helpful assistant.",
    )];
    let (system, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.is_empty());
    assert!(messages.is_empty());
    let system = system.unwrap_or_else(|| panic!("expected system"));
    assert_eq!(system.len(), 1);
    assert_eq!(system[0]["type"], "text");
    assert_eq!(system[0]["text"], "You are a helpful assistant.");
}

#[test]
fn converts_developer_message_as_system_block() {
    let prompt = vec![LanguageModelV4Message::developer_text("Follow app policy.")];
    let (system, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.is_empty());
    assert!(messages.is_empty());
    let system = system.unwrap_or_else(|| panic!("expected system"));
    assert_eq!(system[0]["type"], "text");
    assert_eq!(system[0]["text"], "Follow app policy.");
}

#[test]
fn converts_user_text_message() {
    let prompt = vec![LanguageModelV4Message::user_text("Hello")];
    let (system, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(system.is_none());
    assert!(warnings.is_empty());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let content = messages[0]["content"]
        .as_array()
        .unwrap_or_else(|| panic!("expected array"));
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Hello");
}

#[test]
fn converts_assistant_text_message() {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Text(TextPart {
            text: "Hi there!".into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "assistant");
    let content = messages[0]["content"]
        .as_array()
        .unwrap_or_else(|| panic!("expected array"));
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Hi there!");
}

#[test]
fn converts_assistant_tool_call() {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "tc_1".into(),
            tool_name: "get_weather".into(),
            input: serde_json::json!({"city": "SF"}),
            provider_executed: None,
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    let content = messages[0]["content"]
        .as_array()
        .unwrap_or_else(|| panic!("expected array"));
    assert_eq!(content[0]["type"], "tool_use");
    assert_eq!(content[0]["id"], "tc_1");
    assert_eq!(content[0]["name"], "get_weather");
    assert_eq!(content[0]["input"]["city"], "SF");
}

#[test]
fn converts_tool_result() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Text {
                    value: "Sunny, 72°F".into(),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    assert_eq!(messages[0]["role"], "user");
    let content = messages[0]["content"]
        .as_array()
        .unwrap_or_else(|| panic!("expected array"));
    assert_eq!(content[0]["type"], "tool_result");
    assert_eq!(content[0]["tool_use_id"], "tc_1");
    assert_eq!(content[0]["content"], "Sunny, 72°F");
}

#[test]
fn system_is_none_when_no_system_messages() {
    let prompt = vec![LanguageModelV4Message::user_text("Hi")];
    let (system, _, _) = convert_to_anthropic_messages(&prompt, true);
    assert!(system.is_none());
}

#[test]
fn multiple_system_messages_concatenated() {
    let prompt = vec![
        LanguageModelV4Message::system("First instruction."),
        LanguageModelV4Message::system("Second instruction."),
    ];
    let (system, _, _) = convert_to_anthropic_messages(&prompt, true);
    let system = system.unwrap_or_else(|| panic!("expected system"));
    assert_eq!(system.len(), 2);
}

// ---------------------------------------------------------------------------
// Message grouping tests
// ---------------------------------------------------------------------------

#[test]
fn groups_consecutive_user_messages_into_single_message() {
    let prompt = vec![
        LanguageModelV4Message::user_text("Hello"),
        LanguageModelV4Message::user_text("How are you?"),
    ];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    // Should produce 1 message with 2 content parts
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["text"], "Hello");
    assert_eq!(content[1]["text"], "How are you?");
}

#[test]
fn groups_user_and_tool_messages_into_single_user_block() {
    let prompt = vec![
        LanguageModelV4Message::user_text("What is the weather?"),
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(
                vercel_ai_provider::content::ToolResultPart {
                    tool_call_id: "tc_1".into(),
                    tool_name: String::new(),
                    is_error: false,
                    output: ToolResultContent::Text {
                        value: "Sunny".into(),
                        provider_options: None,
                    },
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
    ];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    // User + Tool should be grouped into 1 user message
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[1]["type"], "tool_result");
}

#[test]
fn alternating_user_assistant_produces_separate_messages() {
    let prompt = vec![
        LanguageModelV4Message::user_text("Hi"),
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "Hello!".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        LanguageModelV4Message::user_text("Bye"),
    ];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[2]["role"], "user");
}

#[test]
fn consecutive_tool_messages_grouped_into_single_user_block() {
    let prompt = vec![
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(
                vercel_ai_provider::content::ToolResultPart {
                    tool_call_id: "tc_1".into(),
                    tool_name: String::new(),
                    is_error: false,
                    output: ToolResultContent::Text {
                        value: "Result 1".into(),
                        provider_options: None,
                    },
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
        LanguageModelV4Message::Tool {
            content: vec![ToolContentPart::ToolResult(
                vercel_ai_provider::content::ToolResultPart {
                    tool_call_id: "tc_2".into(),
                    tool_name: String::new(),
                    is_error: false,
                    output: ToolResultContent::Text {
                        value: "Result 2".into(),
                        provider_options: None,
                    },
                    provider_metadata: None,
                },
            )],
            provider_options: None,
        },
    ];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
}

// ---------------------------------------------------------------------------
// Provider-executed tool call round-trip
// ---------------------------------------------------------------------------

#[test]
fn converts_provider_executed_server_tool_use() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "web_search".to_string(),
        "anthropic.web_search_20250305".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "tc_ws".into(),
            tool_name: "anthropic.web_search_20250305".into(),
            input: json!({"query": "rust programming"}),
            provider_executed: Some(true),
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "server_tool_use");
    assert_eq!(content[0]["name"], "web_search");
    assert_eq!(content[0]["id"], "tc_ws");
}

#[test]
fn converts_mcp_tool_use_round_trip() {
    let mapping = ToolNameMapping::empty();

    let mut mcp_meta = HashMap::new();
    mcp_meta.insert(
        "anthropic".into(),
        json!({"type": "mcp-tool-use", "serverName": "my-server"}),
    );

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![
            AssistantContentPart::ToolCall(ToolCallPart {
                tool_call_id: "mcp_1".into(),
                tool_name: "my_tool".into(),
                input: json!({"arg": "value"}),
                provider_executed: Some(true),
                provider_metadata: Some(ProviderMetadata(mcp_meta)),
                invalid: false,
                invalid_reason: None,
            }),
            AssistantContentPart::ToolResult(vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "mcp_1".into(),
                tool_name: "my_tool".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!({"result": "ok"}),
                    provider_options: None,
                },
                provider_metadata: None,
            }),
        ],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "mcp_tool_use");
    assert_eq!(content[0]["server_name"], "my-server");
    assert_eq!(content[1]["type"], "mcp_tool_result");
    assert_eq!(content[1]["tool_use_id"], "mcp_1");
}

#[test]
fn converts_code_execution_sub_tool() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "code_execution".to_string(),
        "anthropic.code_execution_20250825".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "tc_ce".into(),
            tool_name: "anthropic.code_execution_20250825".into(),
            input: json!({"type": "bash_code_execution", "command": "ls"}),
            provider_executed: Some(true),
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "server_tool_use");
    assert_eq!(content[0]["name"], "bash_code_execution");
}

#[test]
fn strips_programmatic_tool_call_type() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "code_execution".to_string(),
        "anthropic.code_execution_20250825".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "tc_prog".into(),
            tool_name: "anthropic.code_execution_20250825".into(),
            input: json!({"type": "programmatic-tool-call", "code": "print('hello')"}),
            provider_executed: Some(true),
            provider_metadata: None,
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "server_tool_use");
    assert_eq!(content[0]["name"], "code_execution");
    // type field should be stripped
    assert!(content[0]["input"].get("type").is_none());
    assert_eq!(content[0]["input"]["code"], "print('hello')");
}

// ---------------------------------------------------------------------------
// Code execution result variants
// ---------------------------------------------------------------------------

#[test]
fn converts_code_execution_tool_result_in_assistant_block() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "code_execution".to_string(),
        "anthropic.code_execution_20250825".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_ce".into(),
                tool_name: "anthropic.code_execution_20250825".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!({
                        "type": "code_execution_result",
                        "stdout": "hello\n",
                        "stderr": "",
                        "return_code": 0,
                    }),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "code_execution_tool_result");
    assert_eq!(content[0]["content"]["type"], "code_execution_result");
}

// ---------------------------------------------------------------------------
// Trailing whitespace trimming
// ---------------------------------------------------------------------------

#[test]
fn trims_trailing_whitespace_on_last_assistant_text() {
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Text(TextPart {
            text: "Hello world  \n  ".into(),
            provider_metadata: None,
        })],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["text"], "Hello world");
}

#[test]
fn does_not_trim_non_trailing_assistant_text() {
    let prompt = vec![
        LanguageModelV4Message::Assistant {
            content: vec![AssistantContentPart::Text(TextPart {
                text: "Hello  ".into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        LanguageModelV4Message::user_text("Hi"),
    ];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    // First assistant message is not the last block, so no trimming
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["text"], "Hello  ");
}

// ---------------------------------------------------------------------------
// Compaction block
// ---------------------------------------------------------------------------

#[test]
fn converts_compaction_text_block() {
    let mut meta = HashMap::new();
    meta.insert("anthropic".into(), json!({"type": "compaction"}));

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Text(TextPart {
            text: "compacted summary".into(),
            provider_metadata: Some(ProviderMetadata(meta)),
        })],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "compaction");
    assert_eq!(content[0]["content"], "compacted summary");
}

// ---------------------------------------------------------------------------
// Caller info in tool calls
// ---------------------------------------------------------------------------

#[test]
fn forwards_caller_info_in_regular_tool_call() {
    let mut meta = HashMap::new();
    meta.insert(
        "anthropic".into(),
        json!({"caller": {"type": "code_execution_20250825", "toolId": "ce_1"}}),
    );

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolCall(ToolCallPart {
            tool_call_id: "tc_1".into(),
            tool_name: "my_tool".into(),
            input: json!({}),
            provider_executed: None,
            provider_metadata: Some(ProviderMetadata(meta)),
            invalid: false,
            invalid_reason: None,
        })],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["caller"]["type"], "code_execution_20250825");
    assert_eq!(content[0]["caller"]["tool_id"], "ce_1");
}

// ---------------------------------------------------------------------------
// Execution denied message
// ---------------------------------------------------------------------------

#[test]
fn execution_denied_uses_correct_default_message() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::ExecutionDenied {
                    reason: None,
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["content"], "Tool execution denied.");
    assert_eq!(content[0]["is_error"], true);
}

// ---------------------------------------------------------------------------
// Image/* default mapping
// ---------------------------------------------------------------------------

#[test]
fn maps_image_wildcard_to_image_jpeg() {
    let prompt = vec![LanguageModelV4Message::User {
        content: vec![UserContentPart::File(
            vercel_ai_provider::content::FilePart {
                data: vercel_ai_provider::SharedV4FileData::data_base64("abc"),
                media_type: "image/*".into(),
                filename: None,
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, _) = convert_to_anthropic_messages(&prompt, true);
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "image");
    assert_eq!(content[0]["source"]["media_type"], "image/jpeg");
}

// ---------------------------------------------------------------------------
// Phase 1.1: web_fetch_tool_result camelCase→snake_case
// ---------------------------------------------------------------------------

#[test]
fn web_fetch_tool_result_transforms_camel_case_to_snake_case() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "web_fetch".to_string(),
        "anthropic.web_fetch_20250910".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_wf".into(),
                tool_name: "anthropic.web_fetch_20250910".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!({
                        "url": "https://example.com",
                        "retrievedAt": "2024-01-01T00:00:00Z",
                        "content": {
                            "title": "Example",
                            "citations": true,
                            "source": {
                                "type": "base64",
                                "mediaType": "text/html",
                                "data": "aGVsbG8="
                            }
                        }
                    }),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "web_fetch_tool_result");
    let inner = &content[0]["content"];
    assert_eq!(inner["type"], "web_fetch_result");
    assert_eq!(inner["url"], "https://example.com");
    assert_eq!(inner["retrieved_at"], "2024-01-01T00:00:00Z");
    assert_eq!(inner["content"]["type"], "document");
    assert_eq!(inner["content"]["title"], "Example");
    assert_eq!(inner["content"]["citations"], true);
    assert_eq!(inner["content"]["source"]["type"], "base64");
    assert_eq!(inner["content"]["source"]["media_type"], "text/html");
    assert_eq!(inner["content"]["source"]["data"], "aGVsbG8=");
}

#[test]
fn web_fetch_tool_result_fallback_on_missing_fields() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "web_fetch".to_string(),
        "anthropic.web_fetch_20250910".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_wf".into(),
                tool_name: "anthropic.web_fetch_20250910".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!({"partial": true}),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    // Should produce a warning and forward raw JSON
    assert!(result.warnings.iter().any(
        |w| matches!(w, Warning::Other { message } if message.contains("missing required fields"))
    ));
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "web_fetch_tool_result");
    assert_eq!(content[0]["content"]["partial"], true);
}

// ---------------------------------------------------------------------------
// Phase 1.2: web_search_tool_result camelCase→snake_case
// ---------------------------------------------------------------------------

#[test]
fn web_search_tool_result_transforms_camel_case_fields() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "web_search".to_string(),
        "anthropic.web_search_20250305".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_ws".into(),
                tool_name: "anthropic.web_search_20250305".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!([
                        {
                            "url": "https://example.com",
                            "title": "Example",
                            "type": "web_search_result",
                            "pageAge": "2024-01-01",
                            "encryptedContent": "encrypted_data_here"
                        }
                    ]),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "web_search_tool_result");
    let items = content[0]["content"].as_array().unwrap();
    assert_eq!(items[0]["url"], "https://example.com");
    assert_eq!(items[0]["title"], "Example");
    assert_eq!(items[0]["type"], "web_search_result");
    assert_eq!(items[0]["page_age"], "2024-01-01");
    assert_eq!(items[0]["encrypted_content"], "encrypted_data_here");
    // camelCase keys should not be present
    assert!(items[0].get("pageAge").is_none());
    assert!(items[0].get("encryptedContent").is_none());
}

// ---------------------------------------------------------------------------
// Phase 1.3: tool_search_tool_result structured wrapping
// ---------------------------------------------------------------------------

#[test]
fn tool_search_tool_result_wraps_with_structured_format() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "tool_search_tool_regex".to_string(),
        "anthropic.tool_search_20260209".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_ts".into(),
                tool_name: "anthropic.tool_search_20260209".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!([
                        {"toolName": "my_tool"},
                        {"toolName": "other_tool"}
                    ]),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "tool_search_tool_result");
    let inner = &content[0]["content"];
    assert_eq!(inner["type"], "tool_search_tool_search_result");
    let refs = inner["tool_references"].as_array().unwrap();
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[0]["type"], "tool_reference");
    assert_eq!(refs[0]["tool_name"], "my_tool");
    assert_eq!(refs[1]["tool_name"], "other_tool");
}

// ---------------------------------------------------------------------------
// Phase 1.4: code_execution_result content default
// ---------------------------------------------------------------------------

#[test]
fn code_execution_result_defaults_content_to_empty_array() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "code_execution".to_string(),
        "anthropic.code_execution_20250825".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    // Result without a content field
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_ce".into(),
                tool_name: "anthropic.code_execution_20250825".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!({
                        "type": "code_execution_result",
                        "stdout": "hello\n",
                        "stderr": "",
                        "return_code": 0,
                    }),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    // content field should be defaulted to []
    assert_eq!(content[0]["content"]["content"], json!([]));
}

#[test]
fn code_execution_result_preserves_existing_content() {
    let mut api_to_sdk = HashMap::new();
    api_to_sdk.insert(
        "code_execution".to_string(),
        "anthropic.code_execution_20250825".to_string(),
    );
    let mapping = ToolNameMapping::new(&api_to_sdk);

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_ce".into(),
                tool_name: "anthropic.code_execution_20250825".into(),
                is_error: false,
                output: ToolResultContent::Json {
                    value: json!({
                        "type": "code_execution_result",
                        "content": [{"type": "output", "text": "hello"}],
                    }),
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];

    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &mapping,
        &mut CacheControlValidator::new(),
    );
    let content = result.messages[0]["content"].as_array().unwrap();
    // Existing content should be preserved
    let inner_content = content[0]["content"]["content"].as_array().unwrap();
    assert_eq!(inner_content.len(), 1);
    assert_eq!(inner_content[0]["text"], "hello");
}

// ---------------------------------------------------------------------------
// Phase 2.1: serialize_tool_result content part variants
// ---------------------------------------------------------------------------

#[test]
fn tool_result_content_image_data() {
    // Image data — distinguished by media_type starting with "image/"
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Content {
                    value: vec![vercel_ai_provider::ToolResultContentPart::FileData {
                        data: "iVBOR...".into(),
                        media_type: "image/png".into(),
                        filename: None,
                        provider_options: None,
                    }],
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.is_empty(), "{warnings:?}");
    let content = messages[0]["content"].as_array().unwrap();
    let parts = content[0]["content"].as_array().unwrap();
    assert_eq!(parts[0]["type"], "image");
    assert_eq!(parts[0]["source"]["type"], "base64");
    assert_eq!(parts[0]["source"]["media_type"], "image/png");
    assert_eq!(parts[0]["source"]["data"], "iVBOR...");
}

#[test]
fn tool_result_content_image_url() {
    // Image URL — FileUrl with image/* media_type routes to image block.
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Content {
                    value: vec![vercel_ai_provider::ToolResultContentPart::FileUrl {
                        url: "https://example.com/img.png".into(),
                        media_type: "image/png".into(),
                        provider_options: None,
                    }],
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.is_empty(), "{warnings:?}");
    let parts = messages[0]["content"].as_array().unwrap()[0]["content"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["type"], "image");
    assert_eq!(parts[0]["source"]["type"], "url");
    assert_eq!(parts[0]["source"]["url"], "https://example.com/img.png");
}

#[test]
fn tool_result_content_file_url() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Content {
                    value: vec![vercel_ai_provider::ToolResultContentPart::FileUrl {
                        url: "https://example.com/doc.pdf".into(),
                        media_type: "application/pdf".into(),
                        provider_options: None,
                    }],
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.is_empty(), "{warnings:?}");
    let parts = messages[0]["content"].as_array().unwrap()[0]["content"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["type"], "document");
    assert_eq!(parts[0]["source"]["type"], "url");
    assert_eq!(parts[0]["source"]["url"], "https://example.com/doc.pdf");
}

#[test]
fn tool_result_content_file_data_pdf_adds_beta() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Content {
                    value: vec![vercel_ai_provider::ToolResultContentPart::FileData {
                        data: "JVBERi0=".into(),
                        media_type: "application/pdf".into(),
                        filename: None,
                        provider_options: None,
                    }],
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let result = convert_to_anthropic_messages_full(
        &prompt,
        true,
        &ToolNameMapping::empty(),
        &mut CacheControlValidator::new(),
    );
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    assert!(result.betas.contains("pdfs-2024-09-25"));
    let parts = result.messages[0]["content"].as_array().unwrap()[0]["content"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["type"], "document");
    assert_eq!(parts[0]["source"]["type"], "base64");
    assert_eq!(parts[0]["source"]["media_type"], "application/pdf");
}

#[test]
fn tool_result_content_file_data_unsupported_media_type_warns() {
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Content {
                    value: vec![vercel_ai_provider::ToolResultContentPart::FileData {
                        data: "data".into(),
                        media_type: "application/zip".into(),
                        filename: None,
                        provider_options: None,
                    }],
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.iter().any(|w| matches!(w, Warning::Other { message } if message.contains("unsupported tool content part type: file-data with media type: application/zip"))));
    // The unsupported part should be filtered out
    let parts = messages[0]["content"].as_array().unwrap()[0]["content"]
        .as_array()
        .unwrap();
    assert!(parts.is_empty());
}

#[test]
fn tool_result_content_custom_tool_reference() {
    let mut inner = HashMap::new();
    inner.insert("type".into(), json!("tool-reference"));
    inner.insert("toolName".into(), json!("my_tool"));
    let mut po = HashMap::new();
    po.insert("anthropic".into(), inner);
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Content {
                    value: vec![vercel_ai_provider::ToolResultContentPart::Custom {
                        provider_options: Some(ProviderOptions::from_map(po.clone())),
                    }],
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.is_empty(), "{warnings:?}");
    let parts = messages[0]["content"].as_array().unwrap()[0]["content"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["type"], "tool_reference");
    assert_eq!(parts[0]["tool_name"], "my_tool");
}

#[test]
fn tool_result_content_custom_unsupported_warns() {
    let mut inner = HashMap::new();
    inner.insert("type".into(), json!("unknown-type"));
    let mut po = HashMap::new();
    po.insert("anthropic".into(), inner);
    let prompt = vec![LanguageModelV4Message::Tool {
        content: vec![ToolContentPart::ToolResult(
            vercel_ai_provider::content::ToolResultPart {
                tool_call_id: "tc_1".into(),
                tool_name: String::new(),
                is_error: false,
                output: ToolResultContent::Content {
                    value: vec![vercel_ai_provider::ToolResultContentPart::Custom {
                        provider_options: Some(ProviderOptions::from_map(po.clone())),
                    }],
                    provider_options: None,
                },
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, _, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.iter().any(
        |w| matches!(w, Warning::Other { message } if message.contains("unsupported custom tool content part"))
    ));
}

// ---------------------------------------------------------------------------
// Phase 2.2: Unsupported media type warning for user files
// ---------------------------------------------------------------------------

#[test]
fn unsupported_user_file_media_type_warns() {
    let prompt = vec![LanguageModelV4Message::User {
        content: vec![UserContentPart::File(
            vercel_ai_provider::content::FilePart {
                data: vercel_ai_provider::SharedV4FileData::data_base64("data"),
                media_type: "application/zip".into(),
                filename: None,
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.iter().any(
        |w| matches!(w, Warning::Unsupported { feature, .. } if feature == "media type: application/zip")
    ));
    // No content should be generated — empty user block is omitted entirely
    assert!(messages.is_empty());
}

// ---------------------------------------------------------------------------
// Phase 2.3: Non-contiguous system messages warning
// ---------------------------------------------------------------------------

#[test]
fn non_contiguous_system_messages_produces_warning() {
    let prompt = vec![
        LanguageModelV4Message::system("First system."),
        LanguageModelV4Message::user_text("Hello"),
        LanguageModelV4Message::system("Second system."),
    ];
    let (_, _, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.iter().any(|w| matches!(
        w,
        Warning::Unsupported { feature, .. }
            if feature.contains("Multiple system messages")
    )));
}

#[test]
fn contiguous_system_messages_no_warning() {
    let prompt = vec![
        LanguageModelV4Message::system("First."),
        LanguageModelV4Message::system("Second."),
    ];
    let (_, _, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(
        warnings.is_empty(),
        "should not warn for contiguous system messages: {warnings:?}",
    );
}

// ---------------------------------------------------------------------------
// Phase 3.2: "sending reasoning disabled" warning
// ---------------------------------------------------------------------------

#[test]
fn reasoning_disabled_produces_warning() {
    let mut meta = HashMap::new();
    meta.insert("anthropic".into(), json!({"signature": "sig123"}));

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "thinking...".into(),
                provider_metadata: Some(ProviderMetadata(meta)),
            },
        )],
        provider_options: None,
    }];
    // send_reasoning = false
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, false);
    assert!(warnings.iter().any(|w| matches!(
        w,
        Warning::Other { message }
            if message.contains("sending reasoning content is disabled")
    )));
    // No content should be generated — empty assistant block is omitted entirely
    assert!(messages.is_empty());
}

// ---------------------------------------------------------------------------
// Phase 3.3: "unsupported reasoning metadata" warning
// ---------------------------------------------------------------------------

#[test]
fn reasoning_without_signature_or_redacted_data_warns() {
    // No anthropic metadata at all
    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "thinking...".into(),
                provider_metadata: None,
            },
        )],
        provider_options: None,
    }];
    let (_, _, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.iter().any(|w| matches!(
        w,
        Warning::Other { message }
            if message.contains("unsupported reasoning metadata")
    )));
}

#[test]
fn reasoning_with_empty_anthropic_metadata_warns() {
    let mut meta = HashMap::new();
    meta.insert("anthropic".into(), json!({}));

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "thinking...".into(),
                provider_metadata: Some(ProviderMetadata(meta)),
            },
        )],
        provider_options: None,
    }];
    let (_, _, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(warnings.iter().any(|w| matches!(
        w,
        Warning::Other { message }
            if message.contains("unsupported reasoning metadata")
    )));
}

#[test]
fn reasoning_with_signature_does_not_warn() {
    let mut meta = HashMap::new();
    meta.insert("anthropic".into(), json!({"signature": "sig123"}));

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "thinking...".into(),
                provider_metadata: Some(ProviderMetadata(meta)),
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(
        warnings.is_empty(),
        "should not warn when signature present: {warnings:?}",
    );
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "thinking");
    assert_eq!(content[0]["signature"], "sig123");
}

#[test]
fn reasoning_with_redacted_data_does_not_warn() {
    let mut meta = HashMap::new();
    meta.insert("anthropic".into(), json!({"redactedData": "redacted123"}));

    let prompt = vec![LanguageModelV4Message::Assistant {
        content: vec![AssistantContentPart::Reasoning(
            vercel_ai_provider::ReasoningPart {
                text: "".into(),
                provider_metadata: Some(ProviderMetadata(meta)),
            },
        )],
        provider_options: None,
    }];
    let (_, messages, warnings) = convert_to_anthropic_messages(&prompt, true);
    assert!(
        warnings.is_empty(),
        "should not warn when redactedData present: {warnings:?}",
    );
    let content = messages[0]["content"].as_array().unwrap();
    assert_eq!(content[0]["type"], "redacted_thinking");
    assert_eq!(content[0]["data"], "redacted123");
}
