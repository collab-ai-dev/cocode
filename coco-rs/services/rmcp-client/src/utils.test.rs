use super::*;
use coco_mcp_types::ContentBlock;
use pretty_assertions::assert_eq;
use rmcp::model::CallToolResult as RmcpCallToolResult;
use serde_json::json;

use serial_test::serial;
use std::ffi::OsString;

struct EnvVarGuard {
    key: String,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &str, value: &str) -> Self {
        let original = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self {
            key: key.to_string(),
            original,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            unsafe {
                std::env::set_var(&self.key, value);
            }
        } else {
            unsafe {
                std::env::remove_var(&self.key);
            }
        }
    }
}

#[tokio::test]
async fn create_env_honors_overrides() {
    let value = "custom".to_string();
    let env = create_env_for_mcp_server(Some(HashMap::from([("TZ".into(), value.clone())])), &[]);
    assert_eq!(env.get("TZ"), Some(&value));
}

#[test]
#[serial(extra_rmcp_env)]
fn create_env_includes_additional_whitelisted_variables() {
    let custom_var = "EXTRA_RMCP_ENV";
    let value = "from-env";
    let _guard = EnvVarGuard::set(custom_var, value);
    let env = create_env_for_mcp_server(None, &[custom_var.to_string()]);
    assert_eq!(env.get(custom_var), Some(&value.to_string()));
}

#[test]
fn convert_call_tool_result_defaults_missing_content() -> Result<()> {
    let structured_content = json!({ "key": "value" });
    // rmcp 1.7's CallToolResult is #[non_exhaustive], so build via a
    // constructor then clear `content` (structured_error mirrors the value
    // into content; this test specifically exercises the empty-content path).
    let mut rmcp_result = RmcpCallToolResult::structured_error(structured_content.clone());
    rmcp_result.content = vec![];

    let result = convert_call_tool_result(rmcp_result)?;

    assert!(result.content.is_empty());
    assert_eq!(result.structured_content, Some(structured_content));
    assert_eq!(result.is_error, Some(true));

    Ok(())
}

#[test]
fn convert_call_tool_result_preserves_existing_content() -> Result<()> {
    let rmcp_result = RmcpCallToolResult::success(vec![rmcp::model::Content::text("hello")]);

    let result = convert_call_tool_result(rmcp_result)?;

    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        ContentBlock::TextContent(text_content) => {
            assert_eq!(text_content.text, "hello");
            assert_eq!(text_content.r#type, "text");
        }
        other => panic!("expected text content got {other:?}"),
    }
    assert_eq!(result.structured_content, None);
    assert_eq!(result.is_error, Some(false));

    Ok(())
}
