use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use coco_sandbox::EnforcementLevel;
use coco_sandbox::SandboxConfig;
use coco_sandbox::SandboxSettings;
use coco_sandbox::SandboxState;
use coco_sandbox::WritableRoot;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolUseContext;

use super::preflight_path;

fn make_state(config: SandboxConfig) -> Arc<SandboxState> {
    let settings = SandboxSettings::enabled();
    let platform = coco_sandbox::platform::create_platform();
    Arc::new(SandboxState::new(
        config.enforcement,
        settings,
        config,
        platform,
    ))
}

fn ctx_with(state: Option<Arc<SandboxState>>) -> ToolUseContext {
    let mut ctx = ToolUseContext::test_default();
    ctx.sandbox_state = state;
    ctx
}

#[tokio::test]
async fn preflight_path_passes_when_no_sandbox_state() {
    let ctx = ctx_with(None);
    preflight_path(&ctx, Path::new("/anywhere"), false)
        .await
        .expect("read ok");
    preflight_path(&ctx, Path::new("/anywhere"), true)
        .await
        .expect("write ok");
}

#[tokio::test]
async fn preflight_path_passes_when_disabled_enforcement() {
    let state = make_state(SandboxConfig {
        enforcement: EnforcementLevel::Disabled,
        ..SandboxConfig::default()
    });
    let ctx = ctx_with(Some(state));
    preflight_path(&ctx, Path::new("/etc/passwd"), false)
        .await
        .expect("read ok");
    preflight_path(&ctx, Path::new("/var/log/x"), true)
        .await
        .expect("write ok");
}

#[tokio::test]
async fn preflight_path_denies_read_under_denied_read_path() {
    let state = make_state(SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        denied_read_paths: vec![PathBuf::from("/etc/shadow")],
        ..SandboxConfig::default()
    });
    let ctx = ctx_with(Some(state));
    let err = preflight_path(&ctx, Path::new("/etc/shadow/group"), false)
        .await
        .expect_err("read should be denied");
    assert!(matches!(err, ToolError::PermissionDenied { .. }));
}

#[tokio::test]
async fn preflight_path_denies_write_in_read_only_mode() {
    let state = make_state(SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        ..SandboxConfig::default()
    });
    let ctx = ctx_with(Some(state));
    let err = preflight_path(&ctx, Path::new("/tmp/foo"), true)
        .await
        .expect_err("write should be denied in read-only");
    assert!(matches!(err, ToolError::PermissionDenied { .. }));
}

#[tokio::test]
async fn preflight_path_allows_write_under_writable_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = make_state(SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        writable_roots: vec![WritableRoot::unprotected(tmp.path())],
        ..SandboxConfig::default()
    });
    let ctx = ctx_with(Some(state));
    let target = tmp.path().join("file.txt");
    preflight_path(&ctx, &target, true)
        .await
        .expect("write should pass under writable root");
}

#[tokio::test]
async fn preflight_path_denies_write_outside_writable_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let state = make_state(SandboxConfig {
        enforcement: EnforcementLevel::WorkspaceWrite,
        writable_roots: vec![WritableRoot::unprotected(tmp.path())],
        ..SandboxConfig::default()
    });
    let ctx = ctx_with(Some(state));
    let err = preflight_path(&ctx, Path::new("/etc/foo"), true)
        .await
        .expect_err("write outside writable_roots should be denied");
    assert!(matches!(err, ToolError::PermissionDenied { .. }));
}

#[tokio::test]
async fn preflight_path_picks_up_hot_reload() {
    // Hot-reload check: state starts in ReadOnly (deny writes), then
    // gets reconfigured to FullAccess. Pre-flight calls a fresh checker
    // each time, so the second write should pass without re-binding ctx.
    let initial = SandboxConfig {
        enforcement: EnforcementLevel::ReadOnly,
        ..SandboxConfig::default()
    };
    let state = make_state(initial);
    let ctx = ctx_with(Some(state.clone()));

    // First call: read-only — write denied.
    assert!(matches!(
        preflight_path(&ctx, Path::new("/tmp/foo"), true).await,
        Err(ToolError::PermissionDenied { .. })
    ));

    // Hot-reload to Disabled enforcement.
    state.update_config(
        EnforcementLevel::Disabled,
        SandboxSettings::default(),
        SandboxConfig {
            enforcement: EnforcementLevel::Disabled,
            ..SandboxConfig::default()
        },
    );

    // Second call: ctx unchanged, but the live state now allows writes.
    preflight_path(&ctx, Path::new("/tmp/foo"), true)
        .await
        .expect("write should pass after reload");
}
