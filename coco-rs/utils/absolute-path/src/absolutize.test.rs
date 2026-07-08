use std::path::Path;
use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::absolutize_from;

#[cfg(unix)]
#[test]
fn absolute_path_without_dots_is_unchanged() {
    assert_eq!(
        absolutize_from(Path::new("/path/to/123/456"), Path::new("/base")),
        PathBuf::from("/path/to/123/456")
    );
}

#[cfg(unix)]
#[test]
fn absolute_path_dots_are_removed() {
    assert_eq!(
        absolutize_from(Path::new("/path/to/./123/../456"), Path::new("/base")),
        PathBuf::from("/path/to/456")
    );
}

#[cfg(unix)]
#[test]
fn relative_path_without_dot_uses_base() {
    assert_eq!(
        absolutize_from(Path::new("path/to/123/456"), Path::new("/base")),
        PathBuf::from("/base/path/to/123/456")
    );
}

#[cfg(unix)]
#[test]
fn relative_path_with_current_dir_uses_base() {
    assert_eq!(
        absolutize_from(Path::new("./path/to/123/456"), Path::new("/base")),
        PathBuf::from("/base/path/to/123/456")
    );
}

#[cfg(unix)]
#[test]
fn relative_path_with_parent_dir_uses_base_parent() {
    assert_eq!(
        absolutize_from(Path::new("../path/to/123/456"), Path::new("/base/cwd")),
        PathBuf::from("/base/path/to/123/456")
    );
}

#[cfg(unix)]
#[test]
fn parent_dir_above_root_stays_at_root() {
    assert_eq!(
        absolutize_from(Path::new("../../path/to/123/456"), Path::new("/")),
        PathBuf::from("/path/to/123/456")
    );
}

#[cfg(unix)]
#[test]
fn empty_path_uses_base() {
    assert_eq!(
        absolutize_from(Path::new(""), Path::new("/base/cwd")),
        PathBuf::from("/base/cwd")
    );
}

#[cfg(windows)]
#[test]
fn windows_root_relative_path_uses_base_prefix() {
    assert_eq!(
        absolutize_from(Path::new(r"\path\to\file"), Path::new(r"C:\base\cwd")),
        PathBuf::from(r"C:\path\to\file")
    );
}

#[cfg(windows)]
#[test]
fn windows_drive_relative_path_uses_path_prefix_and_base_tail() {
    assert_eq!(
        absolutize_from(Path::new(r"D:path\to\file"), Path::new(r"C:\base\cwd")),
        PathBuf::from(r"D:\base\cwd\path\to\file")
    );
}
