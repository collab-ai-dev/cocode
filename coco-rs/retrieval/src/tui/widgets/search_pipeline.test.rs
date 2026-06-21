use super::*;

#[test]
fn test_stage_ordering() {
    assert!(SearchStage::Preprocessing < SearchStage::Bm25Search);
    assert!(SearchStage::Complete > SearchStage::Reranking);
    assert!(SearchStage::Idle < SearchStage::Preprocessing);
}

#[test]
fn test_stage_icon() {
    // Completed stage
    assert_eq!(
        SearchPipeline::stage_icon(SearchStage::Bm25Search, SearchStage::Preprocessing),
        "✓"
    );
    // Current stage
    assert_eq!(
        SearchPipeline::stage_icon(SearchStage::Bm25Search, SearchStage::Bm25Search),
        "●"
    );
    // Future stage
    assert_eq!(
        SearchPipeline::stage_icon(SearchStage::Bm25Search, SearchStage::Fusion),
        "○"
    );
}

#[test]
fn test_truncate_str() {
    assert_eq!(truncate_str("short", 10), "short");
    // max_len=10, so we take first 7 chars ("a very ") and add "..."
    assert_eq!(truncate_str("a very long string", 10), "a very ...");
}
