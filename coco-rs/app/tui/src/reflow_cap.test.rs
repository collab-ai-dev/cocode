use super::*;
use pretty_assertions::assert_eq;

#[test]
fn test_resolve_max_reflow_rows_auto_uses_terminal_scrollback_defaults() {
    for (terminal, expected) in [
        (TerminalName::VsCode, VSCODE_MAX_ROWS),
        (TerminalName::WindowsTerminal, WINDOWS_TERMINAL_MAX_ROWS),
        (TerminalName::WezTerm, WEZTERM_MAX_ROWS),
        (TerminalName::Alacritty, ALACRITTY_MAX_ROWS),
        (TerminalName::Dumb, DUMB_MAX_ROWS),
        (TerminalName::Ghostty, FALLBACK_MAX_ROWS),
        (TerminalName::Unknown, FALLBACK_MAX_ROWS),
    ] {
        assert_eq!(
            resolve_max_reflow_rows(ReflowMaxRows::Auto, terminal).get(),
            expected,
            "{terminal:?}"
        );
    }
}

#[test]
fn test_resolve_max_reflow_rows_explicit_limit_overrides_auto() {
    assert_eq!(
        resolve_max_reflow_rows(ReflowMaxRows::Rows(42), TerminalName::VsCode).get(),
        42
    );
}

#[test]
fn test_resolve_max_reflow_rows_unlimited_never_caps() {
    assert_eq!(
        resolve_max_reflow_rows(ReflowMaxRows::Unlimited, TerminalName::VsCode),
        MaxReflowRows::UNLIMITED
    );
}

/// A configured 0 would rebuild an empty transcript, which reads as a broken
/// terminal rather than as a preference.
#[test]
fn test_resolve_max_reflow_rows_zero_is_treated_as_unlimited() {
    assert_eq!(
        resolve_max_reflow_rows(ReflowMaxRows::Rows(0), TerminalName::VsCode),
        MaxReflowRows::UNLIMITED
    );
}

/// The default setting must not change behavior for terminals coco cannot
/// identify — the historical flat cap stays put.
#[test]
fn test_resolve_max_reflow_rows_default_setting_keeps_legacy_cap_for_unknown() {
    assert_eq!(
        resolve_max_reflow_rows(ReflowMaxRows::default(), TerminalName::Unknown).get(),
        9_000
    );
}

/// A derived `Default` on a bare `usize` would mean "replay zero rows"; the
/// newtype exists to make the safe cap the default instead.
#[test]
fn test_max_reflow_rows_default_is_the_fallback_cap() {
    assert_eq!(MaxReflowRows::default().get(), FALLBACK_MAX_ROWS);
}
