use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use coco_config::CatalogPaths;
use coco_config::EnvSnapshot;
use coco_config::RoleSlots;
use coco_config::RuntimeConfig;
use coco_config::RuntimeOverrides;
use coco_config::RuntimePublisher;
use coco_config::SandboxSettings;
use coco_config::Settings;
use coco_config::build_runtime_config_with;
use coco_config::settings::SettingsWithSource;
use coco_sandbox::EnforcementLevel;
use coco_sandbox::SandboxState;
use coco_types::Feature;
use coco_types::ProviderModelSelection;
use coco_types::SandboxMode;
use tempfile::TempDir;

use super::*;

fn settings_with(merged: Settings) -> SettingsWithSource {
    SettingsWithSource {
        merged,
        per_source: HashMap::new(),
        source_paths: HashMap::new(),
    }
}

fn build_test_runtime(mut merged: Settings) -> (TempDir, RuntimeConfig) {
    // Multi-LLM SDK has no implicit Main fallback — sandbox tests
    // don't care about the model, but the build now requires one.
    if merged.models.main.is_none() {
        merged.models.main = Some(RoleSlots::new(ProviderModelSelection {
            provider: "anthropic".into(),
            model_id: "claude-opus-4-7".into(),
        }));
    }
    let tmp = TempDir::new().expect("tempdir");
    let catalogs = CatalogPaths::empty_in(tmp.path());
    let runtime = build_runtime_config_with(
        settings_with(merged),
        EnvSnapshot::default(),
        RuntimeOverrides::default(),
        catalogs,
        coco_config::parse_enabled_setting_sources(None),
    )
    .expect("runtime build");
    (tmp, runtime)
}

fn enabled_sandbox_settings() -> SandboxSettings {
    SandboxSettings {
        enabled: true,
        mode: SandboxMode::WorkspaceWrite,
        ..SandboxSettings::default()
    }
}

fn enable_feature(merged: &mut Settings) {
    merged.features.insert("sandbox".to_string(), true);
}

fn make_state() -> Arc<SandboxState> {
    Arc::new(SandboxState::disabled())
}

#[tokio::test]
async fn reapply_sandbox_disables_when_feature_off() {
    let cwd = std::env::current_dir().expect("cwd");
    let merged = Settings {
        sandbox: enabled_sandbox_settings(),
        ..Default::default()
    };
    // Feature::Sandbox is OFF — reapply should disable enforcement in-place.
    let (_tmp, runtime) = build_test_runtime(merged);

    let state = make_state();
    reapply_sandbox(&state, &runtime, &cwd).expect("reapply");

    assert_eq!(state.enforcement(), EnforcementLevel::Disabled);
    assert!(!state.settings().enabled);
}

#[tokio::test]
async fn reapply_sandbox_disables_when_full_access() {
    let cwd = std::env::current_dir().expect("cwd");
    let mut merged = Settings {
        sandbox: SandboxSettings {
            enabled: true,
            mode: SandboxMode::FullAccess,
            ..SandboxSettings::default()
        },
        ..Default::default()
    };
    enable_feature(&mut merged);
    let (_tmp, runtime) = build_test_runtime(merged);

    let state = make_state();
    reapply_sandbox(&state, &runtime, &cwd).expect("reapply");

    assert_eq!(state.enforcement(), EnforcementLevel::Disabled);
}

#[tokio::test]
async fn reapply_sandbox_applies_settings_when_enabled() {
    let cwd = std::env::current_dir().expect("cwd");
    let mut sandbox = enabled_sandbox_settings();
    sandbox.filesystem.deny_read = vec![PathBuf::from("/etc/shadow")];
    let mut merged = Settings {
        sandbox,
        ..Default::default()
    };
    enable_feature(&mut merged);
    let (_tmp, runtime) = build_test_runtime(merged);

    let state = make_state();
    reapply_sandbox(&state, &runtime, &cwd).expect("reapply");

    assert_eq!(state.enforcement(), EnforcementLevel::WorkspaceWrite);
    assert!(state.settings().enabled);
    let cfg = state.config();
    assert!(
        cfg.denied_read_paths
            .iter()
            .any(|p| p == std::path::Path::new("/etc/shadow")),
        "deny_read should propagate; got: {:?}",
        cfg.denied_read_paths,
    );
}

#[tokio::test]
async fn spawn_loop_propagates_published_snapshot() {
    let cwd = std::env::current_dir().expect("cwd");

    // Initial RuntimeConfig has Sandbox feature off, Workspace mode.
    let initial = Settings {
        sandbox: enabled_sandbox_settings(),
        ..Default::default()
    };
    let (tmp_initial, initial_runtime) = build_test_runtime(initial);

    let publisher = Arc::new(RuntimePublisher::new(Arc::new(initial_runtime)));
    let state = make_state();

    let _handle = spawn_sandbox_reload(state.clone(), &publisher, cwd.clone());

    // Publish a snapshot WITH the feature on + a deny_read entry.
    let mut sandbox_next = enabled_sandbox_settings();
    sandbox_next.filesystem.deny_read = vec![PathBuf::from("/etc/shadow")];
    let mut next = Settings {
        sandbox: sandbox_next,
        ..Default::default()
    };
    enable_feature(&mut next);
    let (_tmp_next, next_runtime) = build_test_runtime(next);
    publisher.publish(Arc::new(next_runtime));

    // Poll for the propagated update — `update_config` is sync but
    // the spawned task must observe the publish and run reapply first.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if state.enforcement() == EnforcementLevel::WorkspaceWrite {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            panic!(
                "reload did not propagate; enforcement={:?}",
                state.enforcement()
            );
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let cfg = state.config();
    assert!(
        cfg.denied_read_paths
            .iter()
            .any(|p| p == std::path::Path::new("/etc/shadow")),
        "published deny_read should land on the live state; got: {:?}",
        cfg.denied_read_paths,
    );

    drop(tmp_initial);
}

#[tokio::test]
async fn spawn_loop_exits_when_publisher_dropped() {
    let cwd = std::env::current_dir().expect("cwd");
    let initial = Settings {
        sandbox: enabled_sandbox_settings(),
        ..Default::default()
    };
    let (_tmp, initial_runtime) = build_test_runtime(initial);

    let publisher = Arc::new(RuntimePublisher::new(Arc::new(initial_runtime)));
    let state = make_state();

    let handle = spawn_sandbox_reload(state, &publisher, cwd);

    // Drop the publisher → all senders gone → spawned task exits cleanly.
    drop(publisher);

    // The task should finish within a reasonable timeout.
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("subscriber should exit when publisher drops")
        .expect("task should not panic");
}

// Unused import suppressor: keep `Feature` referenced even when other
// branches are gated out, so the import stays meaningful for readers.
#[allow(dead_code)]
const _FEATURE_SANDBOX: Feature = Feature::Sandbox;
