use std::path::PathBuf;

use coco_types::SandboxMode;
use pretty_assertions::assert_eq;

use super::*;
use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;

// ==========================================================================
// SandboxBypass tests
// ==========================================================================

#[test]
fn test_sandbox_bypass_from_flag() {
    assert_eq!(SandboxBypass::from_flag(false), SandboxBypass::No);
    assert_eq!(SandboxBypass::from_flag(true), SandboxBypass::Requested);
}

// ==========================================================================
// SandboxSettings: defaults + constructors
// ==========================================================================

#[test]
fn test_sandbox_settings_default_disabled() {
    let settings = SandboxSettings::default();
    // High-level posture defaults
    assert_eq!(settings.mode, SandboxMode::ReadOnly);
    assert!(!settings.allow_network);
    // defaults
    assert!(!settings.enabled);
    assert!(settings.auto_allow_bash_if_sandboxed);
    assert!(settings.allow_unsandboxed_commands);
    assert_eq!(
        settings.enabled_platforms,
        vec!["macos", "linux", "windows"]
    );
    assert!(settings.excluded_commands.is_empty());
    assert!(settings.network.allowed_domains.is_empty());
    assert!(settings.network.denied_domains.is_empty());
    assert!(settings.filesystem.allow_write.is_empty());
    assert!(settings.filesystem.deny_write.is_empty());
    assert!(settings.filesystem.deny_read.is_empty());
    assert!(settings.filesystem.allow_read.is_empty());
    assert!(!settings.filesystem.allow_git_config);
    assert!(settings.ignore_violations.is_empty());
    assert!(!settings.enable_weaker_nested_sandbox);
    assert!(!settings.enable_weaker_network_isolation);
    // PTY is allowed by default (codex-rs seatbelt base policy parity); only an
    // explicit `"allow_pty": false` disables it (covered below).
    assert!(settings.allow_pty);
    assert_eq!(settings.mandatory_deny_search_depth, 3);
    assert!(settings.ripgrep.is_none());
}

#[test]
fn test_sandbox_settings_enabled_constructor() {
    let settings = SandboxSettings::enabled();
    assert!(settings.enabled);
    assert!(settings.auto_allow_bash_if_sandboxed);
    assert!(settings.allow_unsandboxed_commands);
}

#[test]
fn test_sandbox_settings_disabled_constructor() {
    let settings = SandboxSettings::disabled();
    assert!(!settings.enabled);
}

// ==========================================================================
// is_sandboxed gating
// ==========================================================================

#[test]
fn test_is_sandboxed_disabled_by_default() {
    let settings = SandboxSettings::default();
    assert!(!settings.is_sandboxed("echo hello", SandboxBypass::No));
    assert!(!settings.is_sandboxed("rm -rf /", SandboxBypass::No));
    assert!(!settings.is_sandboxed("echo hello", SandboxBypass::Requested));
}

#[test]
fn test_is_sandboxed_enabled() {
    let settings = SandboxSettings::enabled();
    assert!(settings.is_sandboxed("echo hello", SandboxBypass::No));
    assert!(settings.is_sandboxed("rm -rf /", SandboxBypass::No));
}

#[test]
fn test_is_sandboxed_bypass_allowed() {
    let settings = SandboxSettings::enabled();
    assert!(!settings.is_sandboxed("echo hello", SandboxBypass::Requested));
}

#[test]
fn test_is_sandboxed_bypass_disallowed() {
    let mut settings = SandboxSettings::enabled();
    settings.allow_unsandboxed_commands = false;
    assert!(settings.is_sandboxed("echo hello", SandboxBypass::Requested));
}

#[test]
fn test_is_sandboxed_empty_command() {
    let settings = SandboxSettings::enabled();
    assert!(!settings.is_sandboxed("", SandboxBypass::No));
    assert!(!settings.is_sandboxed("   ", SandboxBypass::No));
    assert!(!settings.is_sandboxed("\t\n", SandboxBypass::No));
}

#[test]
fn test_is_sandboxed_excluded_exact() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["docker".to_string()];
    assert!(!settings.is_sandboxed("docker", SandboxBypass::No));
    assert!(!settings.is_sandboxed("docker ps", SandboxBypass::No));
    // "dockerize" does NOT match (no space separator)
    assert!(settings.is_sandboxed("dockerize", SandboxBypass::No));
}

#[test]
fn test_is_sandboxed_excluded_wildcard() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["git*".to_string()];
    assert!(!settings.is_sandboxed("git status", SandboxBypass::No));
    assert!(!settings.is_sandboxed("git", SandboxBypass::No));
    assert!(!settings.is_sandboxed("gitk", SandboxBypass::No));
}

#[test]
fn test_is_sandboxed_excluded_phrase() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["git push".to_string()];
    assert!(!settings.is_sandboxed("git push origin main", SandboxBypass::No));
    assert!(settings.is_sandboxed("git status", SandboxBypass::No));
}

// ==========================================================================
// BFS Command Exclusion (env strip + basename + wrapper peel)
// ==========================================================================

#[test]
fn test_excluded_env_stripped() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["npm".to_string()];
    assert!(!settings.is_sandboxed("FOO=bar npm install", SandboxBypass::No));
    assert!(!settings.is_sandboxed("A=1 B=2 npm run build", SandboxBypass::No));
    assert!(settings.is_sandboxed("FOO=bar yarn install", SandboxBypass::No));
}

#[test]
fn test_excluded_basename_extraction() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["npm".to_string()];
    assert!(!settings.is_sandboxed("/usr/bin/npm install", SandboxBypass::No));
    assert!(!settings.is_sandboxed("./node_modules/.bin/npm run test", SandboxBypass::No));
}

#[test]
fn test_excluded_env_and_basename_combined() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["npm".to_string()];
    assert!(!settings.is_sandboxed("FOO=bar /usr/bin/npm install", SandboxBypass::No));
}

#[test]
fn test_excluded_colon_prefix_pattern() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["npm:*".to_string()];
    assert!(!settings.is_sandboxed("npm", SandboxBypass::No));
    assert!(!settings.is_sandboxed("npm install", SandboxBypass::No));
    assert!(!settings.is_sandboxed("npm run build", SandboxBypass::No));
    // Colon-prefix requires exact first word.
    assert!(settings.is_sandboxed("npmx", SandboxBypass::No));
}

#[test]
fn test_excluded_empty_list() {
    let settings = SandboxSettings::enabled();
    assert!(settings.is_sandboxed("npm install", SandboxBypass::No));
}

#[test]
fn test_excluded_does_not_match_unrelated_env_command() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["yarn".to_string()];
    assert!(settings.is_sandboxed("FOO=bar npm install", SandboxBypass::No));
    assert!(!settings.is_sandboxed("FOO=bar yarn install", SandboxBypass::No));
}

#[test]
fn test_excluded_strips_timeout_wrapper() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["bazel".to_string()];
    assert!(!settings.is_sandboxed("timeout 30 bazel build", SandboxBypass::No));
    assert!(!settings.is_sandboxed("timeout 5s bazel test", SandboxBypass::No));
    assert!(!settings.is_sandboxed("timeout 1.5h bazel run", SandboxBypass::No));
    assert!(!settings.is_sandboxed(
        "timeout --foreground --kill-after=5 30 bazel test",
        SandboxBypass::No
    ));
    // Defensive: non-numeric next token disables the strip.
    assert!(settings.is_sandboxed("timeout bazel build", SandboxBypass::No));
}

#[test]
fn test_excluded_strips_other_safe_wrappers() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["git".to_string()];
    assert!(!settings.is_sandboxed("nice git status", SandboxBypass::No));
    assert!(!settings.is_sandboxed("nice -n 10 git status", SandboxBypass::No));
    assert!(!settings.is_sandboxed("nice -10 git status", SandboxBypass::No));
    assert!(!settings.is_sandboxed("time git status", SandboxBypass::No));
    assert!(!settings.is_sandboxed("nohup git pull", SandboxBypass::No));
    assert!(!settings.is_sandboxed("nohup -- git pull", SandboxBypass::No));
}

#[test]
fn test_excluded_chained_wrapper_env_basename() {
    let mut settings = SandboxSettings::enabled();
    settings.excluded_commands = vec!["bazel".to_string()];
    assert!(!settings.is_sandboxed(
        "timeout 300 FOO=bar /usr/bin/bazel run //:foo",
        SandboxBypass::No
    ));
}

#[test]
fn test_is_platform_enabled() {
    let settings = SandboxSettings::default();
    if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
        assert!(settings.is_platform_enabled());
    }

    let empty = SandboxSettings {
        enabled_platforms: vec![],
        ..Default::default()
    };
    assert!(!empty.is_platform_enabled());
}

// ==========================================================================
// Serde round-trip + nested deserialization
// ==========================================================================

#[test]
fn test_sandbox_settings_serde_roundtrip() {
    let settings = SandboxSettings {
        enabled: true,
        auto_allow_bash_if_sandboxed: false,
        allow_unsandboxed_commands: false,
        enabled_platforms: vec!["linux".to_string()],
        excluded_commands: vec!["docker".to_string()],
        network: NetworkConfig {
            allowed_domains: vec!["github.com".to_string()],
            denied_domains: vec!["evil.com".to_string()],
            ..Default::default()
        },
        filesystem: FilesystemConfig {
            deny_read: vec![PathBuf::from("/etc/shadow")],
            allow_read: vec![PathBuf::from("/etc/shadow/public")],
            allow_git_config: true,
            ..Default::default()
        },
        ..Default::default()
    };

    let json = serde_json::to_string(&settings).expect("serialize");
    let parsed: SandboxSettings = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, settings);
}

#[test]
fn test_sandbox_settings_from_empty_json() {
    let settings: SandboxSettings = serde_json::from_str("{}").expect("parse");
    assert!(!settings.enabled);
    assert_eq!(settings.mode, SandboxMode::ReadOnly);
    assert!(settings.auto_allow_bash_if_sandboxed);
    assert!(settings.allow_unsandboxed_commands);
    assert_eq!(
        settings.enabled_platforms,
        vec!["macos", "linux", "windows"]
    );
    assert!(settings.network.allowed_domains.is_empty());
    assert!(settings.filesystem.deny_read.is_empty());
    assert!(settings.filesystem.allow_read.is_empty());
    assert_eq!(settings.mandatory_deny_search_depth, 3);
}

#[test]
fn test_sandbox_settings_partial_json() {
    let settings: SandboxSettings = serde_json::from_str(r#"{"enabled":true}"#).expect("parse");
    assert!(settings.enabled);
    assert!(settings.auto_allow_bash_if_sandboxed);
    assert!(settings.allow_unsandboxed_commands);
}

#[test]
fn test_sandbox_settings_nested_json() {
    // settings.json plumbing — including the new `allow_read` field — flows
    // end-to-end into the rich settings type. Closes the silent-drop bug
    // where `sandbox.filesystem.*` was dropped pre-refactor.
    let json = r#"{
        "enabled": true,
        "filesystem": {
            "allow_write": ["/home/user/project"],
            "deny_read": ["/etc/shadow"],
            "allow_read": ["/etc/shadow/public"],
            "allow_git_config": true
        },
        "network": {
            "allowed_domains": ["github.com"],
            "allow_local_binding": true,
            "http_proxy_port": 3128
        },
        "allow_pty": true,
        "mandatory_deny_search_depth": 5
    }"#;

    let settings: SandboxSettings = serde_json::from_str(json).expect("parse");
    assert!(settings.enabled);
    assert_eq!(
        settings.filesystem.allow_write,
        vec![PathBuf::from("/home/user/project")]
    );
    assert_eq!(
        settings.filesystem.deny_read,
        vec![PathBuf::from("/etc/shadow")]
    );
    assert_eq!(
        settings.filesystem.allow_read,
        vec![PathBuf::from("/etc/shadow/public")]
    );
    assert!(settings.filesystem.allow_git_config);
    assert_eq!(
        settings.network.allowed_domains,
        vec!["github.com".to_string()]
    );
    assert!(settings.network.allow_local_binding);
    assert_eq!(settings.network.http_proxy_port, Some(3128));
    assert!(settings.allow_pty);
    assert_eq!(settings.mandatory_deny_search_depth, 5);
}

#[test]
fn test_allow_pty_opt_out_is_honored() {
    // The default is true (PTY allowed); an explicit false must disable it.
    let settings: SandboxSettings =
        serde_json::from_str(r#"{ "allow_pty": false }"#).expect("parse");
    assert!(!settings.allow_pty);
}

#[test]
fn test_sandbox_settings_ignore_violations() {
    let json = r#"{
        "ignore_violations": {
            "*": ["mach-lookup"],
            "npm install": ["file-write-data", "network-outbound"]
        }
    }"#;

    let settings: SandboxSettings = serde_json::from_str(json).expect("parse");
    assert_eq!(settings.ignore_violations.len(), 2);
    assert_eq!(
        settings.ignore_violations.get("*").unwrap(),
        &vec!["mach-lookup".to_string()]
    );
    assert_eq!(
        settings.ignore_violations.get("npm install").unwrap().len(),
        2
    );
}

// ==========================================================================
// FilesystemConfig
// ==========================================================================

#[test]
fn test_filesystem_config_default() {
    let config = FilesystemConfig::default();
    assert!(config.allow_write.is_empty());
    assert!(config.deny_write.is_empty());
    assert!(config.deny_read.is_empty());
    assert!(config.allow_read.is_empty());
    assert!(!config.allow_git_config);
}

#[test]
fn test_filesystem_config_serde_roundtrip() {
    let config = FilesystemConfig {
        allow_write: vec![PathBuf::from("/home/user/project")],
        deny_write: vec![PathBuf::from("/etc")],
        deny_read: vec![PathBuf::from("/etc/shadow")],
        allow_read: vec![PathBuf::from("/etc/shadow/public")],
        allow_git_config: true,
        allow_managed_read_paths_only: false,
    };
    let json = serde_json::to_string(&config).expect("serialize");
    let parsed: FilesystemConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, config);
}

#[test]
fn test_filesystem_config_from_empty_json() {
    let config: FilesystemConfig = serde_json::from_str("{}").expect("parse");
    assert!(config.allow_write.is_empty());
    assert!(config.allow_read.is_empty());
    assert!(!config.allow_git_config);
}

// ==========================================================================
// NetworkConfig + NetworkMode
// ==========================================================================

#[test]
fn test_network_config_default() {
    let config = NetworkConfig::default();
    assert_eq!(config.mode, NetworkMode::Full);
    assert!(config.allowed_domains.is_empty());
    assert!(config.denied_domains.is_empty());
    assert!(config.allow_unix_sockets.is_empty());
    assert!(!config.allow_all_unix_sockets);
    assert!(!config.allow_local_binding);
    assert!(config.http_proxy_port.is_none());
    assert!(config.socks_proxy_port.is_none());
    assert!(config.mitm_proxy.is_none());
}

#[test]
fn test_network_mode_allows_method() {
    assert!(NetworkMode::Full.allows_method("GET"));
    assert!(NetworkMode::Full.allows_method("POST"));
    assert!(NetworkMode::Full.allows_method("CONNECT"));

    assert!(NetworkMode::Limited.allows_method("GET"));
    assert!(NetworkMode::Limited.allows_method("HEAD"));
    assert!(NetworkMode::Limited.allows_method("OPTIONS"));
    assert!(!NetworkMode::Limited.allows_method("POST"));
    assert!(!NetworkMode::Limited.allows_method("PUT"));
    assert!(!NetworkMode::Limited.allows_method("DELETE"));
    assert!(!NetworkMode::Limited.allows_method("PATCH"));
    assert!(!NetworkMode::Limited.allows_method("CONNECT"));
}

#[test]
fn test_network_mode_serde() {
    let json_full = r#""full""#;
    let json_limited = r#""limited""#;
    assert_eq!(
        serde_json::from_str::<NetworkMode>(json_full).expect("parse full"),
        NetworkMode::Full
    );
    assert_eq!(
        serde_json::from_str::<NetworkMode>(json_limited).expect("parse limited"),
        NetworkMode::Limited
    );
    assert_eq!(
        serde_json::to_string(&NetworkMode::Full).expect("ser"),
        json_full
    );
    assert_eq!(
        serde_json::to_string(&NetworkMode::Limited).expect("ser"),
        json_limited
    );
}

#[test]
fn test_network_config_serde_roundtrip() {
    let config = NetworkConfig {
        mode: NetworkMode::Limited,
        allowed_domains: vec!["github.com".to_string()],
        denied_domains: vec!["evil.com".to_string()],
        allow_unix_sockets: vec![PathBuf::from("/var/run/docker.sock")],
        allow_all_unix_sockets: false,
        allow_local_binding: true,
        http_proxy_port: Some(3128),
        socks_proxy_port: Some(1080),
        mitm_proxy: Some(MitmProxyConfig {
            socket_path: PathBuf::from("/tmp/mitm.sock"),
            domains: vec!["api.example.com".to_string()],
        }),
        block_non_public_ips: true,
        allow_managed_domains_only: false,
    };
    let json = serde_json::to_string(&config).expect("serialize");
    let parsed: NetworkConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed, config);
}

#[test]
fn test_network_config_from_empty_json() {
    let config: NetworkConfig = serde_json::from_str("{}").expect("parse");
    assert!(config.allowed_domains.is_empty());
    assert!(config.http_proxy_port.is_none());
}

// ==========================================================================
// Settings → SandboxSettings end-to-end (closes the silent-drop bug)
// ==========================================================================

#[test]
fn test_settings_sandbox_filesystem_roundtrips_through_settings() {
    // settings.json that exercises the filesystem section — pre-refactor
    // this got dropped because PartialSandboxSettings didn't carry it.
    let json = r#"{
        "sandbox": {
            "enabled": true,
            "filesystem": {
                "deny_read": ["/etc/shadow"],
                "allow_read": ["/etc/shadow/readable"]
            }
        }
    }"#;
    let settings: Settings = serde_json::from_str(json).expect("parse");
    assert!(settings.sandbox.enabled);
    assert_eq!(
        settings.sandbox.filesystem.deny_read,
        vec![PathBuf::from("/etc/shadow")]
    );
    assert_eq!(
        settings.sandbox.filesystem.allow_read,
        vec![PathBuf::from("/etc/shadow/readable")]
    );
}

// ==========================================================================
// resolve(): env-overrides on top of settings deserialization
// ==========================================================================

#[test]
fn test_resolve_passes_through_settings_when_env_empty() {
    let settings = Settings {
        sandbox: SandboxSettings {
            enabled: true,
            mode: SandboxMode::WorkspaceWrite,
            allow_network: true,
            excluded_commands: vec!["docker".to_string()],
            ..Default::default()
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs(Vec::<(EnvKey, &str)>::new());
    let resolved = SandboxSettings::resolve(&settings, &env);
    assert!(resolved.enabled);
    assert_eq!(resolved.mode, SandboxMode::WorkspaceWrite);
    assert!(resolved.allow_network);
    assert_eq!(resolved.excluded_commands, vec!["docker".to_string()]);
}

#[test]
fn test_resolve_env_overrides_mode_and_network() {
    let settings = Settings {
        sandbox: SandboxSettings {
            mode: SandboxMode::ReadOnly,
            ..Default::default()
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoSandboxMode, "workspace-write"),
        (EnvKey::CocoSandboxAllowNetwork, "1"),
        (EnvKey::CocoSandboxFailIfUnavailable, "true"),
    ]);
    let resolved = SandboxSettings::resolve(&settings, &env);
    assert_eq!(resolved.mode, SandboxMode::WorkspaceWrite);
    assert!(resolved.allow_network);
    assert!(resolved.fail_if_unavailable);
}

#[test]
fn test_resolve_env_overrides_excluded_commands() {
    let settings = Settings::default();
    let env = EnvSnapshot::from_pairs([(
        EnvKey::CocoSandboxExcludedCommands,
        "git:cargo,/usr/bin/npm",
    )]);
    let resolved = SandboxSettings::resolve(&settings, &env);
    assert_eq!(
        resolved.excluded_commands,
        vec![
            "git".to_string(),
            "cargo".to_string(),
            "/usr/bin/npm".to_string(),
        ]
    );
}

#[test]
fn test_resolve_env_unrecognised_mode_falls_back_read_only() {
    let settings = Settings {
        sandbox: SandboxSettings {
            mode: SandboxMode::WorkspaceWrite,
            ..Default::default()
        },
        ..Default::default()
    };
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoSandboxMode, "garbage")]);
    let resolved = SandboxSettings::resolve(&settings, &env);
    assert_eq!(resolved.mode, SandboxMode::ReadOnly);
}

// ============================================================================
// SettingsWithSource — per-source rule extraction (drives policy gates)
// ============================================================================

#[test]
fn test_sourced_permission_rules_pulls_per_source_arrays() {
    use crate::settings::SettingsWithSource;
    use crate::settings::source::SettingSource;

    let user_raw = serde_json::json!({
        "permissions": {
            "allow": ["WebFetch(domain:user.com)"],
            "deny":  ["Read(/etc/user-secret)"],
            "ask":   ["Bash(rm:*)"]
        }
    });
    let policy_raw = serde_json::json!({
        "permissions": {
            "allow": ["WebFetch(domain:enterprise.com)"],
            "deny":  []
        }
    });
    let mut per_source = std::collections::HashMap::new();
    per_source.insert(SettingSource::User, user_raw);
    per_source.insert(SettingSource::Policy, policy_raw);
    let swith = SettingsWithSource {
        merged: Settings::default(),
        per_source,
        source_paths: std::collections::HashMap::new(),
    };
    let (allow, deny, ask) = swith.sourced_permission_rules();

    let allow_pairs: Vec<(&str, SettingSource)> =
        allow.iter().map(|r| (r.rule.as_str(), r.source)).collect();
    assert!(
        allow_pairs.contains(&("WebFetch(domain:user.com)", SettingSource::User)),
        "user allow rule tagged with User source"
    );
    assert!(
        allow_pairs.contains(&("WebFetch(domain:enterprise.com)", SettingSource::Policy)),
        "policy allow rule tagged with Policy source"
    );

    assert!(
        deny.iter()
            .any(|r| r.rule == "Read(/etc/user-secret)" && r.source == SettingSource::User),
        "user deny rule tagged with User source"
    );

    assert!(
        ask.iter()
            .any(|r| r.rule == "Bash(rm:*)" && r.source == SettingSource::User),
        "user ask rule tagged with User source"
    );
}

#[test]
fn test_sourced_filesystem_allow_read_groups_by_source() {
    use crate::settings::SettingsWithSource;
    use crate::settings::source::SettingSource;

    let user_raw = serde_json::json!({
        "sandbox": { "filesystem": { "allow_read": ["/u/path"] } }
    });
    let policy_raw = serde_json::json!({
        "sandbox": { "filesystem": { "allow_read": ["/p/path1", "/p/path2"] } }
    });
    let mut per_source = std::collections::HashMap::new();
    per_source.insert(SettingSource::User, user_raw);
    per_source.insert(SettingSource::Policy, policy_raw);
    let swith = SettingsWithSource {
        merged: Settings::default(),
        per_source,
        source_paths: std::collections::HashMap::new(),
    };
    let groups = swith.sourced_filesystem_allow_read();

    let user_paths: Vec<&PathBuf> = groups
        .iter()
        .filter(|(s, _)| matches!(s, SettingSource::User))
        .flat_map(|(_, ps)| ps.iter())
        .collect();
    let policy_paths: Vec<&PathBuf> = groups
        .iter()
        .filter(|(s, _)| matches!(s, SettingSource::Policy))
        .flat_map(|(_, ps)| ps.iter())
        .collect();

    assert_eq!(user_paths, vec![&PathBuf::from("/u/path")]);
    assert_eq!(
        policy_paths,
        vec![&PathBuf::from("/p/path1"), &PathBuf::from("/p/path2")]
    );
}

#[test]
fn test_sourced_helpers_handle_missing_keys_gracefully() {
    use crate::settings::SettingsWithSource;
    use crate::settings::source::SettingSource;

    // No `permissions` block, no `sandbox.filesystem.allow_read` key.
    let raw = serde_json::json!({ "language": "en" });
    let mut per_source = std::collections::HashMap::new();
    per_source.insert(SettingSource::User, raw);
    let swith = SettingsWithSource {
        merged: Settings::default(),
        per_source,
        source_paths: std::collections::HashMap::new(),
    };

    let (allow, deny, ask) = swith.sourced_permission_rules();
    assert!(allow.is_empty());
    assert!(deny.is_empty());
    assert!(ask.is_empty());
    assert!(swith.sourced_filesystem_allow_read().is_empty());
}
