use std::collections::HashMap;
use std::path::Path;

use coco_config::DeniedMcpServerEntry;
use coco_config::McpPolicyConfig;
use coco_config::global_config::GlobalConfig;
use coco_config::global_config::ProjectConfig;
use pretty_assertions::assert_eq;

use super::*;
use crate::types::McpStdioConfig;

fn stdio(command: &str) -> McpServerConfig {
    McpServerConfig::Stdio(McpStdioConfig {
        command: command.to_string(),
        args: vec![],
        env: HashMap::new(),
        cwd: None,
    })
}

fn sse(url: &str) -> McpServerConfig {
    McpServerConfig::Sse(crate::types::McpSseConfig {
        url: url.to_string(),
        headers: HashMap::new(),
        headers_helper: None,
        oauth: None,
    })
}

fn deny(name: &str) -> DeniedMcpServerEntry {
    DeniedMcpServerEntry {
        name: name.to_string(),
        command: None,
        url: None,
    }
}

fn global_with(project_root: &Path, project: ProjectConfig) -> GlobalConfig {
    let mut global = GlobalConfig::default();
    global.projects.insert(project_key(project_root), project);
    global
}

fn root() -> &'static Path {
    Path::new("/repo")
}

#[test]
fn test_user_disabled_wins_regardless_of_defining_scope() {
    let global = global_with(
        root(),
        ProjectConfig {
            disabled_mcp_servers: ["srv".to_string()].into(),
            ..Default::default()
        },
    );
    let policy =
        McpActivationPolicy::resolve_with_global(&global, root(), &McpPolicyConfig::default());

    // The toggle is keyed by name: no lower-precedence definition of the same
    // name can survive it, which is what made the old file-edit disable leak.
    for scope in [
        ConfigScope::User,
        ConfigScope::Local,
        ConfigScope::Enterprise,
    ] {
        assert_eq!(
            policy.activation("srv", scope, Some(&stdio("cmd")), false),
            McpActivation::UserDisabled
        );
    }
}

#[test]
fn test_project_scope_awaits_approval_by_default() {
    let policy = McpActivationPolicy::resolve_with_global(
        &GlobalConfig::default(),
        root(),
        &McpPolicyConfig::default(),
    );

    assert_eq!(
        policy.activation("repo-srv", ConfigScope::Project, Some(&stdio("cmd")), false),
        McpActivation::AwaitingApproval,
        "a cloned repo's .mcp.json must not auto-connect"
    );
    // Every non-project scope is user- or admin-owned and needs no gate.
    for scope in [
        ConfigScope::User,
        ConfigScope::Local,
        ConfigScope::Enterprise,
        ConfigScope::Managed,
        ConfigScope::Dynamic,
        ConfigScope::ClaudeAi,
    ] {
        assert_eq!(
            policy.activation("srv", scope, Some(&stdio("cmd")), false),
            McpActivation::Active
        );
    }
}

#[test]
fn test_user_approval_unlocks_project_server() {
    let global = global_with(
        root(),
        ProjectConfig {
            approved_mcp_servers: ["repo-srv".to_string()].into(),
            ..Default::default()
        },
    );
    let policy =
        McpActivationPolicy::resolve_with_global(&global, root(), &McpPolicyConfig::default());

    assert_eq!(
        policy.activation("repo-srv", ConfigScope::Project, Some(&stdio("cmd")), false),
        McpActivation::Active
    );
    // Approval is per project root: another project sees the gate again.
    let other = McpActivationPolicy::resolve_with_global(
        &global,
        Path::new("/other"),
        &McpPolicyConfig::default(),
    );
    assert_eq!(
        other.activation("repo-srv", ConfigScope::Project, Some(&stdio("cmd")), false),
        McpActivation::AwaitingApproval
    );
}

#[test]
fn test_trusted_settings_pre_approve_project_servers() {
    let all = McpPolicyConfig {
        project_servers_pre_approved: true,
        ..Default::default()
    };
    let policy = McpActivationPolicy::resolve_with_global(&GlobalConfig::default(), root(), &all);
    assert_eq!(
        policy.activation("repo-srv", ConfigScope::Project, Some(&stdio("cmd")), false),
        McpActivation::Active
    );

    let named = McpPolicyConfig {
        trusted_allowed_servers: vec!["repo-srv".to_string()],
        ..Default::default()
    };
    let policy = McpActivationPolicy::resolve_with_global(&GlobalConfig::default(), root(), &named);
    assert_eq!(
        policy.activation("repo-srv", ConfigScope::Project, Some(&stdio("cmd")), false),
        McpActivation::Active
    );
    assert_eq!(
        policy.activation(
            "other-srv",
            ConfigScope::Project,
            Some(&stdio("cmd")),
            false
        ),
        McpActivation::AwaitingApproval
    );
}

#[test]
fn test_policy_deny_beats_approval_and_user_toggle() {
    let global = global_with(
        root(),
        ProjectConfig {
            approved_mcp_servers: ["banned".to_string()].into(),
            ..Default::default()
        },
    );
    let cfg = McpPolicyConfig {
        project_servers_pre_approved: true,
        denied_servers: vec![deny("banned")],
        ..Default::default()
    };
    let policy = McpActivationPolicy::resolve_with_global(&global, root(), &cfg);

    // Deny wins in every scope — this is what "policy can ban a server" means.
    for scope in [
        ConfigScope::User,
        ConfigScope::Project,
        ConfigScope::Managed,
    ] {
        assert_eq!(
            policy.activation("banned", scope, Some(&stdio("cmd")), false),
            McpActivation::PolicyDenied
        );
    }
    assert!(
        policy.is_denied("banned", None),
        "name match needs no config"
    );
}

#[test]
fn test_deny_content_match_catches_renamed_server() {
    let cfg = McpPolicyConfig {
        denied_servers: vec![
            DeniedMcpServerEntry {
                name: "evil".to_string(),
                command: Some("evil-bin".to_string()),
                url: None,
            },
            DeniedMcpServerEntry {
                name: "evil-remote".to_string(),
                command: None,
                url: Some("https://evil.example.com/".to_string()),
            },
        ],
        ..Default::default()
    };
    let policy = McpActivationPolicy::resolve_with_global(&GlobalConfig::default(), root(), &cfg);

    // Renaming does not dodge a content ban.
    assert_eq!(
        policy.activation(
            "innocent",
            ConfigScope::User,
            Some(&stdio("evil-bin")),
            false
        ),
        McpActivation::PolicyDenied
    );
    assert_eq!(
        policy.activation(
            "innocent",
            ConfigScope::User,
            Some(&sse("https://evil.example.com/mcp")),
            false
        ),
        McpActivation::PolicyDenied
    );
    // Different content stays unaffected.
    assert_eq!(
        policy.activation(
            "innocent",
            ConfigScope::User,
            Some(&stdio("fine-bin")),
            false
        ),
        McpActivation::Active
    );
}

#[test]
fn test_legacy_disabled_fails_safe_even_when_otherwise_active() {
    let policy = McpActivationPolicy::resolve_with_global(
        &GlobalConfig::default(),
        root(),
        &McpPolicyConfig::default(),
    );
    assert_eq!(
        policy.activation("srv", ConfigScope::User, Some(&stdio("cmd")), true),
        McpActivation::LegacyDisabled,
        "a residual \"disabled\": true must keep the server off, never re-enable it"
    );
}

#[test]
fn test_filter_active_keeps_only_active_servers() {
    let global = global_with(
        root(),
        ProjectConfig {
            disabled_mcp_servers: ["off".to_string()].into(),
            ..Default::default()
        },
    );
    let policy =
        McpActivationPolicy::resolve_with_global(&global, root(), &McpPolicyConfig::default());

    let servers = vec![
        ScopedMcpServerConfig {
            name: "on".to_string(),
            config: stdio("cmd"),
            scope: ConfigScope::User,
            plugin_source: None,
        },
        ScopedMcpServerConfig {
            name: "off".to_string(),
            config: stdio("cmd"),
            scope: ConfigScope::User,
            plugin_source: None,
        },
        // The plugin bridge path: user toggles apply to Dynamic servers too.
        ScopedMcpServerConfig {
            name: "unapproved".to_string(),
            config: stdio("cmd"),
            scope: ConfigScope::Project,
            plugin_source: None,
        },
    ];

    let active: Vec<String> = policy
        .filter_active(servers)
        .into_iter()
        .map(|server| server.name)
        .collect();
    assert_eq!(active, vec!["on".to_string()]);
}
