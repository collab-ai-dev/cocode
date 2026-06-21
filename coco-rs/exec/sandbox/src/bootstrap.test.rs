use crate::config::SandboxSettings;

use super::*;

#[test]
fn test_disabled_by_settings() {
    let settings = SandboxSettings::default(); // enabled: false
    let result = check_enable_gates(&settings);
    assert!(!result.is_enabled());
    assert!(matches!(result, EnableCheckResult::DisabledBySettings));
}

#[test]
fn test_enabled_on_supported_platform() {
    if !(cfg!(target_os = "macos") || cfg!(target_os = "linux")) {
        return;
    }
    let settings = SandboxSettings::enabled();
    let result = check_enable_gates(&settings);
    // May still fail due to missing deps (e.g., bwrap not installed),
    // but should NOT be DisabledBySettings or DisabledByPlatform
    assert!(!matches!(result, EnableCheckResult::DisabledBySettings));
    assert!(!matches!(
        result,
        EnableCheckResult::DisabledByPlatform { .. }
    ));
}

#[test]
fn test_disabled_by_allowlist() {
    let mut settings = SandboxSettings::enabled();
    settings.enabled_platforms = vec![]; // Empty allowlist
    let result = check_enable_gates(&settings);
    if is_supported_platform() {
        assert!(matches!(result, EnableCheckResult::DisabledByAllowlist));
    }
}

#[test]
fn test_disabled_by_wrong_platform_in_list() {
    let mut settings = SandboxSettings::enabled();
    settings.enabled_platforms = vec!["freebsd".to_string()]; // Not our platform
    let result = check_enable_gates(&settings);
    if is_supported_platform() {
        assert!(matches!(result, EnableCheckResult::DisabledByAllowlist));
    }
}

#[test]
fn test_enable_check_result_is_enabled() {
    assert!(EnableCheckResult::Enabled.is_enabled());
    assert!(!EnableCheckResult::DisabledBySettings.is_enabled());
    assert!(
        !EnableCheckResult::DisabledByPlatform {
            reason: "test".to_string()
        }
        .is_enabled()
    );
    assert!(
        !EnableCheckResult::DisabledByMissingDeps {
            missing: vec!["bwrap".to_string()]
        }
        .is_enabled()
    );
    assert!(!EnableCheckResult::DisabledByAllowlist.is_enabled());
}
