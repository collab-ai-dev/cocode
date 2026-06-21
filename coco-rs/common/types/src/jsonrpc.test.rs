use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn request_id_integer_roundtrip() {
    let id = RequestId::Integer(42);
    let j = serde_json::to_value(&id).unwrap();
    assert_eq!(j, json!(42));
    let back: RequestId = serde_json::from_value(j).unwrap();
    assert_eq!(back, RequestId::Integer(42));
}

#[test]
fn request_id_string_roundtrip() {
    let id = RequestId::String("req-abc".into());
    let j = serde_json::to_value(&id).unwrap();
    assert_eq!(j, json!("req-abc"));
    let back: RequestId = serde_json::from_value(j).unwrap();
    assert_eq!(back, RequestId::String("req-abc".into()));
}

#[test]
fn request_id_display() {
    assert_eq!(RequestId::Integer(7).as_display(), "7");
    assert_eq!(RequestId::String("abc".into()).as_display(), "abc");
}

#[test]
fn jsonrpc_request_serializes_with_type_tag() {
    let msg = JsonRpcMessage::Request(JsonRpcRequest {
        request_id: RequestId::Integer(1),
        method: "turn/start".into(),
        params: json!({ "prompt": "hello" }),
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["type"], "request");
    assert_eq!(j["request_id"], 1);
    assert_eq!(j["method"], "turn/start");
    assert_eq!(j["params"]["prompt"], "hello");
}

#[test]
fn jsonrpc_response_serializes_with_type_tag() {
    let msg = JsonRpcMessage::Response(JsonRpcResponse {
        request_id: RequestId::Integer(1),
        result: json!({ "ok": true }),
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["type"], "response");
    assert_eq!(j["request_id"], 1);
    assert_eq!(j["result"]["ok"], true);
}

#[test]
fn jsonrpc_error_serializes_with_type_tag() {
    let msg = JsonRpcMessage::Error(JsonRpcError {
        request_id: RequestId::Integer(2),
        code: error_codes::METHOD_NOT_FOUND,
        message: "unknown method".into(),
        data: None,
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["type"], "error");
    assert_eq!(j["code"], -32601);
    assert_eq!(j["message"], "unknown method");
    assert!(j.get("data").is_none() || j["data"].is_null());
}

#[test]
fn jsonrpc_notification_serializes_with_type_tag() {
    let msg = JsonRpcMessage::Notification(JsonRpcNotification {
        method: "turn/started".into(),
        params: json!({ "turn_id": "t1", "turn_number": 1 }),
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["type"], "notification");
    assert_eq!(j["method"], "turn/started");
    assert_eq!(j["params"]["turn_number"], 1);
    // Notifications have no request_id
    assert!(j.get("request_id").is_none());
}

#[test]
fn jsonrpc_message_roundtrip() {
    let msg = JsonRpcMessage::Request(JsonRpcRequest {
        request_id: RequestId::String("req-1".into()),
        method: "mcp/status".into(),
        params: json!({}),
    });
    let s = serde_json::to_string(&msg).unwrap();
    let back: JsonRpcMessage = serde_json::from_str(&s).unwrap();
    match back {
        JsonRpcMessage::Request(r) => {
            assert_eq!(r.request_id, RequestId::String("req-1".into()));
            assert_eq!(r.method, "mcp/status");
        }
        _ => panic!("expected Request"),
    }
}

#[test]
fn jsonrpc_error_codes_are_in_reserved_range() {
    // JSON-RPC 2.0 reserves -32768 to -32000 for protocol errors;
    // -32000 to -32099 is the reserved server error range.
    // Const blocks make these compile-time checks instead of runtime asserts.
    const _: () = {
        assert!(error_codes::PARSE_ERROR < -32000);
        assert!(error_codes::INVALID_REQUEST < -32000);
        assert!(error_codes::METHOD_NOT_FOUND < -32000);
        assert!(error_codes::INVALID_PARAMS < -32000);
        assert!(error_codes::INTERNAL_ERROR < -32000);
        assert!(error_codes::REQUEST_CANCELLED >= -32099);
        assert!(error_codes::PERMISSION_DENIED >= -32099);
        assert!(error_codes::NOT_INITIALIZED >= -32099);
    };
}
