use super::*;

#[test]
fn test_repomap_view_creation() {
    let repomap = RepoMapState::default();
    let _view = RepoMapView::new(&repomap);
}

#[test]
fn test_repomap_view_with_content() {
    let repomap = RepoMapState {
        max_tokens: 1024,
        content: Some("test content".to_string()),
        tokens: 100,
        files: 5,
        duration_ms: 50,
        generating: false,
        scroll_offset: 0,
    };
    let _view = RepoMapView::new(&repomap);
}
