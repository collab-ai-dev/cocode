use super::*;
use pretty_assertions::assert_eq;

/// Build a `get_env` seam from a fixed `(name, value)` table.
fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + use<'a> {
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_string())
    }
}

fn name_of(pairs: &[(&str, &str)]) -> TerminalName {
    terminal_info_with(env(pairs)).name
}

fn multiplexer_of(pairs: &[(&str, &str)]) -> Option<Multiplexer> {
    terminal_info_with(env(pairs)).multiplexer()
}

#[test]
fn test_detect_name_empty_env_is_unknown() {
    assert_eq!(terminal_info_with(|_| None).name, TerminalName::Unknown);
}

#[test]
fn test_detect_name_dumb_term_wins_over_term_program() {
    assert_eq!(
        name_of(&[("TERM", "dumb"), ("TERM_PROGRAM", "iTerm.app")]),
        TerminalName::Dumb
    );
}

#[test]
fn test_detect_name_term_program_is_case_insensitive() {
    for (value, expected) in [
        ("iTerm.app", TerminalName::Iterm2),
        ("ITERM.APP", TerminalName::Iterm2),
        ("Apple_Terminal", TerminalName::AppleTerminal),
        ("ghostty", TerminalName::Ghostty),
        ("WezTerm", TerminalName::WezTerm),
        ("vscode", TerminalName::VsCode),
        ("WarpTerminal", TerminalName::Warp),
        ("kitty", TerminalName::Kitty),
        ("alacritty", TerminalName::Alacritty),
    ] {
        assert_eq!(name_of(&[("TERM_PROGRAM", value)]), expected, "{value}");
    }
}

#[test]
fn test_detect_name_lc_terminal_identifies_iterm_over_ssh() {
    assert_eq!(name_of(&[("LC_TERMINAL", "iTerm2")]), TerminalName::Iterm2);
}

#[test]
fn test_detect_name_falls_back_to_emulator_markers() {
    for (var, expected) in [
        ("WT_SESSION", TerminalName::WindowsTerminal),
        ("KITTY_WINDOW_ID", TerminalName::Kitty),
        ("WEZTERM_PANE", TerminalName::WezTerm),
        ("GHOSTTY_BIN_DIR", TerminalName::Ghostty),
        ("ALACRITTY_SOCKET", TerminalName::Alacritty),
        ("KONSOLE_VERSION", TerminalName::Konsole),
        ("GNOME_TERMINAL_SCREEN", TerminalName::GnomeTerminal),
    ] {
        assert_eq!(name_of(&[(var, "1")]), expected, "{var}");
    }
}

/// Under tmux, `TERM_PROGRAM` names tmux rather than the emulator; detection
/// must fall through to the markers tmux passes along instead of guessing.
#[test]
fn test_detect_name_tmux_term_program_falls_through_to_markers() {
    assert_eq!(
        name_of(&[
            ("TMUX", "/tmp/tmux-1000/default,123,0"),
            ("TERM_PROGRAM", "tmux"),
            ("KITTY_WINDOW_ID", "1"),
        ]),
        TerminalName::Kitty
    );
}

#[test]
fn test_detect_name_term_substring_is_last_resort() {
    assert_eq!(name_of(&[("TERM", "xterm-kitty")]), TerminalName::Kitty);
    assert_eq!(name_of(&[("TERM", "xterm-ghostty")]), TerminalName::Ghostty);
    assert_eq!(name_of(&[("TERM", "alacritty")]), TerminalName::Alacritty);
}

#[test]
fn test_detect_name_vte_version_is_the_generic_backend() {
    assert_eq!(name_of(&[("VTE_VERSION", "6800")]), TerminalName::Vte);
}

/// A more specific marker must beat the generic VTE backend.
#[test]
fn test_detect_name_specific_marker_beats_vte_version() {
    assert_eq!(
        name_of(&[("VTE_VERSION", "6800"), ("KONSOLE_VERSION", "220400")]),
        TerminalName::Konsole
    );
}

#[test]
fn test_detect_multiplexer_recognizes_each_multiplexer() {
    assert_eq!(
        multiplexer_of(&[("TMUX", "/tmp/x")]),
        Some(Multiplexer::Tmux)
    );
    assert_eq!(
        multiplexer_of(&[("TMUX_PANE", "%0")]),
        Some(Multiplexer::Tmux)
    );
    assert_eq!(
        multiplexer_of(&[("STY", "1.pts-0")]),
        Some(Multiplexer::Screen)
    );
    assert_eq!(
        multiplexer_of(&[("ZELLIJ", "0")]),
        Some(Multiplexer::Zellij)
    );
    assert_eq!(
        multiplexer_of(&[("ZELLIJ_SESSION_NAME", "dev")]),
        Some(Multiplexer::Zellij)
    );
}

#[test]
fn test_detect_multiplexer_absent_outside_a_multiplexer() {
    assert_eq!(multiplexer_of(&[("TERM_PROGRAM", "ghostty")]), None);
}

#[test]
fn test_detect_multiplexer_and_name_are_independent() {
    let info = terminal_info_with(env(&[("ZELLIJ", "0"), ("TERM_PROGRAM", "ghostty")]));
    assert_eq!(info.name, TerminalName::Ghostty);
    assert_eq!(info.multiplexer(), Some(Multiplexer::Zellij));
}

/// An empty variable is not a signal: terminals set these to identify
/// themselves, so `TMUX=""` must not read as "inside tmux".
#[test]
fn test_empty_values_are_not_signals() {
    let info = terminal_info_with(env(&[("TMUX", ""), ("TERM_PROGRAM", "")]));
    assert_eq!(info.name, TerminalName::Unknown);
    assert_eq!(info.multiplexer(), None);
    assert!(!info.in_multiplexer());
}

/// tmux inside Zellij: the passthrough dialect is tmux's, but Zellij is still
/// in the chain and still governs repaint/scrollback decisions.
#[test]
fn test_nested_multiplexers_report_innermost_and_membership() {
    let info = terminal_info_with(env(&[("TMUX", "/tmp/x"), ("ZELLIJ", "0")]));
    assert_eq!(info.multiplexer(), Some(Multiplexer::Tmux));
    assert!(info.is_inside(Multiplexer::Tmux));
    assert!(info.is_inside(Multiplexer::Zellij));
    assert!(!info.is_inside(Multiplexer::Screen));
    assert!(info.in_multiplexer());
}
