use super::*;
use pretty_assertions::assert_eq;

#[test]
fn drive_letter_to_posix() {
    assert_eq!(windows_path_to_posix_path(r"C:\Users\foo"), "/c/Users/foo");
    assert_eq!(
        windows_path_to_posix_path(r"D:\path\with\spaces are ok"),
        "/d/path/with/spaces are ok"
    );
}

#[test]
fn unc_to_posix() {
    assert_eq!(
        windows_path_to_posix_path(r"\\server\share"),
        "//server/share"
    );
}

#[test]
fn relative_flips_slashes() {
    assert_eq!(windows_path_to_posix_path(r"foo\bar\baz"), "foo/bar/baz");
}

#[test]
fn posix_drive_to_windows() {
    assert_eq!(posix_path_to_windows_path("/c/Users/foo"), r"C:\Users\foo");
    assert_eq!(posix_path_to_windows_path("/c"), r"C:\");
}

#[test]
fn cygdrive_to_windows() {
    assert_eq!(
        posix_path_to_windows_path("/cygdrive/c/Users/foo"),
        r"C:\Users\foo"
    );
    assert_eq!(posix_path_to_windows_path("/cygdrive/d"), r"D:\");
}

#[test]
fn unc_back_to_windows() {
    assert_eq!(
        posix_path_to_windows_path("//server/share"),
        r"\\server\share"
    );
}

#[test]
fn relative_flips_back() {
    assert_eq!(posix_path_to_windows_path("foo/bar"), r"foo\bar");
}
