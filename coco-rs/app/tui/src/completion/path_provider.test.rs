use super::*;

fn temp_completion_dir(test_name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "coco-tui-path-provider-{test_name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp completion dir");
    dir
}

fn request_key(kind: SuggestionKind, query: String, start_pos: usize) -> CompletionRequestKey {
    CompletionRequestKey {
        kind,
        token_range: start_pos..start_pos + query.len(),
        token_text: query.clone(),
        query,
        generation: 1,
    }
}

#[tokio::test]
async fn test_path_completion_manager_returns_explicit_path_rows() {
    let dir = temp_completion_dir("files");
    std::fs::write(dir.join("alpha.txt"), "").expect("write file");
    std::fs::create_dir_all(dir.join("assets")).expect("write directory");
    let (tx, mut rx) = create_path_completion_channel();
    let mut manager = PathCompletionManager::new(tx);
    let query = format!("{}/a", dir.display());

    manager.search(request_key(SuggestionKind::Path, query.clone(), 1));

    let event = rx.recv().await.expect("path result");
    let PathCompletionEvent::SearchResult { key, suggestions } = event;
    assert_eq!(key.kind, SuggestionKind::Path);
    assert_eq!(key.query, query);
    assert_eq!(key.token_range.start, 1);
    let labels = suggestions
        .iter()
        .map(|item| item.label.clone())
        .collect::<Vec<_>>();
    assert!(labels.contains(&format!("{}/alpha.txt", dir.display())));
    assert!(labels.contains(&format!("{}/assets", dir.display())));

    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn test_path_completion_manager_filters_directories_only() {
    let dir = temp_completion_dir("directories");
    std::fs::write(dir.join("alpha.txt"), "").expect("write file");
    std::fs::create_dir_all(dir.join("assets")).expect("write directory");
    let (tx, mut rx) = create_path_completion_channel();
    let mut manager = PathCompletionManager::new(tx);
    let query = format!("{}/a", dir.display());

    manager.search(request_key(SuggestionKind::Directory, query, 0));

    let PathCompletionEvent::SearchResult { suggestions, .. } =
        rx.recv().await.expect("path result");
    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].label, format!("{}/assets", dir.display()));
    assert!(matches!(
        suggestions[0].metadata,
        Some(SuggestionMeta::Path { is_directory: true })
    ));

    let _ = std::fs::remove_dir_all(dir);
}
