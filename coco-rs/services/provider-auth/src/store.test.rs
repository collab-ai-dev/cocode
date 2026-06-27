use super::*;
use coco_types::OAuthFlowId;
use pretty_assertions::assert_eq;

fn sample() -> StoredCredential {
    StoredCredential {
        flow: OAuthFlowId::OpenAiChatGpt,
        access_token: "ACCESS-TOKEN-SECRET".into(),
        refresh_token: Some("REFRESH-TOKEN-SECRET".into()),
        id_token: Some("ID-TOKEN-SECRET".into()),
        account_id: Some("acct_1".into()),
        expires_at_ms: Some(123),
        plan_type: Some("pro".into()),
        email: Some("a@b.c".into()),
        login_epoch: 2,
    }
}

#[test]
fn ephemeral_roundtrip() {
    let b = EphemeralBackend::default();
    assert!(b.load("openai-chatgpt").unwrap().is_none());
    b.save("openai-chatgpt", &sample()).unwrap();
    let got = b.load("openai-chatgpt").unwrap().unwrap();
    assert_eq!(got.access_token, "ACCESS-TOKEN-SECRET");
    assert_eq!(got.login_epoch, 2);
    assert!(b.delete("openai-chatgpt").unwrap());
    assert!(b.load("openai-chatgpt").unwrap().is_none());
}

#[test]
fn file_roundtrip_and_mode() {
    let tmp = tempfile::tempdir().unwrap();
    // Point at a not-yet-existing subdir so `ensure_dir` (not tempfile) creates it.
    let auth_dir = tmp.path().join("auth");
    let b = FileBackend::new(auth_dir.clone());
    assert!(b.load("openai-chatgpt").unwrap().is_none());
    b.save("openai-chatgpt", &sample()).unwrap();
    let got = b.load("openai-chatgpt").unwrap().unwrap();
    assert_eq!(got.refresh_token.as_deref(), Some("REFRESH-TOKEN-SECRET"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The credential file is 0600.
        let meta = std::fs::metadata(auth_dir.join("openai-chatgpt.json")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        // And the auth directory itself is 0700, not the umask default.
        let dir_mode = std::fs::metadata(&auth_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "auth dir must be created 0700");
    }
}

/// A provider-instance name with path separators / traversal must be rejected,
/// never used to write a 0600 secret outside the auth dir.
#[test]
fn file_backend_rejects_unsafe_names() {
    let dir = tempfile::tempdir().unwrap();
    let b = FileBackend::new(dir.path().to_path_buf());
    for bad in ["../escape", "a/b", "/abs", "", "has space", "dots..dots"] {
        assert!(b.save(bad, &sample()).is_err(), "save must reject {bad:?}");
        assert!(b.load(bad).is_err(), "load must reject {bad:?}");
        assert!(b.delete(bad).is_err(), "delete must reject {bad:?}");
    }
    // A valid slug still works.
    assert!(b.save("openai-chat_oauth-2", &sample()).is_ok());
}

#[test]
fn debug_redacts_tokens() {
    let dbg = format!("{:?}", sample());
    assert!(dbg.contains("<redacted>"));
    assert!(!dbg.contains("ACCESS-TOKEN-SECRET"));
    assert!(!dbg.contains("REFRESH-TOKEN-SECRET"));
    assert!(!dbg.contains("ID-TOKEN-SECRET"));
    // Non-secret fields remain visible.
    assert!(dbg.contains("acct_1"));
}
