use super::*;

#[test]
fn test_build_powershell_args_shape() {
    let args = build_powershell_args("Write-Host hi");
    assert_eq!(
        args,
        vec![
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            "Write-Host hi".to_string(),
        ]
    );
}

#[test]
fn test_build_powershell_args_preserves_quotes() {
    let args = build_powershell_args("Write-Host 'hello world'");
    assert_eq!(args[3], "Write-Host 'hello world'");
}

#[cfg(target_os = "windows")]
#[test]
fn test_windows_path_to_posix_drive_letter() {
    assert_eq!(windows_path_to_posix_path(r"C:\Users\foo"), "/c/Users/foo");
    assert_eq!(
        windows_path_to_posix_path(r"D:\Projects\bar"),
        "/d/Projects/bar"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn test_windows_path_to_posix_already_posix_passthrough() {
    assert_eq!(windows_path_to_posix_path("/c/Users/foo"), "/c/Users/foo");
    assert_eq!(windows_path_to_posix_path("relative/path"), "relative/path");
}

#[cfg(target_os = "windows")]
#[test]
fn test_windows_path_to_posix_lowercase_drive() {
    assert_eq!(
        windows_path_to_posix_path(r"c:\foo"),
        "/c/foo",
        "drive letter normalized to lowercase",
    );
}

#[cfg(not(target_os = "windows"))]
#[test]
fn test_windows_path_passthrough_on_non_windows() {
    // On non-Windows we don't touch the path — caller is using POSIX
    // already.
    assert_eq!(windows_path_to_posix_path(r"C:\Users\foo"), r"C:\Users\foo");
    assert_eq!(windows_path_to_posix_path("/usr/bin"), "/usr/bin");
}

#[cfg(not(target_os = "windows"))]
#[test]
fn test_git_bash_returns_none_on_non_windows() {
    assert!(find_git_bash_path().is_none());
}

#[tokio::test]
async fn test_powershell_path_returns_consistent_value() {
    // Whatever value we get on the first call, subsequent calls
    // must return the same (cached) value.
    let first = cached_powershell_path().await;
    let second = cached_powershell_path().await;
    assert_eq!(first, second);
}
