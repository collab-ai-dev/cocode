use std::fs;

use super::*;

#[test]
fn test_extract_directories_basic() {
    let files = vec![
        "src/main.rs".to_string(),
        "src/lib.rs".to_string(),
        "src/utils/mod.rs".to_string(),
        "Cargo.toml".to_string(),
    ];

    let dirs = extract_directories(&files);

    assert!(dirs.contains(&"src/".to_string()));
    assert!(dirs.contains(&"src/utils/".to_string()));
}

#[test]
fn test_extract_directories_nested() {
    let files = vec![
        "src/components/Button.tsx".to_string(),
        "src/components/Input.tsx".to_string(),
        "src/utils/helpers.ts".to_string(),
    ];

    let dirs = extract_directories(&files);

    assert!(dirs.contains(&"src/".to_string()));
    assert!(dirs.contains(&"src/components/".to_string()));
    assert!(dirs.contains(&"src/utils/".to_string()));
}

#[test]
fn test_file_index_cache_validity() {
    let index = FileIndex::new("/tmp");
    assert!(!index.is_cache_valid());
}

#[tokio::test]
async fn test_discovery_result_default() {
    let result = DiscoveryResult::default();
    assert!(result.files.is_empty());
    assert!(result.directories.is_empty());
}

#[test]
fn test_at_style_query_returns_files_and_directories_from_cache() {
    let mut index = FileIndex::new("/path/that/does/not/exist");
    index.apply_discovery(DiscoveryResult {
        files: vec!["src/main.rs".into(), "README.md".into()],
        directories: vec!["src/".into()],
    });

    let suggestions = index.get_suggestions("src", MAX_SUGGESTIONS);
    let paths = suggestions
        .iter()
        .map(|suggestion| suggestion.path.as_str())
        .collect::<Vec<_>>();

    assert!(paths.contains(&"src/"));
    assert!(paths.contains(&"src/main.rs"));
}

#[test]
fn test_directory_suggestions_are_slash_suffixed_and_marked() {
    let mut index = FileIndex::new("/path/that/does/not/exist");
    index.apply_discovery(DiscoveryResult {
        files: vec!["src/main.rs".into()],
        directories: vec!["src".into()],
    });

    let suggestion = index
        .get_suggestions("src", MAX_SUGGESTIONS)
        .into_iter()
        .find(|suggestion| suggestion.is_directory)
        .expect("directory suggestion");

    assert_eq!(suggestion.path, "src/");
    assert_eq!(suggestion.display_text, "src/");
    assert!(suggestion.is_directory);
}

#[test]
fn test_cached_search_does_not_require_existing_cwd() {
    let mut index = FileIndex::new("/path/that/does/not/exist");
    index.apply_discovery(DiscoveryResult {
        files: vec!["cached/file.rs".into()],
        directories: vec![],
    });

    let suggestions = index.get_suggestions("cached", MAX_SUGGESTIONS);

    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].path, "cached/file.rs");
}

#[test]
fn test_empty_query_returns_no_cached_suggestions() {
    let mut index = FileIndex::new("/path/that/does/not/exist");
    index.apply_discovery(DiscoveryResult {
        files: vec!["src/main.rs".into()],
        directories: vec!["src/".into()],
    });

    assert!(index.get_suggestions("", MAX_SUGGESTIONS).is_empty());
}

#[tokio::test]
async fn test_aborted_shared_refresh_does_not_prevent_later_refresh() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let file_path = temp_dir.path().join("refresh-target.rs");
    fs::write(&file_path, "fn main() {}\n").expect("write test file");

    let index = create_shared_index(temp_dir.path());
    let refresh_index = index.clone();
    let handle = tokio::spawn(async move {
        FileIndex::refresh_if_stale(&refresh_index).await;
    });
    handle.abort();

    FileIndex::refresh_if_stale(&index).await;

    let guard = index.read().await;
    let suggestions = guard.get_suggestions("refresh", MAX_SUGGESTIONS);
    assert!(
        suggestions
            .iter()
            .any(|suggestion| suggestion.path == "refresh-target.rs")
    );
}
