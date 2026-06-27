use super::*;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn rejects_empty() {
    assert_eq!(validate_memory_path(""), Err(PathValidationError::Empty));
}

#[test]
fn rejects_null_bytes() {
    assert_eq!(
        validate_memory_path("a\0b.md"),
        Err(PathValidationError::NullByte)
    );
}

#[test]
fn rejects_unc() {
    assert_eq!(
        validate_memory_path("\\\\server\\share\\foo.md"),
        Err(PathValidationError::UncPath)
    );
}

#[test]
fn rejects_absolute() {
    assert_eq!(
        validate_memory_path("/etc/passwd"),
        Err(PathValidationError::AbsolutePath)
    );
}

#[test]
fn rejects_drive_root() {
    assert_eq!(
        validate_memory_path("C:foo.md"),
        Err(PathValidationError::DriveRoot)
    );
}

#[test]
fn rejects_bare_tilde() {
    assert_eq!(validate_memory_path("~"), Err(PathValidationError::Tilde));
}

#[test]
fn rejects_literal_traversal() {
    assert_eq!(
        validate_memory_path("../etc/passwd"),
        Err(PathValidationError::Traversal)
    );
    assert_eq!(
        validate_memory_path("a/../b"),
        Err(PathValidationError::Traversal)
    );
}

#[test]
fn rejects_url_encoded_traversal() {
    assert_eq!(
        validate_memory_path("%2e%2e/x"),
        Err(PathValidationError::Traversal)
    );
}

#[test]
fn rejects_fullwidth_unicode() {
    assert_eq!(
        validate_memory_path("\u{FF0E}\u{FF0E}/x"),
        Err(PathValidationError::UnicodeTraversal)
    );
}

#[test]
fn accepts_normal_relative_paths() {
    assert!(validate_memory_path("user_role.md").is_ok());
    assert!(validate_memory_path("nested/file.md").is_ok());
}

#[test]
fn within_memory_dir_predicate() {
    let mem = Path::new("/m");
    assert!(is_within_memory_dir(Path::new("/m/x.md"), mem));
    assert!(is_within_memory_dir(Path::new("/m/sub/y.md"), mem));
    assert!(!is_within_memory_dir(Path::new("/etc/passwd"), mem));
}

#[test]
fn validate_resolved_path_rejects_escape() {
    let mem = Path::new("/m");
    assert!(validate_resolved_path(Path::new("../etc"), mem).is_err());
}
