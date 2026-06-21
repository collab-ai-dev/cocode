use super::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

fn make_jwt(payload: serde_json::Value) -> String {
    let header = URL_SAFE_NO_PAD.encode(b"{\"alg\":\"none\"}");
    let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("{header}.{body}.sig")
}

#[test]
fn reads_nested_account_id_claim() {
    let jwt = make_jwt(serde_json::json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct_123" }
    }));
    assert_eq!(
        read_string_claim(&jwt, &["https://api.openai.com/auth", "chatgpt_account_id"]).as_deref(),
        Some("acct_123")
    );
}

#[test]
fn reads_exp_as_milliseconds() {
    let jwt = make_jwt(serde_json::json!({ "exp": 1_700_000_000i64 }));
    assert_eq!(read_exp_ms(&jwt), Some(1_700_000_000_000));
}

#[test]
fn missing_claim_and_garbage_are_none() {
    let jwt = make_jwt(serde_json::json!({ "x": 1 }));
    assert_eq!(read_string_claim(&jwt, &["nope"]), None);
    assert_eq!(read_string_claim("not-a-jwt", &["x"]), None);
    assert_eq!(read_exp_ms("not-a-jwt"), None);
}
