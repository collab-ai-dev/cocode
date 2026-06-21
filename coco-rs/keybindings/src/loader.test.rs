use super::default_keybindings_path;
use super::load_keybindings;
use crate::KeybindingAction;
use crate::validator::Severity;
use std::io::Write;

#[tokio::test]
async fn missing_file_returns_defaults_silently() {
    let dir = tempdir();
    let path = dir.path().join("does_not_exist.json");
    let result = load_keybindings(&path).await;
    assert!(result.warnings.is_empty());
    assert!(!result.bindings.is_empty(), "defaults always present");
}

#[tokio::test]
async fn malformed_json_surfaces_error_warning() {
    let dir = tempdir();
    let path = dir.path().join("keybindings.json");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"{ not valid json").unwrap();
    drop(f);
    let result = load_keybindings(&path).await;
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.severity == Severity::Error),
        "malformed JSON must surface as error",
    );
    // Defaults still load so the UI keeps working.
    assert!(!result.bindings.is_empty());
}

#[tokio::test]
async fn user_binding_merges_with_defaults() {
    let dir = tempdir();
    let path = dir.path().join("keybindings.json");
    let json = r#"{
        "bindings": [
            { "context": "Chat", "bindings": { "ctrl+y": "chat:submit" } }
        ]
    }"#;
    std::fs::write(&path, json).unwrap();

    let result = load_keybindings(&path).await;
    assert!(
        result.warnings.is_empty(),
        "expected no warnings, got {:?}",
        result.warnings
    );

    // The user's `ctrl+y` binding shows up after defaults — last-wins
    // ensures the user override takes effect when the resolver builds.
    let user_binding = result.bindings.iter().find(|b| {
        b.action == Some(KeybindingAction::ChatSubmit)
            && b.context == crate::KeybindingContext::Chat
    });
    assert!(user_binding.is_some(), "user chat:submit binding present");
}

#[tokio::test]
async fn null_unbind_round_trips() {
    let dir = tempdir();
    let path = dir.path().join("keybindings.json");
    // Bind a chord to null in user config.
    let json = r#"{
        "bindings": [
            { "context": "Chat", "bindings": { "ctrl+t": null } }
        ]
    }"#;
    std::fs::write(&path, json).unwrap();
    let result = load_keybindings(&path).await;
    let unbind = result
        .bindings
        .iter()
        .find(|b| b.context == crate::KeybindingContext::Chat && b.action.is_none());
    assert!(unbind.is_some(), "null unbind preserved through loader");
}

#[test]
fn default_path_is_under_coco_home() {
    let path = default_keybindings_path();
    let s = path.to_string_lossy();
    assert!(s.contains(".coco"));
    assert!(s.ends_with("keybindings.json"));
}

// Lightweight tempdir helper — avoid a tempfile dep in the keybindings
// crate; tests run sequentially per-process so the PID-namespaced dir
// is collision-free.
struct TempDir(std::path::PathBuf);
impl TempDir {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn tempdir() -> TempDir {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let path = std::env::temp_dir().join(format!(
        "coco_keybindings_test_{}_{}",
        std::process::id(),
        nanos,
    ));
    std::fs::create_dir_all(&path).unwrap();
    TempDir(path)
}
