use super::*;

#[test]
fn validate_paths_accepts_normal() {
    assert_eq!(validate_paths("foo/bar.md"), PathValidation::Ok);
    assert_eq!(validate_paths(""), PathValidation::Ok);
}

#[test]
fn validate_paths_rejects_absolute() {
    assert_eq!(validate_paths("/etc/passwd"), PathValidation::Absolute);
}

#[test]
fn validate_paths_rejects_dotdot() {
    assert_eq!(validate_paths("../escape"), PathValidation::DotDotSegment);
    assert_eq!(validate_paths("a/../b"), PathValidation::DotDotSegment);
}

#[test]
fn check_impersonation_blocks_official_name() {
    let r = check_impersonation("claude-plugins-official", false);
    assert!(matches!(r, ImpersonationResult::OfficialNameMatch { .. }));
}

#[test]
fn check_impersonation_blocks_variants() {
    for name in [
        "claude-plugin-official",
        "ClaudePluginsOfficial",
        "claude_plugins_official",
        "claude-plugins-official-extras",
        "anthropic-plugins",
        "claude-code-official",
    ] {
        let r = check_impersonation(name, false);
        assert!(
            matches!(r, ImpersonationResult::OfficialNameMatch { .. }),
            "should block {name}: got {r:?}"
        );
    }
}

#[test]
fn check_impersonation_allows_clean_names() {
    let r = check_impersonation("my-cool-plugin", false);
    assert_eq!(r, ImpersonationResult::Ok);
}

#[test]
fn check_impersonation_allows_official_marketplace() {
    let r = check_impersonation("claude-plugins-official", true);
    assert_eq!(r, ImpersonationResult::Ok);
}

#[test]
fn check_impersonation_catches_cyrillic_homograph() {
    // 'а' is Cyrillic small a (U+0430), which folds to ASCII 'a'.
    let r = check_impersonation("clаude-plugins-official", false);
    assert!(matches!(r, ImpersonationResult::HomographMatch { .. }));
}

#[test]
fn policy_blocks_marketplace_explicitly() {
    let p = EnterprisePolicy {
        blocked_marketplaces: vec!["evil".into()],
        ..Default::default()
    };
    let id = PluginId::parse("foo@evil");
    assert_eq!(
        check_policy(&id, false, &p),
        PolicyVerdict::BlockedMarketplace {
            marketplace: "evil".into()
        }
    );
}

#[test]
fn policy_strict_known_blocks_unlisted() {
    let p = EnterprisePolicy {
        strict_known_marketplaces: true,
        known_marketplaces: vec!["approved".into()],
        ..Default::default()
    };
    let unlisted = PluginId::parse("foo@unknown");
    assert!(matches!(
        check_policy(&unlisted, false, &p),
        PolicyVerdict::UnapprovedMarketplace { .. }
    ));
    let approved = PluginId::parse("foo@approved");
    assert_eq!(check_policy(&approved, false, &p), PolicyVerdict::Ok);
}

#[test]
fn policy_strict_user_scope_forbids_user_install() {
    let p = EnterprisePolicy {
        strict_plugin_only_customization: coco_config::StrictPluginOnlyCustomization::AllLocked(
            true,
        ),
        ..Default::default()
    };
    let id = PluginId::parse("foo@m");
    assert_eq!(
        check_policy(&id, true, &p),
        PolicyVerdict::UserScopeForbidden
    );
    assert_eq!(check_policy(&id, false, &p), PolicyVerdict::Ok);
}

#[test]
fn policy_blocks_specific_plugin_by_id() {
    // `enabledPlugins["foo@m"] === false` → per-plugin force-disable,
    // keyed by the `name@marketplace` display form.
    let p = EnterprisePolicy {
        blocked_plugins: ["foo@m".to_string()].into_iter().collect(),
        ..Default::default()
    };
    assert_eq!(
        check_policy(&PluginId::parse("foo@m"), false, &p),
        PolicyVerdict::BlockedPlugin {
            plugin: "foo@m".into()
        }
    );
    // A different plugin from the same marketplace is untouched.
    assert_eq!(
        check_policy(&PluginId::parse("bar@m"), false, &p),
        PolicyVerdict::Ok
    );
}

#[test]
fn policy_per_plugin_block_takes_precedence_over_user_scope() {
    // The per-plugin blocklist is the primary gate: it fires even when a
    // broader rule (user-scope forbidden) would also match, so the verdict
    // names the specific plugin rather than the generic scope rule.
    let p = EnterprisePolicy {
        blocked_plugins: ["foo@m".to_string()].into_iter().collect(),
        strict_plugin_only_customization: coco_config::StrictPluginOnlyCustomization::AllLocked(
            true,
        ),
        ..Default::default()
    };
    assert_eq!(
        check_policy(&PluginId::parse("foo@m"), true, &p),
        PolicyVerdict::BlockedPlugin {
            plugin: "foo@m".into()
        }
    );
}

#[test]
fn policy_blocks_bare_named_plugin() {
    // A blocked id without a marketplace (bare `name`) is still gated —
    // the per-plugin check runs before the `marketplace.is_none()` shortcut.
    let p = EnterprisePolicy {
        blocked_plugins: ["foo".to_string()].into_iter().collect(),
        ..Default::default()
    };
    assert_eq!(
        check_policy(&PluginId::bare("foo"), false, &p),
        PolicyVerdict::BlockedPlugin {
            plugin: "foo".into()
        }
    );
}

#[test]
fn policy_default_blocks_nothing() {
    // The all-empty default (no managed settings) must never block.
    let p = EnterprisePolicy::default();
    assert_eq!(
        check_policy(&PluginId::parse("foo@m"), true, &p),
        PolicyVerdict::Ok
    );
}

#[test]
fn from_policy_settings_maps_all_managed_fields() {
    // Mirror a managed-settings.json policy layer.
    let policy: coco_config::Settings = serde_json::from_str(
        r#"{
            "enabled_plugins": {
                "blocked@m": {"enabled": false},
                "kept@m": {"enabled": true}
            },
            "strict_known_marketplaces": ["approved"],
            "blocked_marketplaces": ["evil"],
            "strict_plugin_only_customization": true
        }"#,
    )
    .expect("settings json");

    let p = EnterprisePolicy::from_policy_settings(&policy);

    // Per-plugin blocklist: only `enabled == false` entries.
    assert!(p.blocked_plugins.contains("blocked@m"));
    assert!(!p.blocked_plugins.contains("kept@m"));
    // Marketplace-level: allowlist (presence ⇒ strict) + denylist + scope flag.
    assert!(p.strict_known_marketplaces);
    assert_eq!(p.known_marketplaces, vec!["approved".to_string()]);
    assert_eq!(p.blocked_marketplaces, vec!["evil".to_string()]);
    assert_eq!(
        p.strict_plugin_only_customization,
        coco_config::StrictPluginOnlyCustomization::AllLocked(true)
    );
    assert!(
        p.strict_plugin_only_customization
            .is_restricted_to_plugin_only("plugins")
    );

    // And the verdicts wire through.
    assert_eq!(
        check_policy(&PluginId::parse("blocked@m"), false, &p),
        PolicyVerdict::BlockedPlugin {
            plugin: "blocked@m".into()
        }
    );
    assert!(matches!(
        check_policy(&PluginId::parse("x@evil"), false, &p),
        PolicyVerdict::BlockedMarketplace { .. }
    ));
    assert!(matches!(
        check_policy(&PluginId::parse("x@unlisted"), false, &p),
        PolicyVerdict::UnapprovedMarketplace { .. }
    ));
}
