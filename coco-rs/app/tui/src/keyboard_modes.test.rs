use crossterm::Command;
use pretty_assertions::assert_eq;

use super::DisableModifyOtherKeys;
use super::EnableModifyOtherKeys;
use super::ResetKeyboardEnhancementFlags;
use super::keyboard_enhancement_disabled_for;
use super::tmux_session_detected;
use super::tmux_should_enable_modify_other_keys_for;
use super::vscode_terminal_detected;

fn ansi_for(command: impl Command) -> String {
    let mut out = String::new();
    command.write_ansi(&mut out).expect("write to String");
    out
}

#[test]
fn test_keyboard_enhancement_auto_disables_for_vscode_in_wsl() {
    assert!(keyboard_enhancement_disabled_for(
        /*override_env*/ None, /*is_wsl*/ true, /*is_vscode_terminal*/ true
    ));
}

#[test]
fn test_keyboard_enhancement_auto_disable_requires_wsl_and_vscode() {
    assert!(!keyboard_enhancement_disabled_for(
        /*override_env*/ None, /*is_wsl*/ true, /*is_vscode_terminal*/ false
    ));
    assert!(!keyboard_enhancement_disabled_for(
        /*override_env*/ None, /*is_wsl*/ false, /*is_vscode_terminal*/ true
    ));
}

#[test]
fn test_keyboard_enhancement_env_override_beats_auto_detection() {
    assert!(!keyboard_enhancement_disabled_for(
        Some(false),
        /*is_wsl*/ true,
        /*is_vscode_terminal*/ true
    ));
    assert!(keyboard_enhancement_disabled_for(
        Some(true),
        /*is_wsl*/ false,
        /*is_vscode_terminal*/ false
    ));
}

#[test]
fn test_vscode_terminal_detection_uses_linux_and_windows_term_program() {
    assert!(vscode_terminal_detected(
        Some("vscode"),
        /*windows_term_program*/ None
    ));
    assert!(vscode_terminal_detected(
        /*linux_term_program*/ None,
        Some("vscode")
    ));
    assert!(!vscode_terminal_detected(
        /*linux_term_program*/ None,
        Some("WindowsTerminal")
    ));
    assert!(!vscode_terminal_detected(
        /*linux_term_program*/ None, /*windows_term_program*/ None
    ));
}

#[test]
fn test_tmux_session_detection_accepts_tmux_or_tmux_pane() {
    assert!(tmux_session_detected(
        Some("/tmp/tmux-501/default,1,0"),
        /*tmux_pane*/ None
    ));
    assert!(tmux_session_detected(/*tmux*/ None, Some("%0")));
    assert!(!tmux_session_detected(
        /*tmux*/ None, /*tmux_pane*/ None
    ));
}

#[test]
fn test_tmux_modify_other_keys_only_requests_confirmed_csi_u_format() {
    assert!(tmux_should_enable_modify_other_keys_for(
        /*running_in_tmux_session*/ true,
        Some("csi-u")
    ));
    assert!(!tmux_should_enable_modify_other_keys_for(
        /*running_in_tmux_session*/ true, /*extended_keys_format*/ None
    ));
    assert!(!tmux_should_enable_modify_other_keys_for(
        /*running_in_tmux_session*/ true,
        Some("xterm")
    ));
    assert!(!tmux_should_enable_modify_other_keys_for(
        /*running_in_tmux_session*/ false,
        Some("csi-u")
    ));
}

#[test]
fn test_reset_keyboard_enhancement_flags_clears_all_pushed_levels() {
    assert_eq!(ansi_for(ResetKeyboardEnhancementFlags), "\x1b[<u");
}

#[test]
fn test_modify_other_keys_sequences() {
    assert_eq!(ansi_for(EnableModifyOtherKeys), "\x1b[>4;2m");
    assert_eq!(ansi_for(DisableModifyOtherKeys), "\x1b[>4;0m");
}
