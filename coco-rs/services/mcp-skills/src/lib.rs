//! MCP → Skills bridge.
//!
//! Discovers MCP skills for every MCP connection (initial + dynamic)
//! when `Feature::McpSkills` and the `resources` capability are both active.
//!
//! ## Layer rule
//!
//! Sits at L4 alongside [`coco_skills`] and depends on [`coco_mcp`] (L3).
//! Higher layers (`app/cli`) only call into this crate — they no longer
//! duplicate the discovery / capability-gate logic inline.
//!
//! ## Skill resource convention
//!
//! A discovered MCP resource is treated as a skill when its URI starts
//! with `skill://`.
//!
//! ## Idempotency
//!
//! [`sync_one`] first calls
//! [`coco_skills::SkillManager::unregister_skills_for_mcp_server`]
//! for the same server, so a reconnect / refresh does not
//! double-register the same skills. [`sync_all`] is the
//! every-connected-server variant.

use std::sync::Arc;

use coco_mcp::McpCapabilities;
use coco_mcp::McpClientError;
use coco_mcp::McpConnectionManager;
use coco_mcp::ReadResourceResultContents;
use coco_mcp::discovery::DiscoveredResource;
use coco_mcp::discovery::DiscoveryCache;
use coco_mcp::discovery::discover_resources;
use coco_skills::SkillManager;
use coco_skills::mcp_builders::McpSkillSpec;
use coco_types::Feature;
use coco_types::Features;
use thiserror::Error;
use tokio::sync::RwLock;
use tracing::debug;
use tracing::warn;

/// Errors during MCP skill discovery / registration.
#[derive(Debug, Error)]
pub enum McpSkillsError {
    #[error("MCP discovery failed: {source}")]
    Discovery {
        #[from]
        source: McpClientError,
    },
}

// Layer `coco-error` classification traits on top so callers can match on
// `StatusCode` without a full snafu migration (same shape as
// `McpClientError`).
impl coco_error::StackError for McpSkillsError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        match self {
            Self::Discovery { source } => Some(source),
        }
    }
}

impl coco_error::ErrorExt for McpSkillsError {
    fn status_code(&self) -> coco_error::StatusCode {
        match self {
            Self::Discovery { source } => source.status_code(),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Outcome of a single-server sync.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncOutcome {
    /// Number of stale skills cleared before re-registration.
    pub dropped: usize,
    /// Number of skills successfully registered this pass.
    pub registered: usize,
    /// Skipped because the feature flag was off.
    pub feature_off: bool,
    /// Skipped because the server doesn't advertise the `resources` capability.
    pub resources_unsupported: bool,
}

/// Discover MCP skills from a single connected server and reconcile
/// them with the [`SkillManager`].
///
/// Steps:
/// 1. Bail when `Feature::McpSkills` is off (sets
///    [`SyncOutcome::feature_off`]).
/// 2. Look up the server's `capabilities`. Bail when `resources` is
///    unsupported (sets [`SyncOutcome::resources_unsupported`]).
/// 3. Clear any stale `Mcp { server_name == name }` skills.
/// 4. List resources; filter by `skill://` URI; read each; register
///    via [`SkillManager::register_mcp_skill`].
///
/// Builder errors for individual resources are logged at `warn` and
/// skipped — a single bad skill never blocks the rest.
pub async fn sync_one(
    server_name: &str,
    mcp: &McpConnectionManager,
    cache: &Arc<RwLock<DiscoveryCache>>,
    skills: &SkillManager,
    features: &Features,
) -> Result<SyncOutcome, McpSkillsError> {
    if !features.enabled(Feature::McpSkills) {
        return Ok(SyncOutcome {
            feature_off: true,
            ..SyncOutcome::default()
        });
    }

    let caps = server_capabilities(mcp, server_name).await;
    if caps.map(|c| !c.resources).unwrap_or(true) {
        return Ok(SyncOutcome {
            resources_unsupported: true,
            ..SyncOutcome::default()
        });
    }

    let resources = discover_resources(mcp, server_name, cache).await?;

    // Drop any stale skills from a previous connection to this server.
    let dropped = skills.unregister_skills_for_mcp_server(server_name);

    let skill_resources: Vec<DiscoveredResource> =
        resources.into_iter().filter(is_skill_resource).collect();

    if skill_resources.is_empty() {
        debug!(
            server = %server_name,
            dropped,
            "no skill:// resources found on server"
        );
        return Ok(SyncOutcome {
            dropped,
            ..SyncOutcome::default()
        });
    }

    let mut registered = 0usize;
    for resource in skill_resources {
        let content = match mcp.read_resource(server_name, &resource.uri).await {
            Ok(result) => extract_text_content(&result),
            Err(e) => {
                warn!(
                    server = %server_name,
                    uri = %resource.uri,
                    "skill read failed: {e}"
                );
                continue;
            }
        };
        let Some(content) = content else {
            warn!(
                server = %server_name,
                uri = %resource.uri,
                "skill resource had no text content; skipping"
            );
            continue;
        };
        let spec = McpSkillSpec {
            server_name: server_name.to_string(),
            uri: resource.uri.clone(),
            name: derive_skill_name(&resource.uri, &resource.name),
            description: resource.description.clone(),
            content,
        };
        match skills.register_mcp_skill(spec) {
            Ok(()) => registered += 1,
            Err(e) => warn!(
                server = %server_name,
                uri = %resource.uri,
                "skill build failed: {e}"
            ),
        }
    }

    debug!(
        server = %server_name,
        dropped,
        registered,
        "MCP skills reconciled"
    );
    Ok(SyncOutcome {
        dropped,
        registered,
        ..SyncOutcome::default()
    })
}

/// Discover MCP skills for **every connected server** the manager
/// currently knows about. Used at session bootstrap so static
/// (config-loaded) servers register their skills the same way dynamic
/// (`mcp/setServers`) servers do.
///
/// Per-server errors are logged at `warn` and counted; the overall
/// result aggregates outcomes so the caller can surface a single
/// summary line.
pub async fn sync_all(
    mcp: &McpConnectionManager,
    cache: &Arc<RwLock<DiscoveryCache>>,
    skills: &SkillManager,
    features: &Features,
) -> SyncSummary {
    if !features.enabled(Feature::McpSkills) {
        return SyncSummary {
            feature_off: true,
            ..SyncSummary::default()
        };
    }

    let mut summary = SyncSummary::default();
    for server in mcp.connected_servers().await {
        match sync_one(&server.name, mcp, cache, skills, features).await {
            Ok(out) => {
                summary.servers += 1;
                summary.total_registered += out.registered;
                summary.total_dropped += out.dropped;
                if out.resources_unsupported {
                    summary.servers_resources_unsupported += 1;
                }
            }
            Err(e) => {
                summary.errors += 1;
                warn!(server = %server.name, "MCP skill sync failed: {e}");
            }
        }
    }
    summary
}

/// Aggregate result from [`sync_all`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncSummary {
    /// Number of servers processed.
    pub servers: usize,
    /// Servers that don't advertise resources (skipped).
    pub servers_resources_unsupported: usize,
    /// Total skills cleared across all servers.
    pub total_dropped: usize,
    /// Total skills registered across all servers.
    pub total_registered: usize,
    /// Per-server errors (logged at `warn` site).
    pub errors: usize,
    /// Whole pass skipped because the feature was off.
    pub feature_off: bool,
}

// ─── helpers ────────────────────────────────────────────────────────────

async fn server_capabilities(
    mcp: &McpConnectionManager,
    server_name: &str,
) -> Option<McpCapabilities> {
    mcp.connected_servers()
        .await
        .into_iter()
        .find(|s| s.name == server_name)
        .map(|s| s.capabilities)
}

/// True when a resource is intended for skill consumption.
///
/// Current heuristic: URI starts with `skill://`.
fn is_skill_resource(r: &DiscoveredResource) -> bool {
    r.uri.starts_with("skill://")
}

/// Derive the canonical skill name from a `skill://server/name` URI.
///
/// Falls back to the resource's `name` field when the URI yields nothing
/// useful.
fn derive_skill_name(uri: &str, fallback: &str) -> String {
    if let Some(rest) = uri.strip_prefix("skill://")
        && let Some(last) = rest.split('/').rfind(|s| !s.is_empty())
    {
        return last.to_string();
    }
    fallback.to_string()
}

/// Pull the first text payload out of an MCP `read_resource` result.
///
/// Skills are markdown text — `Blob`s are skipped. The match is on
/// [`ReadResourceResultContents`] directly so a future rename / variant
/// addition surfaces as a compile error rather than a silent `None`
/// return.
fn extract_text_content(result: &coco_mcp::ReadResourceResult) -> Option<String> {
    result.contents.iter().find_map(|content| match content {
        ReadResourceResultContents::TextResourceContents(text) => Some(text.text.clone()),
        ReadResourceResultContents::BlobResourceContents(_) => None,
    })
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
