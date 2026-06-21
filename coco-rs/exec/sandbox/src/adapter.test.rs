use std::path::Path;
use std::path::PathBuf;

use coco_types::SandboxMode;
use pretty_assertions::assert_eq;

use super::*;
use crate::config::SandboxSettings;

fn empty_inputs<'a>(
    settings: &'a SandboxSettings,
    cwd: &'a Path,
    files: &'a [PathBuf],
) -> AdapterInputs<'a> {
    AdapterInputs {
        settings,
        mode: SandboxMode::WorkspaceWrite,
        settings_root: Path::new("/proj"),
        original_cwd: cwd,
        current_cwd: cwd,
        permission_allow_rules: &[],
        permission_deny_rules: &[],
        additional_directories: &[],
        coco_temp_dir: Path::new("/tmp/coco"),
        settings_files: files,
        worktree_main_repo: None,
        sourced_permission_allow_rules: None,
        sourced_filesystem_allow_read: None,
    }
}

#[test]
fn test_resolve_permission_rule_double_slash_is_absolute_root() {
    let p = resolve_permission_rule_path("//etc/passwd", Path::new("/proj"));
    assert_eq!(p, PathBuf::from("/etc/passwd"));
}

#[test]
fn test_resolve_permission_rule_single_slash_is_settings_relative() {
    let p = resolve_permission_rule_path("/foo/**", Path::new("/proj"));
    assert_eq!(p, PathBuf::from("/proj/foo/**"));
}

#[test]
fn test_resolve_permission_rule_relative_unchanged() {
    let p = resolve_permission_rule_path("./bar", Path::new("/proj"));
    assert_eq!(p, PathBuf::from("./bar"));
}

#[test]
fn test_resolve_filesystem_path_absolute_stays_absolute() {
    let p = resolve_filesystem_path(Path::new("/Users/foo/.cargo"), Path::new("/proj"));
    assert_eq!(p, PathBuf::from("/Users/foo/.cargo"));
}

#[test]
fn test_resolve_filesystem_path_relative_to_settings_root() {
    let p = resolve_filesystem_path(Path::new("nested/dir"), Path::new("/proj"));
    assert_eq!(p, PathBuf::from("/proj/nested/dir"));
}

#[test]
fn test_resolve_filesystem_path_double_slash_legacy_escape() {
    let p = resolve_filesystem_path(Path::new("//Users/foo"), Path::new("/proj"));
    assert_eq!(p, PathBuf::from("/Users/foo"));
}

#[test]
fn test_extract_webfetch_domain_basic() {
    assert_eq!(
        extract_webfetch_domain("WebFetch(domain:example.com)"),
        Some("example.com".into())
    );
}

#[test]
fn test_extract_webfetch_domain_strips_whitespace() {
    assert_eq!(
        extract_webfetch_domain("WebFetch(domain: api.example.com )"),
        Some("api.example.com".into())
    );
}

#[test]
fn test_extract_webfetch_domain_rejects_other_tools() {
    assert_eq!(extract_webfetch_domain("Edit(/foo)"), None);
    assert_eq!(extract_webfetch_domain("Bash(curl *)"), None);
    assert_eq!(extract_webfetch_domain("WebFetch(other:value)"), None);
}

#[test]
fn test_extract_path_for_tool_edit() {
    assert_eq!(
        extract_path_for_tool("Edit(/src/foo)", "Edit"),
        Some("/src/foo".into())
    );
    assert_eq!(extract_path_for_tool("Read(/src/foo)", "Edit"), None);
}

#[test]
fn test_build_runtime_config_includes_cwd_as_writable_root() {
    let settings = SandboxSettings::default();
    let inputs = empty_inputs(&settings, Path::new("/proj"), &[]);
    let out = build_runtime_config(inputs);
    assert!(
        out.config
            .writable_roots
            .iter()
            .any(|r| r.path == Path::new("/proj"))
    );
    assert!(
        out.config
            .writable_roots
            .iter()
            .any(|r| r.path == Path::new("/tmp/coco"))
    );
}

#[test]
fn test_build_runtime_config_blocks_settings_files() {
    let settings = SandboxSettings::default();
    let settings_file = PathBuf::from("/proj/.coco/settings.json");
    let inputs = empty_inputs(
        &settings,
        Path::new("/proj"),
        std::slice::from_ref(&settings_file),
    );
    let out = build_runtime_config(inputs);
    assert!(out.config.deny_write_paths.contains(&settings_file));
    assert!(
        out.config
            .deny_write_paths
            .contains(&PathBuf::from("/proj/.coco/skills"))
    );
    assert!(
        !out.config
            .deny_write_paths
            .contains(&PathBuf::from("/proj/.claude/skills"))
    );
}

#[test]
fn test_build_runtime_config_extracts_webfetch_domains() {
    let settings = SandboxSettings::default();
    let allow = vec!["WebFetch(domain:api.example.com)".to_string()];
    let deny = vec!["WebFetch(domain:tracker.evil.com)".to_string()];
    let inputs = AdapterInputs {
        permission_allow_rules: &allow,
        permission_deny_rules: &deny,
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    assert!(
        out.settings
            .network
            .allowed_domains
            .contains(&"api.example.com".to_string())
    );
    assert!(
        out.settings
            .network
            .denied_domains
            .contains(&"tracker.evil.com".to_string())
    );
}

#[test]
fn test_allow_managed_domains_only_filters_to_policy_source() {
    use coco_config::SourcedRule;
    use coco_config::settings::source::SettingSource;

    // settings.json (merged) opts in to the policy gate. Without sourced
    // rules, the gate degrades to "all sources contribute"; with sourced
    // rules, only Policy-sourced allow rules pass.
    let mut settings = SandboxSettings::default();
    settings.network.allow_managed_domains_only = true;

    let user_allow = SourcedRule {
        rule: "WebFetch(domain:user.example.com)".to_string(),
        source: SettingSource::User,
    };
    let policy_allow = SourcedRule {
        rule: "WebFetch(domain:enterprise.example.com)".to_string(),
        source: SettingSource::Policy,
    };
    let sourced = vec![user_allow, policy_allow];

    // Flat list mirrors the per-source view (would normally come from
    // the same merge) — adapter ignores it on the sourced path.
    let flat_allow = vec![
        "WebFetch(domain:user.example.com)".to_string(),
        "WebFetch(domain:enterprise.example.com)".to_string(),
    ];

    let inputs = AdapterInputs {
        permission_allow_rules: &flat_allow,
        sourced_permission_allow_rules: Some(&sourced),
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    let allowed = &out.settings.network.allowed_domains;

    assert!(
        allowed.contains(&"enterprise.example.com".to_string()),
        "policy-sourced domain must contribute"
    );
    assert!(
        !allowed.contains(&"user.example.com".to_string()),
        "user-sourced domain must be filtered out by allow_managed_domains_only"
    );
}

#[test]
fn test_allow_managed_domains_only_off_lets_all_sources_through() {
    use coco_config::SourcedRule;
    use coco_config::settings::source::SettingSource;

    // Gate flag off → behaves identically to no sourced data.
    let settings = SandboxSettings::default();
    let sourced = vec![SourcedRule {
        rule: "WebFetch(domain:user.example.com)".to_string(),
        source: SettingSource::User,
    }];
    let flat = vec!["WebFetch(domain:user.example.com)".to_string()];

    let inputs = AdapterInputs {
        permission_allow_rules: &flat,
        sourced_permission_allow_rules: Some(&sourced),
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    assert!(
        out.settings
            .network
            .allowed_domains
            .contains(&"user.example.com".to_string()),
        "with gate off, user-sourced domain still contributes"
    );
}

#[test]
fn test_allow_managed_domains_only_denies_honored_from_all_sources() {
    use coco_config::SourcedRule;
    use coco_config::settings::source::SettingSource;

    // Even with the gate on, denied domains from non-policy sources
    // still apply (TS security floor).
    let mut settings = SandboxSettings::default();
    settings.network.allow_managed_domains_only = true;

    let sourced_allow = vec![SourcedRule {
        rule: "WebFetch(domain:enterprise.example.com)".to_string(),
        source: SettingSource::Policy,
    }];
    let flat_deny = vec!["WebFetch(domain:tracker.user.com)".to_string()];

    let inputs = AdapterInputs {
        permission_deny_rules: &flat_deny,
        sourced_permission_allow_rules: Some(&sourced_allow),
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    assert!(
        out.settings
            .network
            .denied_domains
            .contains(&"tracker.user.com".to_string()),
        "denials from any source must always be honored"
    );
}

#[test]
fn test_allow_managed_read_paths_only_filters_to_policy_source() {
    use coco_config::settings::source::SettingSource;

    let mut settings = SandboxSettings::default();
    settings.filesystem.allow_managed_read_paths_only = true;
    // Merged view contains paths from both sources; the adapter should
    // ignore it on the sourced path and use only policy-sourced entries.
    settings.filesystem.allow_read = vec![
        PathBuf::from("/etc/shadow/user_allowed"),
        PathBuf::from("/etc/shadow/enterprise_allowed"),
    ];

    let sourced: Vec<(SettingSource, Vec<PathBuf>)> = vec![
        (
            SettingSource::User,
            vec![PathBuf::from("/etc/shadow/user_allowed")],
        ),
        (
            SettingSource::Policy,
            vec![PathBuf::from("/etc/shadow/enterprise_allowed")],
        ),
    ];

    let inputs = AdapterInputs {
        sourced_filesystem_allow_read: Some(&sourced),
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    let allowed = &out.config.allowed_read_paths;

    assert!(
        allowed.contains(&PathBuf::from("/etc/shadow/enterprise_allowed")),
        "policy-sourced allow_read must contribute; got: {allowed:?}"
    );
    assert!(
        !allowed.contains(&PathBuf::from("/etc/shadow/user_allowed")),
        "user-sourced allow_read must be filtered out by the gate; got: {allowed:?}"
    );
}

#[test]
fn test_allow_managed_read_paths_only_falls_back_when_no_sourced_data() {
    // Gate ON but adapter receives no per-source data → degrade open
    // (use the merged settings.filesystem.allow_read).
    let mut settings = SandboxSettings::default();
    settings.filesystem.allow_managed_read_paths_only = true;
    settings.filesystem.allow_read = vec![PathBuf::from("/etc/shadow/some_path")];

    let inputs = AdapterInputs {
        sourced_filesystem_allow_read: None,
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    assert!(
        out.config
            .allowed_read_paths
            .contains(&PathBuf::from("/etc/shadow/some_path")),
        "without sourced data, the gate degrades open"
    );
}

#[test]
fn test_build_runtime_config_edit_rule_becomes_writable_root() {
    let settings = SandboxSettings::default();
    let allow = vec!["Edit(/extra/dir)".to_string()];
    let inputs = AdapterInputs {
        permission_allow_rules: &allow,
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    // "/extra/dir" is a single-slash permission-rule path — settings-relative.
    assert!(
        out.config
            .writable_roots
            .iter()
            .any(|r| r.path == Path::new("/proj/extra/dir"))
    );
}

#[test]
fn test_build_runtime_config_read_deny_becomes_denied_read() {
    let settings = SandboxSettings::default();
    let deny = vec!["Read(//secrets/token)".to_string()];
    let inputs = AdapterInputs {
        permission_deny_rules: &deny,
        ..empty_inputs(&settings, Path::new("/proj"), &[])
    };
    let out = build_runtime_config(inputs);
    // "//secrets/token" is double-slash → absolute root.
    assert!(
        out.config
            .denied_read_paths
            .contains(&PathBuf::from("/secrets/token"))
    );
}

#[test]
fn test_build_runtime_config_enforcement_from_mode() {
    let settings = SandboxSettings::default();
    let cwd = Path::new("/proj");
    for (mode, expected) in &[
        (SandboxMode::ReadOnly, EnforcementLevel::ReadOnly),
        (
            SandboxMode::WorkspaceWrite,
            EnforcementLevel::WorkspaceWrite,
        ),
        (SandboxMode::FullAccess, EnforcementLevel::Disabled),
        (
            SandboxMode::ExternalSandbox,
            EnforcementLevel::WorkspaceWrite,
        ),
    ] {
        let inputs = AdapterInputs {
            mode: *mode,
            ..empty_inputs(&settings, cwd, &[])
        };
        let out = build_runtime_config(inputs);
        assert_eq!(out.enforcement, *expected, "mode = {mode:?}");
    }
}

#[test]
fn test_sandbox_unavailable_reason_when_disabled_returns_none() {
    let settings = SandboxSettings::default(); // enabled = false
    let r = sandbox_unavailable_reason(&settings, /*supported*/ false, true, &[]);
    assert_eq!(r, None);
}

#[test]
fn test_sandbox_unavailable_reason_missing_deps() {
    let settings = SandboxSettings::enabled();
    let r = sandbox_unavailable_reason(&settings, true, true, &["bwrap".into()]);
    assert!(r.is_some(), "expected reason");
    assert!(r.unwrap().contains("bwrap"));
}

#[test]
fn test_dedup_paths_preserves_first_occurrence() {
    let mut paths = vec![
        PathBuf::from("/a"),
        PathBuf::from("/b"),
        PathBuf::from("/a"),
        PathBuf::from("/c"),
        PathBuf::from("/b"),
    ];
    dedup_paths(&mut paths);
    assert_eq!(
        paths,
        vec![
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ]
    );
}

#[test]
fn test_bare_repo_scrub_paths_only_returns_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    // Create one of the files; the rest don't exist and should be returned.
    std::fs::write(tmp.path().join("HEAD"), "fake").unwrap();
    let scrub = bare_repo_scrub_paths(tmp.path(), tmp.path());
    assert!(!scrub.contains(&tmp.path().join("HEAD")));
    assert!(scrub.contains(&tmp.path().join("objects")));
    assert!(scrub.contains(&tmp.path().join("config")));
}

/// scrub_bare_repo_files deletes anything passed in that exists, no-ops
/// for paths that don't, and never errors out on missing paths.
#[test]
fn test_scrub_bare_repo_files_removes_planted_files() {
    let tmp = tempfile::tempdir().unwrap();
    let head = tmp.path().join("HEAD");
    let objects = tmp.path().join("objects");
    std::fs::write(&head, "fake").unwrap();
    std::fs::create_dir(&objects).unwrap();
    std::fs::write(objects.join("inner"), "x").unwrap();
    let nonexistent = tmp.path().join("config");
    scrub_bare_repo_files(&[head.clone(), objects.clone(), nonexistent.clone()]);
    assert!(!head.exists(), "HEAD file should be deleted");
    assert!(
        !objects.exists(),
        "objects/ directory should be deleted recursively"
    );
    assert!(
        !nonexistent.exists(),
        "missing path stays missing without panicking"
    );
}

/// `settings.filesystem.allow_read` paths are collected and resolved
/// against the settings root, so the platform wrappers see absolute
/// carve-outs. TS parity: `entrypoints/sandboxTypes.ts:71-77`.
#[test]
fn test_filesystem_allow_read_paths_collected() {
    use crate::config::FilesystemConfig;
    let s = SandboxSettings {
        filesystem: FilesystemConfig {
            allow_read: vec![
                PathBuf::from("/etc/shadow/public"),
                PathBuf::from("relative/under/root"),
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    let inputs = empty_inputs(&s, Path::new("/proj"), &[]);
    let out = build_runtime_config(inputs);
    assert!(
        out.config
            .allowed_read_paths
            .iter()
            .any(|p| p == Path::new("/etc/shadow/public")),
        "absolute allow_read path must land in allowed_read_paths",
    );
    assert!(
        out.config
            .allowed_read_paths
            .iter()
            .any(|p| p == Path::new("/proj/relative/under/root")),
        "relative allow_read path must be resolved against settings_root",
    );
}

/// Filesystem `deny_read` entries with glob metacharacters are routed
/// into `denied_read_globs` rather than `denied_read_paths`.
#[test]
fn test_filesystem_deny_read_globs_routed_separately() {
    use crate::config::FilesystemConfig;
    let s = SandboxSettings {
        filesystem: FilesystemConfig {
            deny_read: vec![
                // Glob — should land in denied_read_globs.
                PathBuf::from("**/*.env"),
                PathBuf::from("secrets/?.txt"),
                // Literal — should land in denied_read_paths (resolved against root).
                PathBuf::from("/abs/literal"),
            ],
            ..Default::default()
        },
        ..Default::default()
    };
    let inputs = empty_inputs(&s, Path::new("/proj"), &[]);
    let out = build_runtime_config(inputs);
    assert!(
        out.config.denied_read_globs.iter().any(|g| g == "**/*.env"),
        "glob `**/*.env` must land in denied_read_globs",
    );
    assert!(
        out.config
            .denied_read_globs
            .iter()
            .any(|g| g == "secrets/?.txt"),
        "glob `secrets/?.txt` must land in denied_read_globs",
    );
    assert!(
        out.config
            .denied_read_paths
            .iter()
            .any(|p| p == Path::new("/abs/literal")),
        "literal path stays in denied_read_paths",
    );
}

// ── Network isolation posture ──────────────────────────────────────────────
//
// `allow_network == false` is the secure default once the sandbox is enabled:
// egress is isolated and routed through the per-domain proxy filter (TS keeps
// network filtered whenever the sandbox is on). These lock that posture so the
// `NetworkMode::default() == Full` regression cannot silently re-open it.

fn inputs_with_mode<'a>(settings: &'a SandboxSettings, mode: SandboxMode) -> AdapterInputs<'a> {
    AdapterInputs {
        mode,
        ..empty_inputs(settings, Path::new("/proj"), &[])
    }
}

#[test]
fn test_enabled_sandbox_default_isolates_network() {
    // Sandbox on, no network config at all — must isolate (allow_network=false).
    // This is the dominant config and the one the NetworkMode regression broke.
    let s = SandboxSettings {
        enabled: true,
        ..Default::default()
    };
    let out = build_runtime_config(inputs_with_mode(&s, SandboxMode::WorkspaceWrite));
    assert!(
        !out.config.allow_network,
        "enabled sandbox with default network must isolate (allow_network=false)",
    );
}

#[test]
fn test_full_access_mode_allows_network() {
    // FullAccess == "no sandbox restrictions", so network is unrestricted.
    let s = SandboxSettings {
        enabled: true,
        ..Default::default()
    };
    let out = build_runtime_config(inputs_with_mode(&s, SandboxMode::FullAccess));
    assert!(
        out.config.allow_network,
        "FullAccess mode must not isolate network",
    );
}

#[test]
fn test_allow_network_toggle_opts_out_of_isolation() {
    // The coarse `allow_network` toggle is the only opt-out from isolation.
    let s = SandboxSettings {
        enabled: true,
        allow_network: true,
        ..Default::default()
    };
    let out = build_runtime_config(inputs_with_mode(&s, SandboxMode::WorkspaceWrite));
    assert!(
        out.config.allow_network,
        "allow_network=true must bypass isolation",
    );
}

#[test]
fn test_limited_network_mode_does_not_relax_isolation() {
    // NetworkMode gates HTTP methods only; Limited must still isolate (and a
    // fortiori must not flip allow_network on like the old `mode != Full` did).
    use crate::config::{NetworkConfig, NetworkMode};
    let s = SandboxSettings {
        enabled: true,
        network: NetworkConfig {
            mode: NetworkMode::Limited,
            ..Default::default()
        },
        ..Default::default()
    };
    let out = build_runtime_config(inputs_with_mode(&s, SandboxMode::WorkspaceWrite));
    assert!(
        !out.config.allow_network,
        "Limited mode must still isolate network",
    );
}

#[test]
fn test_disabled_sandbox_does_not_isolate_network() {
    // No sandbox ⇒ no network restriction.
    let s = SandboxSettings {
        enabled: false,
        ..Default::default()
    };
    let out = build_runtime_config(inputs_with_mode(&s, SandboxMode::WorkspaceWrite));
    assert!(
        out.config.allow_network,
        "disabled sandbox must not isolate network",
    );
}
