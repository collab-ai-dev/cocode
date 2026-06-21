use super::*;

#[test]
fn test_format_size() {
    let mut state = StatsPanelState::new();

    state.index_size_bytes = 512;
    assert_eq!(state.format_size(), "512 B");

    state.index_size_bytes = 2048;
    assert_eq!(state.format_size(), "2.0 KB");

    state.index_size_bytes = 5 * 1024 * 1024;
    assert_eq!(state.format_size(), "5.0 MB");

    state.index_size_bytes = 2 * 1024 * 1024 * 1024;
    assert_eq!(state.format_size(), "2.0 GB");
}

#[test]
fn test_set_stats() {
    let mut state = StatsPanelState::new();
    state.set_stats(100, 500, 200);

    assert_eq!(state.file_count, 100);
    assert_eq!(state.chunk_count, 500);
    assert_eq!(state.symbol_count, 200);
    assert!(state.is_ready);
}
