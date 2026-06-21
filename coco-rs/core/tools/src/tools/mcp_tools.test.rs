use super::*;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::McpToolAnnotations;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::mcp_handle::McpContentBlock;
use coco_tool_runtime::mcp_handle::McpResourceContent;
use coco_tool_runtime::mcp_handle::McpResourceInfo;
use coco_tool_runtime::mcp_handle::McpToolCallResult;
use serde_json::json;
use std::sync::Arc;

/// Test wrapper: tests always pass valid schemas, so unwrap the v4.2
/// fallible `McpTool::new`.
fn mcp_tool(
    server: String,
    tool: String,
    desc: String,
    schema: serde_json::Value,
    annotations: McpToolAnnotations,
) -> McpTool {
    McpTool::new(server, tool, desc, schema, annotations).expect("test mcp schema must be valid")
}

fn make_mcp_tool() -> McpTool {
    mcp_tool(
        "test-server".into(),
        "test-tool".into(),
        "test description".into(),
        json!({"properties": {}}),
        McpToolAnnotations::default(),
    )
}

// ---------------------------------------------------------------------------
// alwaysLoad — isDeferredTool opt-out
// ---------------------------------------------------------------------------

#[test]
fn always_load_defaults_to_false_so_mcp_tools_are_deferred() {
    let tool = make_mcp_tool();
    let tool: &dyn DynTool = &tool;
    assert!(tool.should_defer(), "MCP tools defer by default");
    assert!(
        !tool.always_load(),
        "Default annotations do not opt out of deferral"
    );
}

#[test]
fn always_load_propagates_from_meta_opt_out() {
    let schema = json!({
        "_meta": {"anthropic/alwaysLoad": true},
        "properties": {}
    });
    let annotations = McpToolAnnotations::from_input_schema_meta(&schema);
    assert!(annotations.always_load);

    let tool = mcp_tool(
        "test-server".into(),
        "always-on".into(),
        "always-loaded tool".into(),
        schema,
        annotations,
    );

    let tool: &dyn DynTool = &tool;
    assert!(tool.always_load(), "always_load flows through annotations");
    // `should_defer()` stays `true` — the registry filter is what
    // decides whether to surface the schema: `should_defer() &&
    // !always_load()`.
    assert!(tool.should_defer());
}

// ---------------------------------------------------------------------------
// Schema wire-envelope preservation — guards against the DeepSeek
// `type: null` regression. McpTool::input_json_schema must hand back
// the server's wire schema verbatim (with `type: object` folded in if
// omitted) so strict OpenAI-compatible providers accept it.
// ---------------------------------------------------------------------------

#[test]
fn input_json_schema_returns_wire_envelope_verbatim() {
    let schema = json!({
        "type": "object",
        "properties": {
            "param": { "type": "string", "description": "demo" }
        },
        "required": ["param"],
        "additionalProperties": false
    });
    let tool = mcp_tool(
        "server".into(),
        "tool".into(),
        "desc".into(),
        schema.clone(),
        McpToolAnnotations::default(),
    );
    let tool: &dyn DynTool = &tool;
    assert_eq!(tool.runtime_validation_schema().as_value(), &schema);
}

#[test]
fn input_json_schema_folds_in_type_object_when_omitted() {
    // MCP `tools/list` implicitly assumes `type: object`; some servers
    // (and tests above) omit it. Strict providers reject — fold it in
    // at construction time.
    let schema = json!({
        "properties": { "param": { "type": "string" } }
    });
    let tool = mcp_tool(
        "server".into(),
        "tool".into(),
        "desc".into(),
        schema,
        McpToolAnnotations::default(),
    );
    let tool: &dyn DynTool = &tool;
    let envelope = tool.runtime_validation_schema().as_value();
    assert_eq!(
        envelope.get("type").and_then(|v| v.as_str()),
        Some("object")
    );
}

#[test]
fn required_array_preserved_from_wire() {
    // McpTool::new used to hardcode `required: Vec::new()`, silently
    // dropping the server's required-field list. Schema views must
    // surface the wire value.
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "age": { "type": "integer" }
        },
        "required": ["name"]
    });
    let tool = mcp_tool(
        "server".into(),
        "tool".into(),
        "desc".into(),
        schema,
        McpToolAnnotations::default(),
    );
    let tool: &dyn DynTool = &tool;
    let view = tool.runtime_validation_schema().as_value();
    let required: Vec<&str> = view["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(required, vec!["name"]);
}

#[test]
fn non_object_schema_falls_back_to_empty_envelope() {
    // Defensive: a misbehaving server that sends a bare string or
    // array as the schema must not crash the tool — fall back to the
    // canonical empty-params envelope.
    let tool = mcp_tool(
        "server".into(),
        "tool".into(),
        "desc".into(),
        json!("bogus"),
        McpToolAnnotations::default(),
    );
    let tool: &dyn DynTool = &tool;
    let envelope = tool.runtime_validation_schema().as_value();
    assert_eq!(
        envelope.get("type").and_then(|v| v.as_str()),
        Some("object")
    );
    assert!(envelope.get("properties").is_some());
}

#[test]
fn always_load_meta_extractor_ignores_non_bool_values() {
    // Defensive: arbitrary `_meta["anthropic/alwaysLoad"]` payloads
    // from misbehaving servers (string "true", number 1) must NOT
    // accidentally opt the tool out.
    let schemas = [
        json!({"_meta": {"anthropic/alwaysLoad": "true"}}),
        json!({"_meta": {"anthropic/alwaysLoad": 1}}),
        json!({"_meta": {}}),
        json!({}),
    ];
    for schema in schemas {
        let a = McpToolAnnotations::from_input_schema_meta(&schema);
        assert!(
            !a.always_load,
            "non-bool _meta should not opt out: {schema}"
        );
    }
}

// ---------------------------------------------------------------------------
// ListMcpResources / ReadMcpResource render — `jsonStringify` output shape
// ---------------------------------------------------------------------------

#[test]
fn list_mcp_resources_render_empty_string_path() {
    // execute() emits a bare string for the empty/error branches; the
    // render unwraps it so the wire output is the bare phrase, not a
    // JSON-quoted "...".
    let data = json!(
        "No resources found. MCP servers may still provide tools even if they have no resources."
    );
    let parts = <ListMcpResourcesTool as DynTool>::render_for_model(&ListMcpResourcesTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.starts_with("No resources found"));
    assert!(!text.starts_with('"'));
}

#[test]
fn list_mcp_resources_render_array_uses_json_stringify() {
    // Non-empty branch: `jsonStringify(content)`.
    let data = json!([
        {"uri": "u1", "name": "n1", "description": "d1", "mime_type": "text/plain"},
    ]);
    let parts = <ListMcpResourcesTool as DynTool>::render_for_model(&ListMcpResourcesTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.starts_with('['), "expected JSON array, got: {text}");
    assert!(text.contains("\"uri\":\"u1\""));
}

#[test]
fn read_mcp_resource_render_uses_json_stringify() {
    let data = json!({"uri": "u1", "text": "hello", "mime_type": "text/plain"});
    let parts = <ReadMcpResourceTool as DynTool>::render_for_model(&ReadMcpResourceTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.starts_with('{'), "expected JSON object, got: {text}");
    assert!(text.contains("\"text\":\"hello\""));
}

#[tokio::test]
async fn mcp_auth_tool_forwards_to_generic_handle() {
    let mut ctx = ToolUseContext::test_default();
    ctx.mcp = Arc::new(AuthHandle {
        message: "Authentication started".into(),
    });
    assert!(!<McpAuthTool as DynTool>::is_read_only(
        &McpAuthTool,
        &json!({"server_name": "srv"})
    ));
    assert_eq!(
        <McpAuthTool as DynTool>::to_auto_classifier_input(
            &McpAuthTool,
            &json!({"server_name": "srv"})
        ),
        Some("srv".to_string())
    );
    let permission = <McpAuthTool as DynTool>::check_permissions(
        &McpAuthTool,
        &json!({"server_name": "srv"}),
        &ctx,
    )
    .await;
    let coco_types::ToolCheckResult::Allow { updated_input, .. } = permission else {
        panic!("McpAuthTool should explicitly allow its auth-start input");
    };
    assert_eq!(updated_input, Some(json!({"server_name": "srv"})));

    let result =
        <McpAuthTool as DynTool>::execute(&McpAuthTool, json!({"server_name": "srv"}), &ctx)
            .await
            .unwrap();

    assert_eq!(result.data, json!("Authentication started"));
}

// ---------------------------------------------------------------------------
// McpAuthServerTool — per-server authenticate pseudo-tool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_auth_server_tool_bakes_server_name_into_authenticate() {
    // The pre-bound server (not model input) must be what reaches the handle —
    // this is the whole point of the per-server pseudo-tool.
    let tool = McpAuthServerTool::new("github".into(), "http", Some("https://api.github.com/mcp"));
    let mut ctx = ToolUseContext::test_default();
    ctx.mcp = Arc::new(EchoServerAuthHandle);
    let tool: &dyn DynTool = &tool;
    let result = tool.execute(json!({}), &ctx).await.unwrap();
    // The echo handle returns the server name it was called with.
    assert_eq!(result.data, json!("authenticating github"));
}

#[test]
fn mcp_auth_server_tool_is_deferred() {
    let tool = McpAuthServerTool::new("github".into(), "http", None);
    let tool: &dyn DynTool = &tool;
    assert!(
        tool.should_defer(),
        "auth pseudo-tool loads through ToolSearch"
    );
}

#[test]
fn mcp_auth_tool_is_deferred() {
    let tool: &dyn DynTool = &McpAuthTool;
    assert!(
        tool.should_defer(),
        "generic MCP auth loads through ToolSearch"
    );
}

#[test]
fn mcp_auth_server_tool_is_searchable_by_qualified_name() {
    let registry = coco_tool_runtime::ToolRegistry::new();
    registry.register(std::sync::Arc::new(McpAuthServerTool::new(
        "github".into(),
        "http",
        None,
    )));
    let ctx = coco_tool_runtime::ToolUseContext::test_default()
        .with_model_capabilities(false, true)
        .with_tool_search_candidates(true);

    let names: Vec<String> = registry
        .searchable_deferred(&ctx)
        .into_iter()
        .map(|tool| tool.name().to_string())
        .collect();

    assert_eq!(names, vec!["mcp__github__authenticate"]);
}

#[test]
fn mcp_auth_server_tool_id_is_qualified_and_server_owned() {
    let tool = McpAuthServerTool::new("github".into(), "http", None);
    let tool: &dyn DynTool = &tool;
    // ToolId::Mcp Display = mcp__<server>__<tool> — must match the wipe prefix.
    assert_eq!(tool.id().to_string(), "mcp__github__authenticate");
    assert_eq!(tool.name(), "mcp__github__authenticate");
    assert_eq!(
        tool.mcp_info().map(|i| i.server_name.as_str()),
        Some("github"),
        "server ownership drives the replace_server_tools wipe"
    );
}

#[test]
fn mcp_auth_server_tool_description_names_server_and_url() {
    let tool = McpAuthServerTool::new("github".into(), "http", Some("https://api.github.com/mcp"));
    let desc = <McpAuthServerTool as DynTool>::description(
        &tool,
        &json!({}),
        &coco_tool_runtime::DescriptionOptions::default(),
    );
    assert!(desc.contains("github"), "got: {desc}");
    assert!(desc.contains("https://api.github.com/mcp"), "got: {desc}");
    assert!(desc.contains("requires"), "got: {desc}");
}

#[test]
fn auth_pseudo_tool_is_wiped_when_real_tools_register() {
    // The swap fabric: register the per-server auth tool, then register real
    // tools for the same server — replace_server_tools must remove the auth
    // tool (it shares the server ownership) and install the real tool. Mirrors
    // the mcp__<server>__* prefix wipe on a successful reconnect.
    let registry = coco_tool_runtime::ToolRegistry::new();
    crate::register_mcp_auth_tool(&registry, "github", "http", Some("https://x"));
    let auth_id = coco_types::ToolId::Mcp {
        server: "github".into(),
        tool: "authenticate".into(),
    };
    assert!(
        registry.get(&auth_id).is_some(),
        "auth pseudo-tool present after surfacing"
    );

    crate::register_mcp_tools(
        &registry,
        "github",
        vec![coco_tool_runtime::McpToolSchema {
            server_name: "github".into(),
            tool_name: "create_issue".into(),
            description: Some("create an issue".into()),
            input_schema: json!({"type": "object", "properties": {}}),
            annotations: McpToolAnnotations::default(),
        }],
    );

    assert!(
        registry.get(&auth_id).is_none(),
        "auth pseudo-tool must be wiped when real tools register"
    );
    assert!(
        registry
            .get(&coco_types::ToolId::Mcp {
                server: "github".into(),
                tool: "create_issue".into(),
            })
            .is_some(),
        "real tool installed by the same swap"
    );
}

// ---------------------------------------------------------------------------
// render_for_model — pass-through MCP server-provided multimodal content
// ---------------------------------------------------------------------------

#[test]
fn render_decodes_text_block_array_into_text_part() {
    // Success path: data is a bare array of `{type, text/data, ...}` blocks.
    let tool = make_mcp_tool();
    let tool: &dyn DynTool = &tool;
    let data = json!([
        {"type": "text", "text": "first chunk"},
        {"type": "text", "text": "second chunk"},
    ]);
    let parts = tool.render_for_model(&data);
    assert_eq!(parts.len(), 2);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part 0");
    };
    assert_eq!(text, "first chunk");
    let ToolResultContentPart::Text { text, .. } = &parts[1] else {
        panic!("expected Text part 1");
    };
    assert_eq!(text, "second chunk");
}

#[test]
fn render_decodes_image_block_into_filedata_part() {
    let tool = make_mcp_tool();
    let tool: &dyn DynTool = &tool;
    let data = json!([
        {"type": "image", "data": "iVBOR...", "mime_type": "image/png"},
    ]);
    let parts = tool.render_for_model(&data);
    assert_eq!(parts.len(), 1);
    match &parts[0] {
        ToolResultContentPart::FileData {
            data,
            media_type,
            filename,
            ..
        } => {
            assert_eq!(data, "iVBOR...");
            assert_eq!(media_type, "image/png");
            assert!(filename.is_none());
        }
        other => panic!("expected FileData, got {other:?}"),
    }
}

#[test]
fn render_handles_mixed_text_and_image_in_order() {
    // Text + image + text — order must be preserved so the model sees
    // captions adjacent to the screenshot they describe.
    let tool = make_mcp_tool();
    let tool: &dyn DynTool = &tool;
    let data = json!([
        {"type": "text", "text": "Screenshot of the page:"},
        {"type": "image", "data": "iVBOR...", "mime_type": "image/png"},
        {"type": "text", "text": "Notice the error banner."},
    ]);
    let parts = tool.render_for_model(&data);
    assert_eq!(parts.len(), 3);
    assert!(matches!(&parts[0], ToolResultContentPart::Text { .. }));
    assert!(matches!(&parts[1], ToolResultContentPart::FileData { .. }));
    assert!(matches!(&parts[2], ToolResultContentPart::Text { .. }));
}

#[test]
fn render_decodes_error_envelope_content() {
    // Error path: data is `{error: true, content: [...]}`.
    let tool = make_mcp_tool();
    let tool: &dyn DynTool = &tool;
    let data = json!({
        "error": true,
        "content": [
            {"type": "text", "text": "Tool execution failed: timeout"},
        ],
    });
    let parts = tool.render_for_model(&data);
    assert_eq!(parts.len(), 1);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "Tool execution failed: timeout");
}

#[test]
fn render_unknown_block_falls_back_to_json_string() {
    // Defensive: a future block type the decoder doesn't recognize.
    // Empty parts list triggers the JSON fallback so the model still
    // sees something rather than getting a silent empty result.
    let tool = make_mcp_tool();
    let tool: &dyn DynTool = &tool;
    let data = json!([
        {"type": "audio", "data": "...", "mime_type": "audio/wav"},
    ]);
    let parts = tool.render_for_model(&data);
    assert_eq!(parts.len(), 1);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text fallback");
    };
    // JSON fallback contains the raw payload so nothing is dropped.
    assert!(text.contains("audio"), "got: {text}");
}

#[tokio::test]
async fn read_mcp_resource_persists_blob_to_session_tool_results() {
    use base64::Engine as _;

    let tmp = tempfile::TempDir::new().unwrap();
    let bytes = b"\x89PNG\r\n\x1a\nblob";
    let mut ctx = ToolUseContext::test_default();
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some("session-1".into());
    ctx.tool_result_session_dir = Some(tmp.path().join("sessions/session-1"));
    ctx.tool_use_id = Some("tool-1".into());
    ctx.mcp = Arc::new(BlobResourceHandle {
        blob: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: "image/png".into(),
    });

    let result = <ReadMcpResourceTool as DynTool>::execute(
        &ReadMcpResourceTool,
        json!({
            "server": "srv",
            "uri": "mcp://file",
        }),
        &ctx,
    )
    .await
    .unwrap();
    let rendered =
        <ReadMcpResourceTool as DynTool>::render_for_model(&ReadMcpResourceTool, &result.data);
    let ToolResultContentPart::Text { text, .. } = &rendered[0] else {
        panic!("expected Text part");
    };
    assert!(text.starts_with("<persisted-output>"), "got: {text}");
    assert!(text.contains("MCP output is binary"), "got: {text}");
    let path = tmp
        .path()
        .join("sessions/session-1/tool-results/tool-1.png");
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn read_mcp_resource_preserves_multiple_contents() {
    use base64::Engine as _;

    let tmp = tempfile::TempDir::new().unwrap();
    let bytes = b"audio";
    let mut ctx = ToolUseContext::test_default();
    ctx.tool_result_session_dir = Some(tmp.path().join("sessions/session-1"));
    ctx.tool_use_id = Some("tool-multi".into());
    ctx.mcp = Arc::new(MixedResourceHandle {
        blob: base64::engine::general_purpose::STANDARD.encode(bytes),
    });

    let result = <ReadMcpResourceTool as DynTool>::execute(
        &ReadMcpResourceTool,
        json!({
            "server": "srv",
            "uri": "mcp://bundle",
        }),
        &ctx,
    )
    .await
    .unwrap();

    let contents = result.data["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 2);
    assert_eq!(contents[0]["text"], "first");
    assert!(
        contents[1]["persisted_output"]
            .as_str()
            .unwrap()
            .contains("audio/wav")
    );
    let path = tmp
        .path()
        .join("sessions/session-1/tool-results/tool-multi-2.wav");
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test]
async fn dynamic_mcp_tool_persists_embedded_resource_blob() {
    use base64::Engine as _;

    let tmp = tempfile::TempDir::new().unwrap();
    let bytes = b"%PDF-1.7";
    let mut ctx = ToolUseContext::test_default();
    ctx.config_home = Some(tmp.path().to_path_buf());
    ctx.session_id_for_history = Some("session-1".into());
    ctx.tool_result_session_dir = Some(tmp.path().join("sessions/session-1"));
    ctx.tool_use_id = Some("tool-2".into());
    ctx.mcp = Arc::new(BlobToolHandle {
        blob: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime_type: "application/pdf".into(),
    });

    let tool = make_mcp_tool();

    let tool: &dyn DynTool = &tool;
    let result = tool.execute(json!({}), &ctx).await.unwrap();
    let parts = tool.render_for_model(&result.data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.starts_with("<persisted-output>"), "got: {text}");
    assert!(text.contains("application/pdf"), "got: {text}");
    let path = tmp
        .path()
        .join("sessions/session-1/tool-results/tool-2.pdf");
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

struct BlobResourceHandle {
    blob: String,
    mime_type: String,
}

struct AuthHandle {
    message: String,
}

/// Echoes back the server name passed to `authenticate`, so a test can assert
/// the pre-bound server (not model input) is what reaches the handle.
struct EchoServerAuthHandle;

#[async_trait::async_trait]
impl coco_tool_runtime::McpHandle for EchoServerAuthHandle {
    async fn list_resources(
        &self,
        _: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(vec![])
    }

    async fn read_resource(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        unreachable!("not used by McpAuthServerTool test")
    }

    async fn call_tool(
        &self,
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        unreachable!("not used by McpAuthServerTool test")
    }

    async fn authenticate(&self, server: &str) -> Result<String, coco_error::BoxedError> {
        Ok(format!("authenticating {server}"))
    }

    async fn connected_servers(&self) -> Vec<String> {
        vec![]
    }
}

#[async_trait::async_trait]
impl coco_tool_runtime::McpHandle for AuthHandle {
    async fn list_resources(
        &self,
        _: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(vec![])
    }

    async fn read_resource(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        unreachable!("not used by McpAuthTool test")
    }

    async fn call_tool(
        &self,
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        unreachable!("not used by McpAuthTool test")
    }

    async fn authenticate(&self, _: &str) -> Result<String, coco_error::BoxedError> {
        Ok(self.message.clone())
    }

    async fn connected_servers(&self) -> Vec<String> {
        vec![]
    }
}

#[async_trait::async_trait]
impl coco_tool_runtime::McpHandle for BlobResourceHandle {
    async fn list_resources(
        &self,
        _: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(vec![])
    }

    async fn read_resource(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        Ok(vec![McpResourceContent {
            uri: "mcp://file".into(),
            text: None,
            blob: Some(self.blob.clone()),
            mime_type: Some(self.mime_type.clone()),
        }])
    }

    async fn call_tool(
        &self,
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        unreachable!("not used by ReadMcpResourceTool test")
    }

    async fn authenticate(&self, _: &str) -> Result<String, coco_error::BoxedError> {
        unreachable!("not used by ReadMcpResourceTool test")
    }

    async fn connected_servers(&self) -> Vec<String> {
        vec![]
    }
}

struct BlobToolHandle {
    blob: String,
    mime_type: String,
}

struct MixedResourceHandle {
    blob: String,
}

#[async_trait::async_trait]
impl coco_tool_runtime::McpHandle for MixedResourceHandle {
    async fn list_resources(
        &self,
        _: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(vec![])
    }

    async fn read_resource(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        Ok(vec![
            McpResourceContent {
                uri: "mcp://bundle/text".into(),
                text: Some("first".into()),
                blob: None,
                mime_type: Some("text/plain".into()),
            },
            McpResourceContent {
                uri: "mcp://bundle/audio".into(),
                text: None,
                blob: Some(self.blob.clone()),
                mime_type: Some("audio/wav".into()),
            },
        ])
    }

    async fn call_tool(
        &self,
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        unreachable!("not used by multi resource test")
    }

    async fn authenticate(&self, _: &str) -> Result<String, coco_error::BoxedError> {
        unreachable!("not used by multi resource test")
    }

    async fn connected_servers(&self) -> Vec<String> {
        vec![]
    }
}

#[async_trait::async_trait]
impl coco_tool_runtime::McpHandle for BlobToolHandle {
    async fn list_resources(
        &self,
        _: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        Ok(vec![])
    }

    async fn read_resource(
        &self,
        _: &str,
        _: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        unreachable!("not used by dynamic MCP tool test")
    }

    async fn call_tool(
        &self,
        _: &str,
        _: &str,
        _: Option<serde_json::Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        Ok(McpToolCallResult {
            content: vec![McpContentBlock::Resource {
                uri: "mcp://report".into(),
                text: None,
                blob: Some(self.blob.clone()),
                mime_type: Some(self.mime_type.clone()),
            }],
            is_error: false,
        })
    }

    async fn authenticate(&self, _: &str) -> Result<String, coco_error::BoxedError> {
        unreachable!("not used by dynamic MCP tool test")
    }

    async fn connected_servers(&self) -> Vec<String> {
        vec![]
    }
}
