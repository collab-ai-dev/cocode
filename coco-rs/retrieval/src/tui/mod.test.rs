use super::*;

#[test]
fn test_view_mode_exports() {
    // Ensure exports work
    let _ = ViewMode::Search;
    let _ = ViewMode::Index;
    let _ = ViewMode::RepoMap;
}
