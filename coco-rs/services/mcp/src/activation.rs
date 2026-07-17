//! Activation resolution: which defined MCP servers actually run.
//!
//! Merging *definitions* (`crate::config`) is deliberately separate from
//! deciding *whether each definition runs*. Definitions may arrive with a
//! checked-out repository; every input to the run/don't-run decision is
//! either user-owned (`GlobalConfig`, outside the repo by construction) or
//! resolved through trusted-source settings accessors
//! (`coco_config::McpPolicyConfig`) — so a repository can define a server
//! but can never approve, enable, or un-deny one.

use std::collections::BTreeSet;
use std::path::Path;

use coco_config::DeniedMcpServerEntry;
use coco_config::McpPolicyConfig;
use coco_config::global_config::GlobalConfig;
use tracing::info;
use tracing::warn;

use crate::config::DefinedMcpServer;
use crate::types::ConfigScope;
use crate::types::McpServerConfig;
use crate::types::ScopedMcpServerConfig;

/// Whether one defined MCP server runs this session, and if not, why not.
///
/// The single authority both the connection path (keep `Active` only) and
/// `/mcp list` render from — the two views cannot disagree by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpActivation {
    /// Connects at session start.
    Active,
    /// The user switched it off for this project (`/mcp disable`).
    UserDisabled,
    /// Repo-defined (project-scope) server not yet approved by the user or a
    /// trusted setting. Fails closed: a cloned repository's `.mcp.json` does
    /// not spawn processes until someone who isn't the repository says so.
    AwaitingApproval,
    /// Matched a `denied_mcp_servers` settings entry.
    PolicyDenied,
    /// The definition still carries the removed `"disabled": true` field.
    /// Fail-safe off until the field is removed (`/mcp enable` migrates it).
    LegacyDisabled,
}

/// Per-project activation inputs, resolved once per session bootstrap (or
/// `/mcp` invocation) from `GlobalConfig` + the settings-derived
/// [`McpPolicyConfig`].
#[derive(Debug, Clone, Default)]
pub struct McpActivationPolicy {
    /// Servers the user switched off for this project.
    user_disabled: BTreeSet<String>,
    /// Project-scope servers the user approved for this project.
    user_approved: BTreeSet<String>,
    /// Trusted-source `enable_all_project_mcp_servers`.
    project_pre_approved: bool,
    /// Trusted-source `allowed_mcp_servers` names.
    trusted_allowed: BTreeSet<String>,
    /// Deny entries from every settings source.
    denied: Vec<DeniedMcpServerEntry>,
}

/// The `GlobalConfig.projects` key for a project root. Must stay in sync with
/// how the rest of the codebase keys that map (lossy path string).
pub fn project_key(project_root: &Path) -> String {
    project_root.to_string_lossy().to_string()
}

impl McpActivationPolicy {
    /// Resolve against the default global-config path. An unreadable global
    /// config logs and degrades to "no user toggles" — project-scope servers
    /// still fail closed through the approval gate.
    pub fn resolve(project_root: &Path, policy: &McpPolicyConfig) -> Self {
        let global = coco_config::global_config::load_global_config().unwrap_or_else(|error| {
            warn!(%error, "failed to read global config; MCP user toggles unavailable");
            GlobalConfig::default()
        });
        Self::resolve_with_global(&global, project_root, policy)
    }

    /// Resolve against an explicit global-config path (tests, `/mcp`).
    pub fn resolve_at(
        global_config_path: &Path,
        project_root: &Path,
        policy: &McpPolicyConfig,
    ) -> Self {
        let global = coco_config::global_config::load_global_config_at(global_config_path)
            .unwrap_or_else(|error| {
                warn!(%error, "failed to read global config; MCP user toggles unavailable");
                GlobalConfig::default()
            });
        Self::resolve_with_global(&global, project_root, policy)
    }

    /// Pure combination of an already-loaded `GlobalConfig` with the
    /// settings-derived policy.
    pub fn resolve_with_global(
        global: &GlobalConfig,
        project_root: &Path,
        policy: &McpPolicyConfig,
    ) -> Self {
        let project = global.projects.get(&project_key(project_root));
        Self {
            user_disabled: project
                .map(|p| p.disabled_mcp_servers.clone())
                .unwrap_or_default(),
            user_approved: project
                .map(|p| p.approved_mcp_servers.clone())
                .unwrap_or_default(),
            project_pre_approved: policy.project_servers_pre_approved,
            trusted_allowed: policy.trusted_allowed_servers.iter().cloned().collect(),
            denied: policy.denied_servers.clone(),
        }
    }

    /// Resolve one server's activation.
    ///
    /// Precedence, strongest first: policy deny > legacy fail-safe > user
    /// toggle > approval gate. `config` feeds the deny list's content
    /// matching; `None` (unparseable entry) still matches by name.
    pub fn activation(
        &self,
        name: &str,
        scope: ConfigScope,
        config: Option<&McpServerConfig>,
        legacy_disabled: bool,
    ) -> McpActivation {
        if self
            .denied
            .iter()
            .any(|entry| entry_denies(entry, name, config))
        {
            return McpActivation::PolicyDenied;
        }
        if legacy_disabled {
            return McpActivation::LegacyDisabled;
        }
        if self.user_disabled.contains(name) {
            return McpActivation::UserDisabled;
        }
        if scope == ConfigScope::Project
            && !self.project_pre_approved
            && !self.user_approved.contains(name)
            && !self.trusted_allowed.contains(name)
        {
            return McpActivation::AwaitingApproval;
        }
        McpActivation::Active
    }

    /// Activation for a merged definition (`/mcp list`'s per-row status).
    pub fn for_defined(&self, server: &DefinedMcpServer) -> McpActivation {
        self.activation(
            &server.name,
            server.scope,
            server.config.as_ref(),
            server.legacy_disabled,
        )
    }

    /// Keep only servers that activate; log the rest. The connection path's
    /// choke point — file-defined and plugin servers both pass through here.
    pub fn filter_active(&self, servers: Vec<ScopedMcpServerConfig>) -> Vec<ScopedMcpServerConfig> {
        servers
            .into_iter()
            .filter(|server| {
                let activation = self.activation(
                    &server.name,
                    server.scope,
                    Some(&server.config),
                    /*legacy_disabled*/ false,
                );
                if activation != McpActivation::Active {
                    info!(server = %server.name, scope = ?server.scope, ?activation,
                        "MCP server defined but not activated");
                }
                activation == McpActivation::Active
            })
            .collect()
    }

    /// Whether the deny list bans this server (used by `/mcp enable` to
    /// refuse an enable that could never take effect).
    pub fn is_denied(&self, name: &str, config: Option<&McpServerConfig>) -> bool {
        self.denied
            .iter()
            .any(|entry| entry_denies(entry, name, config))
    }
}

/// One deny entry vs one server: exact name, exact stdio command, or URL
/// prefix. Content matching means redefining a banned server under another
/// name does not dodge the ban.
fn entry_denies(
    entry: &DeniedMcpServerEntry,
    name: &str,
    config: Option<&McpServerConfig>,
) -> bool {
    if entry.name == name {
        return true;
    }
    let Some(config) = config else {
        return false;
    };
    match config {
        McpServerConfig::Stdio(stdio) => entry.command.as_deref() == Some(stdio.command.as_str()),
        McpServerConfig::Sse(sse) => url_denied(entry, &sse.url),
        McpServerConfig::Http(http) => url_denied(entry, &http.url),
        McpServerConfig::WebSocket(ws) => url_denied(entry, &ws.url),
        // Client-hosted / claude.ai-proxied servers have no local command or
        // URL to content-match; name matching still applies.
        McpServerConfig::ClientHosted(_) | McpServerConfig::ClaudeAiProxy(_) => false,
    }
}

fn url_denied(entry: &DeniedMcpServerEntry, url: &str) -> bool {
    entry
        .url
        .as_deref()
        .is_some_and(|prefix| url.starts_with(prefix))
}

#[cfg(test)]
#[path = "activation.test.rs"]
mod tests;
