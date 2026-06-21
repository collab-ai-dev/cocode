use super::*;

#[test]
fn detects_native_scrollback_without_zellij_env() {
    let compatibility = TerminalCompatibility::detect_with(|_| None);

    assert_eq!(compatibility, TerminalCompatibility::NativeScrollback);
    assert!(compatibility.native_scrollback_enabled());
    assert_eq!(compatibility.status_message(), None);
}

#[test]
fn disables_native_scrollback_when_zellij_env_is_present() {
    let compatibility =
        TerminalCompatibility::detect_with(|name| (name == "ZELLIJ").then(|| "1".to_string()));

    assert_eq!(
        compatibility,
        TerminalCompatibility::ZellijNativeScrollbackDisabled
    );
    assert!(!compatibility.native_scrollback_enabled());
    assert_eq!(
        compatibility.status_message(),
        Some("native scrollback disabled in Zellij")
    );
}

#[test]
fn disables_native_scrollback_when_zellij_session_name_is_present() {
    let compatibility = TerminalCompatibility::detect_with(|name| {
        (name == "ZELLIJ_SESSION_NAME").then(|| "dev".to_string())
    });

    assert_eq!(
        compatibility,
        TerminalCompatibility::ZellijNativeScrollbackDisabled
    );
}

#[test]
fn disables_native_scrollback_when_zellij_version_is_present() {
    let compatibility = TerminalCompatibility::detect_with(|name| {
        (name == "ZELLIJ_VERSION").then(|| "0.43.1".to_string())
    });

    assert_eq!(
        compatibility,
        TerminalCompatibility::ZellijNativeScrollbackDisabled
    );
}

#[test]
fn synchronized_update_defaults_true_and_reflects_probe() {
    // No probe yet → assume supported (BSU emitted, no fallback). This is the
    // only test that writes the process-global cache, so the default holds
    // until the explicit set below.
    assert!(synchronized_update_supported());

    set_synchronized_update_supported(false);
    assert_eq!(synchronized_update_probed(), Some(false));
    assert!(!synchronized_update_supported());

    set_synchronized_update_supported(true);
    assert!(synchronized_update_supported());
}
