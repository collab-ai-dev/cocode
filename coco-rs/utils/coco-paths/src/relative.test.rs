use super::*;
use pretty_assertions::assert_eq;

#[test]
fn normalize_lexical_removes_current_dir_and_parent_segments() {
    assert_eq!(
        normalize_lexical(Path::new("/repo/./src/../Cargo.toml")),
        PathBuf::from("/repo/Cargo.toml")
    );
}

#[test]
fn relative_posix_path_returns_relative_path_inside_root() {
    assert_eq!(
        relative_posix_path(Path::new("/repo"), Path::new("/repo/src/lib.rs")),
        Some("src/lib.rs".to_string())
    );
}

#[test]
fn relative_posix_path_returns_none_outside_root() {
    assert_eq!(
        relative_posix_path(Path::new("/repo"), Path::new("/tmp/src/lib.rs")),
        None
    );
}

#[test]
fn relative_posix_path_normalizes_parent_segments_before_matching() {
    assert_eq!(
        relative_posix_path(Path::new("/repo"), Path::new("/repo/src/../Cargo.toml")),
        Some("Cargo.toml".to_string())
    );
}
