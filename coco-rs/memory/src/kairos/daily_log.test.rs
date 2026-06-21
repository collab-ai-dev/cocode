use super::*;
use coco_paths::ProjectPaths;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

fn paths() -> ProjectPaths {
    ProjectPaths::new(
        PathBuf::from("/home/u/.coco"),
        std::path::Path::new("/Users/foo/proj"),
    )
}

#[test]
fn daily_log_path_matches_ts_layout() {
    assert_eq!(
        daily_log_path(&paths(), 2026, 5, 16),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory/logs/2026/05/2026-05-16.md"),
    );
}

#[test]
fn daily_log_path_under_for_override_dir() {
    assert_eq!(
        daily_log_path_under(std::path::Path::new("/custom/mem"), 2026, 11, 23),
        PathBuf::from("/custom/mem/logs/2026/11/2026-11-23.md"),
    );
}

#[tokio::test]
async fn append_creates_parents_and_appends_with_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::new(tmp.path().to_path_buf(), std::path::Path::new("/p"));
    let store = DailyLogStore::new(&paths);

    store
        .append("- 10:00 first entry", 2026, 5, 16)
        .await
        .unwrap();
    store
        .append("- 10:05 second entry", 2026, 5, 16)
        .await
        .unwrap();

    let path = paths.daily_log(2026, 5, 16);
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "- 10:00 first entry\n- 10:05 second entry\n");
}

#[tokio::test]
async fn append_preserves_caller_provided_trailing_newline() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = ProjectPaths::new(tmp.path().to_path_buf(), std::path::Path::new("/p"));
    let store = DailyLogStore::new(&paths);

    store.append("- a\n", 2026, 1, 1).await.unwrap();
    let path = paths.daily_log(2026, 1, 1);
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "- a\n");
}
