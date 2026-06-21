use super::*;
use tempfile::TempDir;

#[test]
fn test_features_presets() {
    // NONE
    const { assert!(!RetrievalFeatures::NONE.code_search) };
    const { assert!(!RetrievalFeatures::NONE.vector_search) };
    const { assert!(!RetrievalFeatures::NONE.query_rewrite) };
    assert!(!RetrievalFeatures::NONE.has_search());

    // MINIMAL
    const { assert!(RetrievalFeatures::MINIMAL.code_search) };
    const { assert!(!RetrievalFeatures::MINIMAL.vector_search) };
    const { assert!(!RetrievalFeatures::MINIMAL.query_rewrite) };
    assert!(RetrievalFeatures::MINIMAL.has_search());

    // STANDARD
    const { assert!(RetrievalFeatures::STANDARD.code_search) };
    const { assert!(!RetrievalFeatures::STANDARD.vector_search) };
    const { assert!(RetrievalFeatures::STANDARD.query_rewrite) };
    assert!(RetrievalFeatures::STANDARD.has_search());

    // FULL
    const { assert!(RetrievalFeatures::FULL.code_search) };
    const { assert!(RetrievalFeatures::FULL.vector_search) };
    const { assert!(RetrievalFeatures::FULL.query_rewrite) };
    assert!(RetrievalFeatures::FULL.has_search());
}

#[test]
fn test_features_factory_methods() {
    // none() == NONE
    assert!(!RetrievalFeatures::none().has_search());

    // with_code_search() == MINIMAL
    let features = RetrievalFeatures::with_code_search();
    assert!(features.code_search);
    assert!(!features.vector_search);

    // all() == FULL
    let features = RetrievalFeatures::all();
    assert!(features.code_search);
    assert!(features.vector_search);
    assert!(features.query_rewrite);
}

#[tokio::test]
async fn test_context_creation() {
    let dir = TempDir::new().unwrap();
    let config = RetrievalConfig {
        data_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let ctx = RetrievalContext::new(config, RetrievalFeatures::MINIMAL, dir.path().to_path_buf())
        .await
        .unwrap();

    assert!(ctx.features().code_search);
    assert!(!ctx.features().vector_search);
    assert_eq!(ctx.workspace_root(), dir.path());
}
