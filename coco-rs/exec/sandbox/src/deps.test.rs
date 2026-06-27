use super::*;

#[test]
fn test_check_dependencies_returns_results() {
    let checks = check_dependencies();
    // Should have at least one check on any supported platform
    if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
        assert!(!checks.is_empty());
    }
}

#[test]
fn test_check_dependencies_macos() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let checks = check_dependencies();
    let sandbox_exec = checks.iter().find(|c| c.name == "sandbox-exec");
    assert!(sandbox_exec.is_some());
    let check = sandbox_exec.expect("sandbox-exec check");
    assert!(check.required);
    // sandbox-exec should always be available on macOS
    assert!(check.available);
}

#[test]
fn test_check_dependencies_linux() {
    if !cfg!(target_os = "linux") {
        return;
    }
    let checks = check_dependencies();
    let bwrap = checks.iter().find(|c| c.name == "bwrap");
    assert!(bwrap.is_some());
    let bwrap_check = bwrap.expect("bwrap check");
    assert!(bwrap_check.required);

    let socat = checks.iter().find(|c| c.name == "socat");
    assert!(socat.is_some());
    let socat_check = socat.expect("socat check");
    assert!(!socat_check.required); // Optional
}

#[test]
fn test_missing_required() {
    let missing = missing_required();
    // Just verify it returns a vec (actual content depends on platform)
    let _ = missing;
}

#[test]
fn test_all_required_available() {
    // On macOS, sandbox-exec should always be present
    if cfg!(target_os = "macos") {
        assert!(all_required_available());
    }
    // On other platforms, just check it doesn't panic
    let _ = all_required_available();
}
