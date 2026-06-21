use super::*;

#[test]
fn test_common_patterns_not_empty() {
    assert!(!COMMON_IGNORE_PATTERNS.is_empty());
}

#[test]
fn test_binary_patterns_not_empty() {
    assert!(!BINARY_FILE_PATTERNS.is_empty());
}

#[test]
fn test_get_all_default_excludes() {
    let all = get_all_default_excludes();
    let expected_len = COMMON_IGNORE_PATTERNS.len()
        + BINARY_FILE_PATTERNS.len()
        + COMMON_DIRECTORY_EXCLUDES.len()
        + SYSTEM_FILE_EXCLUDES.len();
    assert_eq!(all.len(), expected_len);
}
