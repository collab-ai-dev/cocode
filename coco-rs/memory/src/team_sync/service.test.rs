use super::*;
use crate::team_sync::types::TeamMemoryContent;

#[test]
fn test_compute_content_hash_matches_known_value() {
    // SHA-256 of "hello\n" = sha256:5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03
    let h = compute_content_hash("hello\n");
    assert_eq!(
        h,
        "sha256:5891b5b522d5df086d0ff0b110fbd9d21bb4fc7163af34d08286a2e846f6be03"
    );
}

#[test]
fn test_endpoint_url_encodes_repo_slug() {
    let url = endpoint("https://api.anthropic.com", "owner/repo with space");
    assert_eq!(
        url,
        "https://api.anthropic.com/api/claude_code/team_memory?repo=owner%2Frepo%20with%20space"
    );
}

#[test]
fn test_scan_only_filters_secrets() {
    let entries = vec![
        PushEntry {
            path: "MEMORY.md".into(),
            content: "Clean notes\n".into(),
        },
        PushEntry {
            path: "leak.md".into(),
            content: "key: ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n".into(),
        },
    ];
    let (clean, skipped) = scan_only(&entries);
    assert_eq!(clean.len(), 1);
    assert_eq!(clean[0].path, "MEMORY.md");
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].path, "leak.md");
    assert_eq!(skipped[0].rule_id, "github-pat");
}

#[tokio::test]
async fn test_apply_pulled_content_writes_files_and_creates_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut entries = std::collections::HashMap::new();
    entries.insert("MEMORY.md".into(), "- root note\n".into());
    entries.insert("subdir/notes.md".into(), "- nested\n".into());
    let content = TeamMemoryContent {
        entries,
        entry_checksums: Default::default(),
    };
    apply_pulled_content(tmp.path(), &content).await;
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("MEMORY.md")).unwrap(),
        "- root note\n"
    );
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("subdir/notes.md")).unwrap(),
        "- nested\n"
    );
}

#[tokio::test]
async fn test_apply_pulled_content_rejects_path_traversal() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut entries = std::collections::HashMap::new();
    entries.insert("../escape.md".into(), "evil".into());
    entries.insert("/abs/path.md".into(), "evil2".into());
    entries.insert("safe.md".into(), "ok".into());
    let content = TeamMemoryContent {
        entries,
        entry_checksums: Default::default(),
    };
    apply_pulled_content(tmp.path(), &content).await;
    // Only the safe key wrote; traversal/abs were rejected.
    assert!(tmp.path().join("safe.md").exists());
    assert!(!tmp.path().parent().unwrap().join("escape.md").exists());
}
