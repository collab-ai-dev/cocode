use super::*;

#[test]
fn test_version_exists() {
    // Just verify we can access the version
    assert!(!VERSION.is_empty());
}
