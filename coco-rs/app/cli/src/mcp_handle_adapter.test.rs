use super::*;

use pretty_assertions::assert_eq;

#[tokio::test]
async fn authenticate_forwards_to_manager() {
    let mut manager = coco_mcp::McpConnectionManager::new(std::env::temp_dir());
    manager.register_server(coco_mcp::ScopedMcpServerConfig {
        name: "local".into(),
        config: coco_mcp::McpServerConfig::Stdio(coco_mcp::types::McpStdioConfig {
            command: "echo".into(),
            args: vec![],
            env: Default::default(),
            cwd: None,
        }),
        scope: coco_mcp::ConfigScope::User,
        plugin_source: None,
    });
    let adapter = McpManagerAdapter::new(Arc::new(Mutex::new(manager)));

    let result = adapter.authenticate("local").await.unwrap();

    assert_eq!(
        result,
        "MCP server 'local' does not use OAuth authentication."
    );
}

#[test]
fn convert_read_resource_result_preserves_all_contents() {
    let result = coco_mcp::ReadResourceResult {
        contents: vec![
            coco_mcp::ReadResourceResultContents::TextResourceContents(
                coco_mcp::TextResourceContents {
                    uri: "mcp://text".into(),
                    text: "hello".into(),
                    mime_type: Some("text/plain".into()),
                },
            ),
            coco_mcp::ReadResourceResultContents::BlobResourceContents(
                coco_mcp::BlobResourceContents {
                    uri: "mcp://blob".into(),
                    blob: "YWJj".into(),
                    mime_type: Some("application/octet-stream".into()),
                },
            ),
        ],
    };

    let converted = convert_read_resource_result(result).unwrap();
    assert_eq!(converted.len(), 2);
    assert_eq!(converted[0].uri, "mcp://text");
    assert_eq!(converted[0].text.as_deref(), Some("hello"));
    assert_eq!(converted[1].uri, "mcp://blob");
    assert_eq!(converted[1].blob.as_deref(), Some("YWJj"));
}
