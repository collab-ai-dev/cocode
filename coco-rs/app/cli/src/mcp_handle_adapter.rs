//! Bridge: `coco_mcp::McpConnectionManager` → `coco_tool_runtime::McpHandle`.
//!
//! Lives in `app/cli` because it sits at the seam where both crates
//! are deps. The adapter forwards the handle's read paths to the
//! manager's internal API; mutating paths (register / disconnect)
//! aren't exposed because tools should never reconfigure MCP at
//! runtime — only the supervisor / config watcher does that.
//!
//! The trait indirection exists for testability.

use std::sync::Arc;

use coco_config::RuntimeConfig;
use coco_mcp::McpConnectionManager;
use coco_mcp::discovery::DiscoveryCache;
use coco_skills::SkillManager;
use coco_tool_runtime::McpHandle;
use coco_tool_runtime::McpToolSchema;
use coco_tool_runtime::mcp_handle::McpContentBlock;
use coco_tool_runtime::mcp_handle::McpResourceContent;
use coco_tool_runtime::mcp_handle::McpResourceInfo;
use coco_tool_runtime::mcp_handle::McpToolAnnotations;
use coco_tool_runtime::mcp_handle::McpToolCallResult;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::RwLock;

/// Adapter that wraps a shared `Arc<Mutex<McpConnectionManager>>` and
/// implements `McpHandle`.
pub struct McpManagerAdapter {
    manager: Arc<Mutex<McpConnectionManager>>,
    /// Optional hook registry for firing `Elicitation` /
    /// `ElicitationResult` hooks when an MCP server requests user
    /// input. `None` keeps the legacy no-op behaviour.
    hook_registry: Option<Arc<coco_hooks::HookRegistry>>,
    /// Builder that produces an `OrchestrationContext` per elicitation
    /// firing — same shape used elsewhere when session state is needed
    /// inside detached closures.
    elicitation_ctx_factory:
        Option<Arc<dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync>>,
    /// Phase 7: clone of `ToolAppState.elicitation_pending_count`.
    /// Threaded into `wrap_send_elicitation_with_hooks` so each
    /// in-flight elicitation increments the counter for the
    /// prompt-suggestion fork's `SuppressReason::ElicitationActive`
    /// guard. `None` keeps the counter untracked (tests / paths
    /// without app_state access).
    elicitation_counter: Option<Arc<std::sync::atomic::AtomicU32>>,
    /// Optional skill bridge wiring. When `Some`,
    /// [`McpHandle::add_dynamic_server`] reconciles `skill://` resources
    /// from the newly-connected server with the [`SkillManager`];
    /// [`McpHandle::remove_dynamic_server`] clears them. Gated on
    /// `Feature::McpSkills` + the server's `resources` capability — the
    /// gating is enforced inside [`coco_mcp_skills::sync_one`].
    skill_bridge: Option<SkillBridgeWiring>,
}

/// Resources the adapter needs to discover and register MCP-sourced
/// skills.
struct SkillBridgeWiring {
    skills: Arc<SkillManager>,
    /// Shared discovery cache — owned by the bridge (caller-injected)
    /// so multiple adapters wrapping the same `SkillManager` can share
    /// it.
    cache: Arc<RwLock<DiscoveryCache>>,
    /// Live `RuntimeConfig` reference so feature flips after session
    /// startup take effect immediately (no stale `Arc<Features>`
    /// snapshot).
    runtime_config: Arc<RuntimeConfig>,
}

impl McpManagerAdapter {
    pub fn new(manager: Arc<Mutex<McpConnectionManager>>) -> Self {
        Self {
            manager,
            hook_registry: None,
            elicitation_ctx_factory: None,
            elicitation_counter: None,
            skill_bridge: None,
        }
    }

    /// Install hook context so dynamic-MCP-server elicitations fire
    /// `Elicitation` / `ElicitationResult` hooks. Without this the
    /// adapter falls back to the legacy no-op send_elicitation closure.
    ///
    /// `elicitation_counter` (Phase 7) is the
    /// `ToolAppState.elicitation_pending_count` Arc — pass `None` from
    /// tests / paths that don't surface state to prompt-suggestion.
    pub fn with_elicitation_hooks(
        mut self,
        registry: Arc<coco_hooks::HookRegistry>,
        ctx_factory: Arc<dyn Fn() -> coco_hooks::orchestration::OrchestrationContext + Send + Sync>,
        elicitation_counter: Option<Arc<std::sync::atomic::AtomicU32>>,
    ) -> Self {
        self.hook_registry = Some(registry);
        self.elicitation_ctx_factory = Some(ctx_factory);
        self.elicitation_counter = elicitation_counter;
        self
    }

    /// Wire the MCP skill bridge. After a successful
    /// [`McpHandle::add_dynamic_server`], the adapter calls
    /// [`coco_mcp_skills::sync_one`] which gates on
    /// `Feature::McpSkills` AND the server's advertised `resources`
    /// capability. On [`McpHandle::remove_dynamic_server`], the same
    /// server's skills are dropped.
    ///
    /// `cache` is shared — caller decides ownership so multiple
    /// adapters can share a single discovery cache when wrapping the
    /// same `McpConnectionManager`. `runtime_config` is the live
    /// session config; the feature flag is read on every call (no
    /// snapshot).
    pub fn with_skill_bridge(
        mut self,
        skills: Arc<SkillManager>,
        cache: Arc<RwLock<DiscoveryCache>>,
        runtime_config: Arc<RuntimeConfig>,
    ) -> Self {
        self.skill_bridge = Some(SkillBridgeWiring {
            skills,
            cache,
            runtime_config,
        });
        self
    }

    fn send_elicitation_for_server(&self, name: &str) -> coco_mcp::SendElicitation {
        let base_elicitation: coco_mcp::SendElicitation = Box::new(|_id, _req| {
            Box::pin(async move {
                Err(coco_mcp::RmcpClientError::generic(
                    "elicitation not supported for dynamically-added agent MCP servers",
                ))
            })
        });
        match (
            self.hook_registry.as_ref(),
            self.elicitation_ctx_factory.as_ref(),
        ) {
            (Some(registry), Some(factory)) => {
                crate::elicitation_hooks::wrap_send_elicitation_with_hooks(
                    name.to_string(),
                    registry.clone(),
                    factory.clone(),
                    self.elicitation_counter.clone(),
                    base_elicitation,
                )
            }
            _ => base_elicitation,
        }
    }
}

#[async_trait::async_trait]
impl McpHandle for McpManagerAdapter {
    async fn list_resources(
        &self,
        server_name: Option<&str>,
    ) -> Result<Vec<McpResourceInfo>, coco_error::BoxedError> {
        let manager = self.manager.lock().await.clone();
        let connected = manager.connected_servers().await;
        let mut out = Vec::new();
        for server in connected {
            if let Some(filter) = server_name
                && server.name != filter
            {
                continue;
            }
            for r in &server.resources {
                out.push(McpResourceInfo {
                    server_name: server.name.clone(),
                    uri: r.uri.clone(),
                    name: r.name.clone(),
                    description: r.description.clone(),
                    mime_type: r.mime_type.clone(),
                });
            }
        }
        Ok(out)
    }

    async fn read_resource(
        &self,
        server_name: &str,
        resource_uri: &str,
    ) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
        let manager = self.manager.lock().await.clone();
        let result = manager
            .read_resource(server_name, resource_uri)
            .await
            .map_err(|e| {
                Box::new(coco_error::PlainError::new(
                    format!("MCP resource read failed: {e}"),
                    coco_error::StatusCode::External,
                )) as coco_error::BoxedError
            })?;
        convert_read_resource_result(result)
    }

    async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<McpToolCallResult, coco_error::BoxedError> {
        let manager = self.manager.lock().await.clone();
        let result = manager
            .call_tool(server_name, tool_name, arguments)
            .await
            .map_err(|e| {
                Box::new(coco_error::PlainError::new(
                    format!("MCP tool call failed: {e}"),
                    coco_error::StatusCode::External,
                )) as coco_error::BoxedError
            })?;
        // Translate rmcp `CallToolResult` into the trait's content
        // blocks. Text blocks pass through; non-text blocks render as
        // an `Image` placeholder (the trait only carries text + image
        // shapes today).
        let content = result
            .content
            .into_iter()
            .filter_map(|block| {
                let raw = serde_json::to_value(&block).ok()?;
                let kind = raw.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match kind {
                    "text" => raw
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(|t| McpContentBlock::Text(t.to_string())),
                    "image" => {
                        let data = raw.get("data").and_then(|v| v.as_str())?.to_string();
                        let mime_type = raw
                            .get("mimeType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("application/octet-stream")
                            .to_string();
                        Some(McpContentBlock::Image { data, mime_type })
                    }
                    "audio" => {
                        let data = raw.get("data").and_then(|v| v.as_str())?.to_string();
                        let mime_type = raw
                            .get("mimeType")
                            .and_then(|v| v.as_str())
                            .unwrap_or("application/octet-stream")
                            .to_string();
                        Some(McpContentBlock::Audio { data, mime_type })
                    }
                    "resource" => {
                        let resource = raw.get("resource")?;
                        let uri = resource
                            .get("uri")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let text = resource
                            .get("text")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let blob = resource
                            .get("blob")
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        let mime_type = resource
                            .get("mimeType")
                            .or_else(|| resource.get("mime_type"))
                            .and_then(|v| v.as_str())
                            .map(str::to_string);
                        Some(McpContentBlock::Resource {
                            uri,
                            text,
                            blob,
                            mime_type,
                        })
                    }
                    _ => None,
                }
            })
            .collect();
        Ok(McpToolCallResult {
            content,
            is_error: result.is_error.unwrap_or(false),
        })
    }

    async fn authenticate(&self, server_name: &str) -> Result<String, coco_error::BoxedError> {
        let manager = self.manager.lock().await.clone();
        manager
            .authenticate(server_name, self.send_elicitation_for_server(server_name))
            .await
            .map_err(|e| {
                Box::new(coco_error::PlainError::new(
                    format!("MCP authentication failed: {e}"),
                    coco_error::StatusCode::AuthenticationFailed,
                )) as coco_error::BoxedError
            })
    }

    async fn connected_servers(&self) -> Vec<String> {
        let manager = self.manager.lock().await.clone();
        manager
            .connected_servers()
            .await
            .into_iter()
            .map(|s| s.name)
            .collect()
    }

    async fn list_tools(&self) -> Vec<McpToolSchema> {
        let manager = self.manager.lock().await.clone();
        manager
            .all_tools()
            .await
            .into_iter()
            .map(|(server, def)| {
                let annotations = McpToolAnnotations::from_input_schema_meta(&def.input_schema);
                McpToolSchema {
                    server_name: server,
                    tool_name: def.name,
                    description: def.description,
                    input_schema: def.input_schema,
                    annotations,
                }
            })
            .collect()
    }

    async fn add_dynamic_server(
        &self,
        name: &str,
        config: serde_json::Value,
    ) -> Result<(), coco_error::BoxedError> {
        // Deserialise the JSON body into the typed `McpServerConfig`
        // discriminated union via serde's `#[serde(tag = "transport")]`.
        let typed: coco_mcp::McpServerConfig = serde_json::from_value(config).map_err(|e| {
            Box::new(coco_error::PlainError::new(
                format!("invalid inline MCP server config for '{name}': {e}"),
                coco_error::StatusCode::InvalidJson,
            )) as coco_error::BoxedError
        })?;
        let scoped = coco_mcp::ScopedMcpServerConfig {
            name: name.to_string(),
            config: typed,
            scope: coco_mcp::ConfigScope::Dynamic,
            plugin_source: None,
        };
        let manager = {
            let mut manager = self.manager.lock().await;
            manager.register_server(scoped);
            manager.clone()
        };
        // `connect` validates wiring + opens the transport. Errors
        // bubble up so the spawn path can fail closed (TS behaviour:
        // a failed inline server logs a warning + skips the agent's
        // tools — we surface the error so the agent's spawn can
        // decide).
        // Hooks run before client routing when hook context is installed.
        let send_elicitation = self.send_elicitation_for_server(name);
        manager.connect(name, send_elicitation).await.map_err(|e| {
            Box::new(coco_error::PlainError::new(
                format!("MCP connect '{name}' failed: {e}"),
                coco_error::StatusCode::ConnectionFailed,
            )) as coco_error::BoxedError
        })?;

        // Skill discovery: best-effort post-connect enrichment.
        // Gating (feature flag + `resources` capability) lives inside
        // `coco_mcp_skills::sync_one` so all skill-discovery callers
        // (this adapter + initial-bootstrap `sync_all`) honour the
        // same TS rules from one place.
        if let Some(bridge) = self.skill_bridge.as_ref() {
            match coco_mcp_skills::sync_one(
                name,
                &manager,
                &bridge.cache,
                &bridge.skills,
                &bridge.runtime_config.features,
            )
            .await
            {
                Ok(outcome) => tracing::debug!(
                    server = %name,
                    registered = outcome.registered,
                    dropped = outcome.dropped,
                    feature_off = outcome.feature_off,
                    resources_unsupported = outcome.resources_unsupported,
                    "MCP skill sync complete"
                ),
                Err(e) => tracing::warn!(server = %name, "MCP skill discovery failed: {e}"),
            }
        }
        Ok(())
    }

    async fn remove_dynamic_server(&self, name: &str) -> Result<(), coco_error::BoxedError> {
        let manager = self.manager.lock().await.clone();
        manager.disconnect(name).await;
        // Drop any skills that came from this server. Safe to call
        // unconditionally — `unregister_skills_for_mcp_server` is a
        // no-op when no skills are registered for `name`.
        if let Some(bridge) = self.skill_bridge.as_ref() {
            let dropped = bridge.skills.unregister_skills_for_mcp_server(name);
            if dropped > 0 {
                tracing::debug!(
                    server = %name,
                    dropped,
                    "cleared MCP-sourced skills on server disconnect"
                );
            }
        }
        Ok(())
    }
}

fn convert_read_resource_result(
    result: coco_mcp::ReadResourceResult,
) -> Result<Vec<McpResourceContent>, coco_error::BoxedError> {
    if result.contents.is_empty() {
        return Err(Box::new(coco_error::PlainError::new(
            "MCP resource read returned no content",
            coco_error::StatusCode::External,
        )));
    }
    Ok(result
        .contents
        .into_iter()
        .map(|content| match content {
            coco_mcp::ReadResourceResultContents::TextResourceContents(text) => {
                McpResourceContent {
                    uri: text.uri,
                    text: Some(text.text),
                    blob: None,
                    mime_type: text.mime_type,
                }
            }
            coco_mcp::ReadResourceResultContents::BlobResourceContents(blob) => {
                McpResourceContent {
                    uri: blob.uri,
                    text: None,
                    blob: Some(blob.blob),
                    mime_type: blob.mime_type,
                }
            }
        })
        .collect())
}

#[cfg(test)]
#[path = "mcp_handle_adapter.test.rs"]
mod tests;
