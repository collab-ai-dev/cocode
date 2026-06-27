use super::*;

#[test]
fn test_default_policy() {
    let policy = IndexPolicy::default();
    assert_eq!(policy.chunk_threshold, 10_000);
    assert_eq!(policy.fts_chunk_threshold, 1_000);
    assert!(!policy.force_rebuild);
}

#[test]
fn test_never_policy() {
    let policy = IndexPolicy::never();
    assert_eq!(policy.chunk_threshold, 0);
    assert_eq!(policy.fts_chunk_threshold, 0);
}

#[test]
fn test_immediate_policy() {
    let policy = IndexPolicy::immediate();
    assert_eq!(policy.chunk_threshold, 1);
    assert_eq!(policy.fts_chunk_threshold, 1);
}

#[test]
fn test_policy_builder() {
    let policy = IndexPolicy::default()
        .with_vector_threshold(5_000)
        .with_fts_threshold(500)
        .with_force_rebuild();

    assert_eq!(policy.chunk_threshold, 5_000);
    assert_eq!(policy.fts_chunk_threshold, 500);
    assert!(policy.force_rebuild);
}

#[test]
fn test_index_status_needs_indexing() {
    let status = IndexStatus::default();
    assert!(!status.needs_indexing());

    let status = IndexStatus {
        vector_index_recommended: true,
        ..Default::default()
    };
    assert!(status.needs_indexing());
}

#[test]
fn test_config_to_policy() {
    let config = IndexPolicyConfig {
        chunk_threshold: 20_000,
        fts_chunk_threshold: 2_000,
    };
    let policy = IndexPolicy::from(&config);
    assert_eq!(policy.chunk_threshold, 20_000);
    assert_eq!(policy.fts_chunk_threshold, 2_000);
    assert!(!policy.force_rebuild);
}
