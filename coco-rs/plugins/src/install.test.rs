//! Tests for the shared install pipeline.
//!
//! Full end-to-end install requires a populated marketplace cache, which
//! the [`MarketplaceManager`] tests already cover. These tests focus on
//! the slim slice this module owns: argument parsing and the
//! no-marketplaces-configured early return. The install-success dependency
//! suffix is the canonical `dependency::format_dependency_count_suffix`
//! (covered in `dependency.test.rs`).

use super::*;
use crate::dependency::ResolutionResult;

#[test]
fn parse_install_target_no_marketplace() {
    assert_eq!(
        parse_install_target("my-plugin"),
        ("my-plugin".to_string(), None)
    );
}

#[test]
fn parse_install_target_with_marketplace() {
    assert_eq!(
        parse_install_target("my-plugin@official"),
        ("my-plugin".to_string(), Some("official".to_string()))
    );
}

#[test]
fn parse_install_target_trims_whitespace() {
    assert_eq!(
        parse_install_target("  my-plugin @ official  "),
        ("my-plugin".to_string(), Some("official".to_string()))
    );
}

#[tokio::test]
async fn install_returns_no_marketplaces_when_unconfigured() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let r = install_plugin_from_marketplace(
        tmp.path(),
        None,
        &EnterprisePolicy::default(),
        "anything@nowhere",
        PluginScope::User,
    )
    .await;
    match r {
        Err(InstallError::NoMarketplacesConfigured) => (),
        other => panic!("expected NoMarketplacesConfigured, got {other:?}"),
    }
}

#[test]
fn dep_note_uses_canonical_plus_n_suffix() {
    use crate::dependency::format_dependency_count_suffix;
    use crate::identifier::PluginId;
    let dep = |n: usize| -> Vec<PluginId> {
        (0..n)
            .map(|i| PluginId::new(format!("dep{i}"), "mkt".to_string()))
            .collect()
    };
    assert_eq!(format_dependency_count_suffix(&dep(0)), "");
    assert_eq!(format_dependency_count_suffix(&dep(1)), " (+ 1 dependency)");
    assert_eq!(
        format_dependency_count_suffix(&dep(2)),
        " (+ 2 dependencies)"
    );
}

#[test]
fn format_resolution_renders_each_variant() {
    use crate::identifier::PluginId;
    let cycle = ResolutionResult::Cycle {
        chain: vec![
            PluginId::new("a", "m"),
            PluginId::new("b", "m"),
            PluginId::new("a", "m"),
        ],
    };
    assert!(format_resolution(&cycle).contains("Dependency cycle"));

    let cross = ResolutionResult::CrossMarketplace {
        dependency: PluginId::new("dep", "other"),
        required_by: PluginId::new("root", "m"),
    };
    let cross_msg = format_resolution(&cross);
    assert!(cross_msg.contains("cross-marketplace"));
    assert!(cross_msg.contains("dep@other"));

    let not_found = ResolutionResult::NotFound {
        missing: PluginId::new("missing", "m2"),
        required_by: PluginId::new("root", "m"),
    };
    let nf_msg = format_resolution(&not_found);
    assert!(nf_msg.contains("'missing@m2'"));
    assert!(nf_msg.contains("'m2' marketplace"));
}

#[test]
fn write_and_read_enabled_plugins_round_trip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let closure = vec![
        crate::identifier::PluginId::new("foo", "official"),
        crate::identifier::PluginId::new("bar", "official"),
    ];
    write_enabled_plugins(tmp.path(), &closure).expect("write");
    let read_back = read_enabled_plugins(Some(tmp.path()));
    assert_eq!(read_back.len(), 2);
    assert!(read_back.contains(&closure[0]));
    assert!(read_back.contains(&closure[1]));
}

#[test]
fn write_enabled_plugins_preserves_existing_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("settings.json"),
        r#"{ "other_field": 42, "enabled_plugins": { "keep@x": { "enabled": true } } }"#,
    )
    .unwrap();
    let closure = vec![crate::identifier::PluginId::new("new", "official")];
    write_enabled_plugins(tmp.path(), &closure).expect("write");
    let raw = std::fs::read_to_string(tmp.path().join("settings.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["other_field"], 42);
    assert!(v["enabled_plugins"]["keep@x"]["enabled"].as_bool().unwrap());
    assert!(
        v["enabled_plugins"]["new@official"]["enabled"]
            .as_bool()
            .unwrap()
    );
}
