use std::fs;

use coco_types::OAuthFlowId;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::read_codex_auth;

const VALID: &str = r#"{
  "tokens": {
    "access_token": "at-123",
    "refresh_token": "rt-456",
    "id_token": "id-789",
    "account_id": "acct-abc"
  }
}"#;

#[test]
fn test_read_codex_auth_maps_tokens_and_flow() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("auth.json");
    fs::write(&path, VALID).unwrap();

    let cred = read_codex_auth(&path).expect("valid codex file parses");
    assert_eq!(cred.flow, OAuthFlowId::OpenAiChatGpt);
    assert_eq!(cred.access_token, "at-123");
    assert_eq!(cred.refresh_token.as_deref(), Some("rt-456"));
    assert_eq!(cred.id_token.as_deref(), Some("id-789"));
    assert_eq!(cred.account_id.as_deref(), Some("acct-abc"));
    // No epoch on read — the persist path (`AuthService::import`) bumps it.
    assert_eq!(cred.login_epoch, 0);
}

#[test]
fn test_read_codex_auth_missing_tokens_object_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("auth.json");
    fs::write(&path, "{}").unwrap();
    assert!(read_codex_auth(&path).is_err());
}

#[test]
fn test_read_codex_auth_invalid_json_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("auth.json");
    fs::write(&path, "not json at all").unwrap();
    assert!(read_codex_auth(&path).is_err());
}

#[test]
fn test_read_codex_auth_missing_file_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("does-not-exist.json");
    assert!(read_codex_auth(&path).is_err());
}

#[cfg(unix)]
#[test]
fn test_read_codex_auth_rejects_symlink() {
    let dir = tempdir().unwrap();
    let target = dir.path().join("real.json");
    fs::write(&target, VALID).unwrap();
    let link = dir.path().join("link.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let err = read_codex_auth(&link).expect_err("symlinked source must be rejected");
    assert!(
        err.to_string().contains("symlink"),
        "expected symlink rejection, got: {err}"
    );
}
