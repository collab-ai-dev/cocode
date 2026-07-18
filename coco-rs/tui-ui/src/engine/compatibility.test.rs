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
fn no_out_of_band_repainter_on_a_plain_terminal() {
    assert!(!repaints_pane_out_of_band_with(|_| None));
}

#[test]
fn detects_out_of_band_repainters() {
    // Each of these can paint over coco's pane while it is unfocused, which the
    // cell diff cannot see — the focus heal keys on this.
    for name in ["TMUX", "STY", "ZELLIJ", "ZELLIJ_SESSION_NAME"] {
        assert!(
            repaints_pane_out_of_band_with(|probed| (probed == name).then(|| "1".to_string())),
            "{name} must mark the pane as repainted out of band"
        );
    }
}

#[test]
fn an_empty_multiplexer_env_var_is_not_a_multiplexer() {
    // Exported-but-empty is the shell's doing, not a live multiplexer; treating
    // it as one would force a full repaint on every focus-gain forever.
    assert!(!repaints_pane_out_of_band_with(
        |name| (name == "TMUX").then(String::new)
    ));
}

#[test]
fn osc8_support_uses_a_conservative_terminal_allowlist() {
    for program in ["iTerm.app", "WezTerm", "kitty", "ghostty"] {
        assert!(osc8_hyperlinks_supported_with(|name| {
            (name == "TERM_PROGRAM").then(|| program.to_string())
        }));
    }
    assert!(osc8_hyperlinks_supported_with(|name| {
        (name == "VTE_VERSION").then(|| "5000".to_string())
    }));
    assert!(!osc8_hyperlinks_supported_with(|_| None));
}

#[test]
fn osc8_support_rejects_unknown_or_old_multiplexers() {
    fn tmux_env(version: &str, name: &str) -> Option<String> {
        match name {
            "TMUX" => Some("/tmp/tmux,1,0".to_string()),
            "TERM_PROGRAM" => Some("tmux".to_string()),
            "TERM_PROGRAM_VERSION" => Some(version.to_string()),
            _ => None,
        }
    }
    assert!(!osc8_hyperlinks_supported_with(|name| {
        tmux_env("3.3", name)
    }));
    assert!(osc8_hyperlinks_supported_with(|name| {
        tmux_env("3.4", name)
    }));
    assert!(!osc8_hyperlinks_supported_with(|name| {
        match name {
            "STY" => Some("123.screen".to_string()),
            "TERM_PROGRAM" => Some("iTerm.app".to_string()),
            _ => None,
        }
    }));
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
