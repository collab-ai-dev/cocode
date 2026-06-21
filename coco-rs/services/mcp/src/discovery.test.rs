use std::sync::Arc;

use pretty_assertions::assert_eq;
use tokio::sync::RwLock;

use super::*;
use crate::types::McpToolDefinition;

fn make_tool(name: &str, description: Option<&str>) -> McpToolDefinition {
    McpToolDefinition {
        name: name.to_string(),
        description: description.map(String::from),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "arg1": { "type": "string" }
            }
        }),
    }
}

fn make_tool_with_annotations(name: &str, read_only: bool, destructive: bool) -> McpToolDefinition {
    McpToolDefinition {
        name: name.to_string(),
        description: Some(format!("Tool {name}")),
        input_schema: serde_json::json!({
            "type": "object",
            "annotations": {
                "readOnlyHint": read_only,
                "destructiveHint": destructive,
            }
        }),
    }
}

fn make_tool_with_search_hint(name: &str, hint: &str) -> McpToolDefinition {
    McpToolDefinition {
        name: name.to_string(),
        description: Some("A tool".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "_meta": {
                "anthropic/searchHint": hint,
                "anthropic/alwaysLoad": true,
            }
        }),
    }
}

#[test]
fn test_convert_server_tools_basic() {
    let tools = vec![
        make_tool("read_file", Some("Read a file from disk")),
        make_tool("write_file", Some("Write data to a file")),
    ];

    let discovered = convert_server_tools("my-server", &tools);

    assert_eq!(discovered.len(), 2);
    assert_eq!(discovered[0].fq_name, "mcp__my-server__read_file");
    assert_eq!(discovered[0].server_name, "my-server");
    assert_eq!(discovered[0].tool_name, "read_file");
    assert_eq!(discovered[0].description, "Read a file from disk");
    assert_eq!(discovered[1].fq_name, "mcp__my-server__write_file");
}

#[test]
fn test_convert_server_tools_description_truncation() {
    let long_desc = "x".repeat(5000);
    let tools = vec![make_tool("tool1", Some(&long_desc))];

    let discovered = convert_server_tools("server", &tools);
    assert!(discovered[0].description.len() < 5000);
    assert!(discovered[0].description.ends_with("… [truncated]"));
}

#[test]
fn test_convert_server_tools_no_description() {
    let tools = vec![make_tool("tool1", None)];
    let discovered = convert_server_tools("server", &tools);
    assert_eq!(discovered[0].description, "");
}

#[test]
fn test_convert_server_tools_annotations() {
    let tools = vec![make_tool_with_annotations("reader", true, false)];
    let discovered = convert_server_tools("server", &tools);

    assert!(discovered[0].annotations.read_only);
    assert!(!discovered[0].annotations.destructive);
    assert!(!discovered[0].annotations.open_world);
}

#[test]
fn test_convert_server_tools_search_hint() {
    let tools = vec![make_tool_with_search_hint(
        "search_tool",
        "find  things\n here",
    )];
    let discovered = convert_server_tools("server", &tools);

    assert_eq!(
        discovered[0].search_hint.as_deref(),
        Some("find things here")
    );
    assert!(discovered[0].always_load);
}

#[test]
fn test_convert_server_tools_generic_search_hint_key() {
    // Provider-neutral `searchHint` (no `anthropic/` namespace) is accepted
    // so non-Anthropic MCP servers can supply a hint too.
    let tool = McpToolDefinition {
        name: "generic".to_string(),
        description: Some("A tool".to_string()),
        input_schema: serde_json::json!({
            "type": "object",
            "_meta": { "searchHint": "look up records by id" }
        }),
    };
    let discovered = convert_server_tools("server", &[tool]);
    assert_eq!(
        discovered[0].search_hint.as_deref(),
        Some("look up records by id")
    );
}

#[test]
fn test_extract_annotations_missing() {
    let schema = serde_json::json!({"type": "object"});
    let annotations = extract_annotations(&schema);
    assert!(!annotations.read_only);
    assert!(!annotations.destructive);
    assert!(!annotations.open_world);
    assert!(annotations.title.is_none());
}

#[test]
fn test_extract_annotations_with_title() {
    let schema = serde_json::json!({
        "type": "object",
        "annotations": {
            "readOnlyHint": true,
            "title": "Custom Title",
        }
    });
    let annotations = extract_annotations(&schema);
    assert!(annotations.read_only);
    assert_eq!(annotations.title.as_deref(), Some("Custom Title"));
}

#[test]
fn test_discovery_cache_basic() {
    let mut cache = DiscoveryCache::default();

    assert!(cache.get_tools("server1").is_none());
    assert!(cache.get_resources("server1").is_none());

    cache.set_tools("server1", vec![]);
    assert!(cache.get_tools("server1").is_some());

    cache.set_resources("server1", vec![]);
    assert!(cache.get_resources("server1").is_some());
}

#[test]
fn test_discovery_cache_invalidate() {
    let mut cache = DiscoveryCache::default();
    cache.set_tools("server1", vec![]);
    cache.set_resources("server1", vec![]);

    cache.invalidate("server1");

    assert!(cache.get_tools("server1").is_none());
    assert!(cache.get_resources("server1").is_none());
}

#[test]
fn test_discovery_cache_clear() {
    let mut cache = DiscoveryCache::default();
    cache.set_tools("server1", vec![]);
    cache.set_tools("server2", vec![]);
    cache.set_resources("server1", vec![]);

    cache.clear();

    assert!(cache.get_tools("server1").is_none());
    assert!(cache.get_tools("server2").is_none());
    assert!(cache.get_resources("server1").is_none());
}

#[tokio::test]
async fn test_discover_tools_unknown_server() {
    let manager = McpConnectionManager::new(std::path::PathBuf::from("/tmp/coco-test"));
    let cache = Arc::new(RwLock::new(DiscoveryCache::default()));

    let result = discover_tools_from_server(&manager, "nonexistent", &cache).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_discover_resources_unknown_server() {
    let manager = McpConnectionManager::new(std::path::PathBuf::from("/tmp/coco-test"));
    let cache = Arc::new(RwLock::new(DiscoveryCache::default()));

    let result = discover_resources(&manager, "nonexistent", &cache).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_discover_all_empty() {
    let manager = McpConnectionManager::new(std::path::PathBuf::from("/tmp/coco-test"));
    let cache = Arc::new(RwLock::new(DiscoveryCache::default()));

    let results = discover_all(&manager, &cache).await;
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_refresh_server_capabilities_unknown_server() {
    let manager = McpConnectionManager::new(std::path::PathBuf::from("/tmp/coco-test"));
    let cache = Arc::new(RwLock::new(DiscoveryCache::default()));

    // Pre-populate cache
    {
        let mut c = cache.write().await;
        c.set_tools("server", vec![]);
    }

    let result = refresh_server_capabilities(&manager, "server", &cache).await;
    // Should fail since server doesn't exist, but cache should be invalidated
    assert!(result.is_err());

    let c = cache.read().await;
    assert!(c.get_tools("server").is_none());
}

// ── DynamicResourceQuery ──

fn make_resource(server: &str, uri: &str, name: &str, mime: Option<&str>) -> DiscoveredResource {
    DiscoveredResource {
        server_name: server.to_string(),
        uri: uri.to_string(),
        name: name.to_string(),
        description: None,
        mime_type: mime.map(String::from),
    }
}

#[test]
fn test_dynamic_query_empty_matches_all() {
    let query = DynamicResourceQuery::default();
    let resource = make_resource("srv", "file:///a.txt", "a.txt", Some("text/plain"));
    assert!(query.matches(&resource));
}

#[test]
fn test_dynamic_query_uri_prefix_match() {
    let query = DynamicResourceQuery {
        uri_prefix: Some("file://".to_string()),
        ..Default::default()
    };
    assert!(query.matches(&make_resource("s", "file:///a.txt", "a", None)));
    assert!(!query.matches(&make_resource("s", "https://x.com/a", "a", None)));
}

#[test]
fn test_dynamic_query_name_contains_case_insensitive() {
    let query = DynamicResourceQuery {
        name_contains: Some("readme".to_string()),
        ..Default::default()
    };
    assert!(query.matches(&make_resource("s", "f://x", "README.md", None)));
    assert!(query.matches(&make_resource("s", "f://x", "Project Readme", None)));
    assert!(!query.matches(&make_resource("s", "f://x", "config.json", None)));
}

#[test]
fn test_dynamic_query_mime_type_exact() {
    let query = DynamicResourceQuery {
        mime_type: Some("application/json".to_string()),
        ..Default::default()
    };
    assert!(query.matches(&make_resource("s", "f://x", "a", Some("application/json"))));
    assert!(!query.matches(&make_resource("s", "f://x", "a", Some("text/plain"))));
    assert!(!query.matches(&make_resource("s", "f://x", "a", None)));
}

#[test]
fn test_dynamic_query_combined_filters() {
    let query = DynamicResourceQuery {
        uri_prefix: Some("https://".to_string()),
        name_contains: Some("api".to_string()),
        mime_type: Some("application/json".to_string()),
    };

    // All filters match
    assert!(query.matches(&make_resource(
        "s",
        "https://api.example.com/data",
        "API Data",
        Some("application/json"),
    )));

    // URI doesn't match
    assert!(!query.matches(&make_resource(
        "s",
        "file:///api.json",
        "API Data",
        Some("application/json"),
    )));

    // Name doesn't match
    assert!(!query.matches(&make_resource(
        "s",
        "https://example.com/data",
        "Data",
        Some("application/json"),
    )));

    // MIME doesn't match
    assert!(!query.matches(&make_resource(
        "s",
        "https://api.example.com/data",
        "API Data",
        Some("text/html"),
    )));
}
