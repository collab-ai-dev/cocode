use super::*;
use pretty_assertions::assert_eq;

#[test]
fn deserializes_server_tool_call_item() {
    let item: ResponseOutputItem = serde_json::from_value(serde_json::json!({
        "type": "web_search_call",
        "id": "ws_1",
        "name": "browse_page",
        "status": "completed",
    }))
    .unwrap();
    let (kind, tool) = item.as_server_tool().expect("server tool");
    assert_eq!(kind, ServerToolKind::WebSearch);
    assert_eq!(tool.id, "ws_1");
}

#[test]
fn resolves_server_tool_name_with_override() {
    let names = ResponsesToolNames {
        web_search: Some("myWebSearch".into()),
        ..Default::default()
    };
    let item = ServerToolCallItem {
        id: "ws_1".into(),
        name: Some("web_search".into()),
        arguments: Some("{\"q\":\"x\"}".into()),
        input: None,
        status: None,
    };
    let (name, input) = resolve_server_tool(ServerToolKind::WebSearch, &item, &names);
    assert_eq!(name, "myWebSearch");
    assert_eq!(input, "{\"q\":\"x\"}");
}

#[test]
fn resolves_mcp_falls_back_to_item_name() {
    let names = ResponsesToolNames::default();
    let item = ServerToolCallItem {
        id: "m1".into(),
        name: Some("list_repos".into()),
        arguments: Some("{}".into()),
        input: None,
        status: None,
    };
    let (name, _) = resolve_server_tool(ServerToolKind::Mcp, &item, &names);
    assert_eq!(name, "list_repos");
}

#[test]
fn custom_tool_call_uses_input_not_arguments() {
    let names = ResponsesToolNames::default();
    let item = ServerToolCallItem {
        id: "c1".into(),
        name: Some("do_thing".into()),
        arguments: Some("ignored".into()),
        input: Some("the-input".into()),
        status: None,
    };
    let (name, input) = resolve_server_tool(ServerToolKind::CustomToolCall, &item, &names);
    assert_eq!(name, "do_thing");
    assert_eq!(input, "the-input");
}

#[test]
fn unknown_output_item_type_is_swallowed() {
    let item: ResponseOutputItem = serde_json::from_value(serde_json::json!({
        "type": "some_future_item",
        "id": "x",
    }))
    .unwrap();
    assert!(matches!(item, ResponseOutputItem::Unknown));
}

#[test]
fn deserializes_stream_events_and_unknown() {
    let ev: ResponsesStreamEvent = serde_json::from_value(serde_json::json!({
        "type": "response.output_text.delta",
        "item_id": "msg_1",
        "delta": "hi",
    }))
    .unwrap();
    assert!(matches!(ev, ResponsesStreamEvent::OutputTextDelta { .. }));

    let unknown: ResponsesStreamEvent = serde_json::from_value(serde_json::json!({
        "type": "response.web_search_call.searching",
        "item_id": "ws_1",
        "output_index": 0,
    }))
    .unwrap();
    assert!(matches!(unknown, ResponsesStreamEvent::Unknown));
}

#[test]
fn function_call_without_call_id_fails_to_decode() {
    let res: Result<ResponseOutputItem, _> = serde_json::from_value(serde_json::json!({
        "type": "function_call",
        "id": "fc_1",
        "name": "get_weather",
        "arguments": "{}",
    }));
    assert!(res.is_err(), "call_id is mandatory");
}
