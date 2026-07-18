//! Request-level MCP exposure and `use_tool` integration gate.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use coco_inference::LanguageModel;
use coco_inference::ModelRuntimeRegistry;
use coco_inference::PrebuiltLanguageModelSlot;
use coco_inference::ProviderClientFingerprint;
use coco_inference::RetryConfig;
use coco_test_harness::model::MockModelBuilder;
use coco_test_harness::model::MockResponse;
use coco_tool_runtime::McpHandle;
use coco_tool_runtime::McpToolAnnotations;
use coco_tool_runtime::ToolRegistry;
use coco_tool_runtime::mcp_handle::McpContentBlock;
use coco_tool_runtime::mcp_handle::McpResourceContent;
use coco_tool_runtime::mcp_handle::McpResourceInfo;
use coco_tool_runtime::mcp_handle::McpToolCallResult;
use coco_tools::McpTool;
use coco_tools::ToolSearchTool;
use coco_tools::UseToolTool;
use coco_types::Capability;
use coco_types::McpToolExposure;
use coco_types::Message;
use coco_types::ModelRole;
use coco_types::PermissionMode;
use coco_types::ProviderApi;
use coco_types::ProviderModelSelection;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_types::WireApi;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy)]
enum StrategyFixture {
    Eager,
    ClientPromotion,
    AnthropicReference,
    OpenAiNative,
}

impl StrategyFixture {
    fn runtime(self, model: Arc<dyn LanguageModel>) -> Arc<ModelRuntimeRegistry> {
        let (api, wire_api, capabilities) = match self {
            Self::Eager => (ProviderApi::OpenaiCompat, None, Vec::new()),
            Self::ClientPromotion => (
                ProviderApi::OpenaiCompat,
                None,
                vec![Capability::ClientSideToolSearchPromotion],
            ),
            Self::AnthropicReference => (
                ProviderApi::Anthropic,
                None,
                vec![Capability::AnthropicToolReference],
            ),
            Self::OpenAiNative => (
                ProviderApi::Openai,
                Some(WireApi::Responses),
                vec![Capability::OpenAiNativeToolSearch],
            ),
        };
        let provider = format!("fixture-{self:?}");
        let model_id = "mcp-exposure-model".to_string();
        let fingerprint = ProviderClientFingerprint {
            provider: provider.clone(),
            api,
            api_model_name: model_id.clone(),
            base_url: "https://example.invalid".into(),
            wire_api,
            client_options_digest: [0; 32],
            timeout_secs: 0,
            api_key_origin_digest: [0; 32],
            runtime_state_digest: [0; 32],
        };
        let model_info = coco_config::ModelInfo {
            model_id: model_id.clone(),
            capabilities: (!capabilities.is_empty()).then_some(capabilities),
            ..Default::default()
        };
        let slot = PrebuiltLanguageModelSlot::new(model, RetryConfig::default())
            .with_fingerprint(fingerprint)
            .with_model_info(model_info)
            .with_model_identity(ProviderModelSelection { provider, model_id });
        Arc::new(ModelRuntimeRegistry::from_prebuilt_language_model(
            ModelRole::Main,
            slot,
        ))
    }
}

fn registry() -> Result<Arc<ToolRegistry>, coco_error::BoxedError> {
    let registry = ToolRegistry::new();
    registry.register(Arc::new(ToolSearchTool));
    registry.register(Arc::new(UseToolTool));
    registry.register(Arc::new(
        McpTool::new(
            "matrix".into(),
            "lookup".into(),
            "Look up a matrix entry".into(),
            serde_json::json!({
                "type": "object",
                "properties": { "key": { "type": "string" } },
                "required": ["key"],
                "additionalProperties": false
            }),
            McpToolAnnotations::default(),
        )
        .map_err(|error| coco_error::boxed(error, coco_error::StatusCode::InvalidArguments))?,
    ));
    Ok(Arc::new(registry))
}

fn request_tool_names(options: &coco_inference::LanguageModelCallOptions) -> Vec<String> {
    let mut names: Vec<_> = options
        .tools
        .iter()
        .flatten()
        .map(|tool| tool.name().to_string())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn exposure_matrix_projects_the_expected_request_surface()
-> Result<(), coco_error::BoxedError> {
    let strategies = [
        StrategyFixture::Eager,
        StrategyFixture::ClientPromotion,
        StrategyFixture::AnthropicReference,
        StrategyFixture::OpenAiNative,
    ];
    let exposures = [
        McpToolExposure::Load,
        McpToolExposure::Defer,
        McpToolExposure::UseTool,
    ];

    for strategy in strategies {
        for exposure in exposures {
            let model = MockModelBuilder::new()
                .on_call(0, move |options| {
                    let names = request_tool_names(options);
                    let has_search = names
                        .iter()
                        .any(|name| name == ToolName::ToolSearch.as_str() || name == "tool_search");
                    let has_use_tool = names.iter().any(|name| name == ToolName::UseTool.as_str());
                    let has_mcp = names.iter().any(|name| name == "mcp__matrix__lookup");

                    match exposure {
                        McpToolExposure::Load => {
                            assert!(!has_use_tool, "{strategy:?}: load exposed use_tool");
                            assert!(has_mcp, "{strategy:?}: load omitted the MCP schema");
                            assert_eq!(
                                has_search,
                                !matches!(strategy, StrategyFixture::Eager),
                                "the generic built-in discovery carrier follows model capability"
                            );
                        }
                        McpToolExposure::UseTool => {
                            assert!(has_search, "{strategy:?}: use_tool needs ToolSearch");
                            assert!(has_use_tool, "{strategy:?}: use_tool carrier missing");
                            assert!(!has_mcp, "{strategy:?}: use_tool leaked an MCP schema");
                        }
                        McpToolExposure::Defer => match strategy {
                            StrategyFixture::Eager => {
                                assert!(has_search && has_use_tool && !has_mcp);
                            }
                            StrategyFixture::AnthropicReference => {
                                assert!(has_search && !has_use_tool && has_mcp);
                                let mcp = options
                                    .tools
                                    .iter()
                                    .flatten()
                                    .find(|tool| tool.name() == "mcp__matrix__lookup")
                                    .and_then(|tool| tool.as_function())
                                    .expect("Anthropic deferred MCP function");
                                let defer_loading = mcp
                                    .provider_options
                                    .as_ref()
                                    .and_then(|options| options.0.get("anthropic"))
                                    .and_then(|values| values.get("deferLoading"));
                                assert_eq!(defer_loading, Some(&serde_json::Value::Bool(true)));
                            }
                            StrategyFixture::ClientPromotion | StrategyFixture::OpenAiNative => {
                                assert!(
                                    has_search && !has_use_tool && !has_mcp,
                                    "{strategy:?} projected unexpected tools: {names:?}"
                                );
                            }
                        },
                    }
                    MockResponse::text("done")
                })
                .build();
            let engine = coco_query::QueryEngine::new(
                coco_query::QueryEngineConfig {
                    model_id: "mcp-exposure-model".into(),
                    mcp_tool_exposure: exposure,
                    max_turns: Some(1),
                    ..Default::default()
                },
                coco_types::SessionId::try_new(format!("matrix-{strategy:?}-{exposure:?}"))
                    .expect("safe session id"),
                strategy.runtime(model),
                registry()?,
                CancellationToken::new(),
                None,
            );
            let result = engine.run("inspect MCP exposure").await?;
            assert_eq!(result.response_text, "done");
        }
    }
    Ok(())
}

#[tokio::test]
async fn server_overrides_and_always_load_share_one_request_surface()
-> Result<(), coco_error::BoxedError> {
    let model = MockModelBuilder::new()
        .on_call(0, |options| {
            let names = request_tool_names(options);
            assert!(names.iter().any(|name| name == "mcp__memory__remember"));
            assert!(names.iter().any(|name| name == "mcp__gitlab__merge"));
            assert!(!names.iter().any(|name| name == "mcp__matrix__lookup"));
            assert!(!names.iter().any(|name| name == "mcp__slack__send"));
            assert!(
                names
                    .iter()
                    .any(|name| name == ToolName::ToolSearch.as_str())
            );
            assert!(names.iter().any(|name| name == ToolName::UseTool.as_str()));
            MockResponse::text("done")
        })
        .build();

    let registry = registry()?;
    for (server, tool, description, always_load) in [
        ("memory", "remember", "Remember a value", false),
        ("slack", "send", "Send a message", true),
        ("gitlab", "merge", "Merge a change", true),
    ] {
        let tool = McpTool::new(
            server.into(),
            tool.into(),
            description.into(),
            serde_json::json!({"type": "object", "properties": {}}),
            McpToolAnnotations {
                always_load,
                ..Default::default()
            },
        )
        .map_err(|error| coco_error::boxed(error, coco_error::StatusCode::InvalidArguments))?;
        registry.register(Arc::new(tool));
    }

    let engine = coco_query::QueryEngine::new(
        coco_query::QueryEngineConfig {
            model_id: "mcp-exposure-model".into(),
            mcp_tool_exposure: McpToolExposure::Defer,
            mcp_server_tool_exposure: Arc::new(HashMap::from([
                ("memory".into(), McpToolExposure::Load),
                ("slack".into(), McpToolExposure::UseTool),
            ])),
            max_turns: Some(1),
            ..Default::default()
        },
        coco_types::SessionId::try_new("mixed-server-exposure").expect("safe session id"),
        StrategyFixture::ClientPromotion.runtime(model),
        registry,
        CancellationToken::new(),
        None,
    );
    let result = engine.run("inspect mixed MCP exposure").await?;
    assert_eq!(result.response_text, "done");
    Ok(())
}

#[derive(Debug, Default)]
struct RecordingMcpHandle {
    calls: Mutex<Vec<(String, String, Option<serde_json::Value>)>>,
}

#[async_trait]
impl McpHandle for RecordingMcpHandle {
    async fn list_resources(
        &self,
        _server_name: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(Vec::new())
    }

    async fn read_resource(
        &self,
        _server_name: &str,
        _resource_uri: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        Ok(Vec::new())
    }

    async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<serde_json::Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        self.calls
            .lock()
            .await
            .push((server_name.to_string(), tool_name.to_string(), arguments));
        Ok(McpToolCallResult {
            content: vec![McpContentBlock::Text("semantic MCP result".into())],
            is_error: false,
        })
    }

    async fn authenticate(&self, _server_name: &str) -> Result<String, coco_error::BoxedError> {
        Ok("ok".into())
    }

    async fn connected_servers(&self) -> Vec<String> {
        vec!["matrix".into()]
    }
}

#[tokio::test]
async fn use_tool_executes_semantic_target_and_preserves_provider_wire_name()
-> Result<(), coco_error::BoxedError> {
    for streaming_tool_execution in [false, true] {
        let model = MockModelBuilder::new()
            .on_call(0, |options| {
                let names = request_tool_names(options);
                assert!(
                    names
                        .iter()
                        .any(|name| name == ToolName::ToolSearch.as_str())
                );
                assert!(names.iter().any(|name| name == ToolName::UseTool.as_str()));
                assert!(!names.iter().any(|name| name == "mcp__matrix__lookup"));
                MockResponse::tool_call(
                    ToolName::ToolSearch.as_str(),
                    serde_json::json!({ "query": "select:mcp__matrix__lookup" }),
                )
            })
            .on_call(1, |_| {
                MockResponse::tool_call(
                    ToolName::UseTool.as_str(),
                    serde_json::json!({
                        "name": "mcp__matrix__lookup",
                        "arguments": { "key": "answer" }
                    }),
                )
            })
            .then_text("done")
            .build();
        let handle = Arc::new(RecordingMcpHandle::default());
        let engine = coco_query::QueryEngine::new(
            coco_query::QueryEngineConfig {
                model_id: "mcp-exposure-model".into(),
                permission_mode: PermissionMode::BypassPermissions,
                mcp_tool_exposure: McpToolExposure::UseTool,
                streaming_tool_execution,
                max_turns: Some(3),
                ..Default::default()
            },
            coco_types::SessionId::try_new(format!("use-tool-{streaming_tool_execution}"))
                .expect("safe session id"),
            StrategyFixture::Eager.runtime(model),
            registry()?,
            CancellationToken::new(),
            None,
        )
        .with_mcp_handle(handle.clone());

        let result = engine.run("call the matrix lookup").await.expect("query");
        assert_eq!(result.response_text, "done");
        assert_eq!(
            *handle.calls.lock().await,
            vec![(
                "matrix".into(),
                "lookup".into(),
                Some(serde_json::json!({ "key": "answer" }))
            )]
        );

        let tool_result = result
            .final_messages
            .iter()
            .find_map(|message| match message.as_ref() {
                Message::ToolResult(result) if result.tool_use_id == "call_1" => Some(result),
                _ => None,
            })
            .expect("use_tool result");
        assert_eq!(
            tool_result.tool_id,
            ToolId::Mcp {
                server: "matrix".into(),
                tool: "lookup".into()
            },
            "transcript metadata uses semantic target identity"
        );
        let provider_name = match &tool_result.message {
            coco_llm_types::LlmMessage::Tool { content, .. } => content.iter().find_map(|part| {
                let coco_llm_types::ToolContentPart::ToolResult(result) = part else {
                    return None;
                };
                Some(result.tool_name.as_str())
            }),
            _ => None,
        };
        assert_eq!(
            provider_name,
            Some(ToolName::UseTool.as_str()),
            "provider pairing retains the use_tool carrier name"
        );
    }
    Ok(())
}
