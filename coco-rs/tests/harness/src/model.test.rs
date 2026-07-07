use coco_inference::LanguageModel;
use coco_llm_types::AssistantContentPart;

use super::*;

#[tokio::test]
async fn scripted_model_returns_text_then_tool_call() {
    let model = MockModelBuilder::new()
        .then_text("hello")
        .then_tool_call("Read", serde_json::json!({ "file_path": "/tmp/x" }))
        .build();
    let options = coco_inference::LanguageModelCallOptions::default();

    let first = model
        .do_generate(&options, None)
        .await
        .expect("text result");
    assert!(matches!(
        &first.content[0],
        AssistantContentPart::Text(text) if text.text == "hello"
    ));

    let second = model
        .do_generate(&options, None)
        .await
        .expect("tool result");
    assert!(matches!(
        &second.content[0],
        AssistantContentPart::ToolCall(tool) if tool.tool_name == "Read"
    ));
}
