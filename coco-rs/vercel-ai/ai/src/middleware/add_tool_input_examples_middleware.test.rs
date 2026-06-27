use super::*;
use serde_json::json;
use std::sync::Arc;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;
use vercel_ai_provider::language_model::v4::function_tool::ToolInputExample;

struct MockModel;

#[async_trait::async_trait]
impl vercel_ai_provider::LanguageModelV4 for MockModel {
    fn provider(&self) -> &str {
        "mock"
    }
    fn model_id(&self) -> &str {
        "mock"
    }
    async fn do_generate(
        &self,
        _: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<vercel_ai_provider::LanguageModelV4GenerateResult, AISdkError> {
        unimplemented!()
    }
    async fn do_stream(
        &self,
        _: &LanguageModelV4CallOptions,
        _abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<vercel_ai_provider::LanguageModelV4StreamResult, AISdkError> {
        unimplemented!()
    }
}

#[tokio::test]
async fn test_adds_examples_to_description() {
    let middleware = AddToolInputExamplesMiddleware::new();

    let func_tool = LanguageModelV4FunctionTool::with_description(
        "test_tool",
        "A test tool",
        json!({"type": "object"}),
    )
    .with_examples(vec![
        ToolInputExample::new(
            json!({"arg": "value1"})
                .as_object()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
        ),
        ToolInputExample::new(
            json!({"arg": "value2"})
                .as_object()
                .unwrap()
                .clone()
                .into_iter()
                .collect(),
        ),
    ]);

    let params = LanguageModelV4CallOptions {
        tools: Some(vec![LanguageModelV4Tool::Function(func_tool)]),
        ..Default::default()
    };

    let result = middleware
        .transform_params(TransformParamsOptions {
            call_type: vercel_ai_provider::language_model_middleware::CallType::Generate,
            params,
            model: Arc::new(MockModel),
        })
        .await
        .unwrap();

    if let Some(tools) = result.tools {
        let func = tools[0].as_function().unwrap();
        let desc = func.description.as_ref().unwrap();
        assert!(desc.contains("A test tool"));
        assert!(desc.contains("Input Examples:"));
        assert!(func.input_examples.is_none()); // removed by default
    }
}
