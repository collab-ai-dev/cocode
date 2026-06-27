use coco_types::SideQueryToolDef;

use super::*;

#[test]
fn forced_tool_maps_to_specific_tool_choice() {
    let request = SideQueryRequest::with_forced_tool(
        "system",
        "user",
        SideQueryToolDef {
            name: "select_memories".to_string(),
            description: "Pick memories".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
        },
        "test",
    );

    assert_eq!(
        forced_tool_choice(&request),
        Some(LanguageModelToolChoice::tool("select_memories")),
    );
}

#[test]
fn missing_forced_tool_leaves_tool_choice_unset() {
    let request = SideQueryRequest::simple("system", "user", "test");
    assert_eq!(forced_tool_choice(&request), None);
}
