//! Tests for skill file extraction.

use super::*;

#[tokio::test]
async fn extract_writes_files_with_proper_layout() {
    let mut files = HashMap::new();
    files.insert("README.md".to_string(), "hello".to_string());
    files.insert(
        "lib/helper.js".to_string(),
        "module.exports = 1".to_string(),
    );

    let dir = extract_bundled_skill_files("test-skill-a", &files)
        .await
        .expect("extraction succeeds");

    let readme = tokio::fs::read_to_string(dir.join("README.md"))
        .await
        .unwrap();
    let helper = tokio::fs::read_to_string(dir.join("lib").join("helper.js"))
        .await
        .unwrap();
    assert_eq!(readme, "hello");
    assert_eq!(helper, "module.exports = 1");
}

#[tokio::test]
async fn concurrent_extraction_runs_once() {
    let mut files = HashMap::new();
    files.insert("only.txt".to_string(), "x".to_string());

    // Spawn 10 concurrent extractions for the same skill.
    let mut handles = Vec::new();
    for _ in 0..10 {
        let f = files.clone();
        handles.push(tokio::spawn(async move {
            extract_bundled_skill_files("test-skill-b", &f).await
        }));
    }
    let results: Vec<_> = futures_util_join_all(handles).await;
    let dirs: Vec<_> = results
        .into_iter()
        .map(|r| r.unwrap().expect("extraction succeeds"))
        .collect();

    // All 10 must report the same dir.
    let first = &dirs[0];
    for d in &dirs[1..] {
        assert_eq!(first, d);
    }

    // File exists exactly once.
    let content = tokio::fs::read_to_string(first.join("only.txt"))
        .await
        .unwrap();
    assert_eq!(content, "x");
}

async fn futures_util_join_all<T>(
    handles: Vec<tokio::task::JoinHandle<T>>,
) -> Vec<Result<T, tokio::task::JoinError>> {
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await);
    }
    out
}

#[test]
fn resolve_rejects_absolute() {
    let base = PathBuf::from("/tmp/base");
    let r = resolve_skill_file_path(&base, "/etc/passwd");
    assert!(r.is_err());
}

#[test]
fn resolve_rejects_dotdot_native_sep() {
    let base = PathBuf::from("/tmp/base");
    let r = resolve_skill_file_path(&base, "../escape.txt");
    assert!(r.is_err());
}

#[test]
fn resolve_rejects_dotdot_in_segment() {
    let base = PathBuf::from("/tmp/base");
    let r = resolve_skill_file_path(&base, "ok/../escape.txt");
    assert!(r.is_err());
}

#[test]
fn resolve_accepts_normal_path() {
    let base = PathBuf::from("/tmp/base");
    let r = resolve_skill_file_path(&base, "ok/file.txt").unwrap();
    assert_eq!(r, PathBuf::from("/tmp/base/ok/file.txt"));
}

#[test]
fn prepend_base_dir_format_matches_ts() {
    let result = prepend_base_dir("hello world", Path::new("/extract/dir"));
    assert_eq!(
        result,
        "Base directory for this skill: /extract/dir\n\nhello world"
    );
}
