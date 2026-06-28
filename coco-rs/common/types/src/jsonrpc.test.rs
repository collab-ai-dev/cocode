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
fn jsonrpc_request_serializes_as_jsonrpc2() {
    let msg = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        request_id: RequestId::Integer(1),
        method: "turn/start".into(),
        params: json!({ "prompt": "hello" }),
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["jsonrpc"], "2.0");
    assert_eq!(j["id"], 1);
    assert_eq!(j["method"], "turn/start");
    assert_eq!(j["params"]["prompt"], "hello");
}

#[test]
fn jsonrpc_response_serializes_as_jsonrpc2() {
    let msg = JsonRpcMessage::Response(JsonRpcResponse {
        jsonrpc: JSONRPC_VERSION.into(),
        request_id: RequestId::Integer(1),
        result: json!({ "ok": true }),
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["jsonrpc"], "2.0");
    assert_eq!(j["id"], 1);
    assert_eq!(j["result"]["ok"], true);
}

#[test]
fn jsonrpc_error_serializes_as_jsonrpc2() {
    let msg = JsonRpcMessage::Error(JsonRpcError {
        jsonrpc: JSONRPC_VERSION.into(),
        request_id: RequestId::Integer(2),
        error: JsonRpcErrorObject {
            code: error_codes::METHOD_NOT_FOUND,
            message: "unknown method".into(),
            data: None,
        },
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["jsonrpc"], "2.0");
    assert_eq!(j["id"], 2);
    assert_eq!(j["error"]["code"], -32601);
    assert_eq!(j["error"]["message"], "unknown method");
    assert!(j["error"].get("data").is_none() || j["error"]["data"].is_null());
}

#[test]
fn jsonrpc_notification_serializes_as_jsonrpc2() {
    let msg = JsonRpcMessage::Notification(JsonRpcNotification {
        jsonrpc: JSONRPC_VERSION.into(),
        method: "turn/started".into(),
        params: json!({ "turn_id": "t1", "turn_number": 1 }),
    });
    let j = serde_json::to_value(&msg).unwrap();
    assert_eq!(j["jsonrpc"], "2.0");
    assert_eq!(j["method"], "turn/started");
    assert_eq!(j["params"]["turn_number"], 1);
    assert!(j.get("id").is_none());
}

#[test]
fn jsonrpc_message_roundtrip() {
    let msg = JsonRpcMessage::Request(JsonRpcRequest {
        jsonrpc: JSONRPC_VERSION.into(),
        request_id: RequestId::String("req-1".into()),
        method: "mcp/status".into(),
        params: json!({}),
    });
    let s = serde_json::to_string(&msg).unwrap();
    let back: JsonRpcMessage = serde_json::from_str(&s).unwrap();
    match back {
        JsonRpcMessage::Request(r) => {
            assert_eq!(r.jsonrpc, JSONRPC_VERSION);
            assert_eq!(r.request_id, RequestId::String("req-1".into()));
            assert_eq!(r.method, "mcp/status");
        }
        _ => panic!("expected Request"),
    }
}

#[test]
fn jsonrpc_requires_version_member() {
    let err = serde_json::from_value::<JsonRpcMessage>(json!({
        "id": 1,
        "method": "control/keepAlive",
        "params": {}
    }))
    .expect_err("missing jsonrpc must fail");
    assert!(err.to_string().contains("data did not match any variant"));
}

#[test]
fn jsonrpc_rejects_non_v2_version() {
    let err = serde_json::from_value::<JsonRpcMessage>(json!({
        "jsonrpc": "1.0",
        "id": 1,
        "method": "control/keepAlive",
        "params": {}
    }))
    .expect_err("wrong jsonrpc version must fail");
    assert!(err.to_string().contains("data did not match any variant"));
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
