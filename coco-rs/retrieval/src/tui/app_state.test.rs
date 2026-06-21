use super::*;

#[test]
fn test_search_stage_display() {
    assert_eq!(SearchStage::Idle.to_string(), "Idle");
    assert_eq!(SearchStage::Bm25Search.to_string(), "BM25");
    assert_eq!(SearchStage::VectorSearch.to_string(), "Vector");
    assert_eq!(SearchStage::Complete.to_string(), "Complete");
}

#[test]
fn test_search_pipeline_state_reset() {
    let mut state = SearchPipelineState {
        stage: SearchStage::Complete,
        bm25_count: Some(10),
        ..Default::default()
    };
    state.reset();
    assert_eq!(state.stage, SearchStage::Idle);
    assert!(state.bm25_count.is_none());
}

#[test]
fn test_search_pipeline_state_start() {
    let mut state = SearchPipelineState::default();
    state.start();
    assert_eq!(state.stage, SearchStage::Preprocessing);
}
