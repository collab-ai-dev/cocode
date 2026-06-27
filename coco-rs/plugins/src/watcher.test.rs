//! Tests for the plugin change detector.

use super::*;
use std::path::Path;

#[test]
fn is_interesting_accepts_plugin_toml() {
    assert!(is_interesting_plugin_path(Path::new("/x/PLUGIN.toml")));
    assert!(is_interesting_plugin_path(Path::new(
        "/x/installed_plugins.json"
    )));
    assert!(is_interesting_plugin_path(Path::new("/x/marketplace.json")));
    assert!(is_interesting_plugin_path(Path::new("/x/some-skill.md")));
}

#[test]
fn is_interesting_rejects_editor_temp_files() {
    assert!(!is_interesting_plugin_path(Path::new("/x/.#PLUGIN.toml")));
    assert!(!is_interesting_plugin_path(Path::new("/x/PLUGIN.toml~")));
    assert!(!is_interesting_plugin_path(Path::new(
        "/x/.PLUGIN.toml.swp"
    )));
}

#[test]
fn derive_reason_picks_known_filenames() {
    assert_eq!(
        derive_reason(&[PathBuf::from("/x/installed_plugins.json")]),
        "installed_plugins.json changed"
    );
    assert_eq!(
        derive_reason(&[PathBuf::from("/x/my-plugin/PLUGIN.toml")]),
        "PLUGIN.toml changed"
    );
    assert_eq!(
        derive_reason(&[PathBuf::from("/x/marketplaces/foo/marketplace.json")]),
        "marketplace.json changed"
    );
}

#[test]
fn derive_reason_falls_back_to_first_filename() {
    assert_eq!(
        derive_reason(&[PathBuf::from("/x/skills/my-skill.md")]),
        "my-skill.md changed"
    );
}

#[test]
fn classify_emits_plugin_change_for_interesting_paths() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let target = tmp.path().join("PLUGIN.toml");
    let event =
        coco_file_watch::Event::new(coco_file_watch::EventKind::Any).add_path(target.clone());

    let changed = classify(&event).expect("interesting plugin path");
    assert_eq!(changed.changed_paths, vec![target]);
    assert_eq!(changed.reason, "PLUGIN.toml changed");
}

#[test]
fn classify_ignores_editor_temp_paths() {
    let event = coco_file_watch::Event::new(coco_file_watch::EventKind::Any)
        .add_path(PathBuf::from("/x/.#PLUGIN.toml"))
        .add_path(PathBuf::from("/x/PLUGIN.toml~"))
        .add_path(PathBuf::from("/x/.PLUGIN.toml.swp"));

    assert!(classify(&event).is_none());
}

#[test]
fn merge_preserves_first_reason_and_appends_paths() {
    let merged = merge(
        PluginsChanged {
            changed_paths: vec![PathBuf::from("/x/PLUGIN.toml")],
            reason: "PLUGIN.toml changed".to_string(),
        },
        PluginsChanged {
            changed_paths: vec![PathBuf::from("/x/skill.md")],
            reason: "skill.md changed".to_string(),
        },
    );

    assert_eq!(merged.reason, "PLUGIN.toml changed");
    assert_eq!(
        merged.changed_paths,
        vec![
            PathBuf::from("/x/PLUGIN.toml"),
            PathBuf::from("/x/skill.md")
        ]
    );
}
