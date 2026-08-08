//! Tests for notification backends.

use super::NotificationBackend;
use super::iterm2_osc;
use super::kitty_body_osc;
use super::kitty_title_osc;
use super::osc9;
use super::wrap_for;
use crate::terminal_detect::Multiplexer;
use crate::terminal_detect::TerminalName;
use pretty_assertions::assert_eq;

#[test]
fn iterm2_osc_contains_title_and_message() {
    let seq = iterm2_osc("Claude", "Ready");
    assert!(seq.starts_with("\x1b]9;1;\n\n"));
    assert!(seq.contains("Claude:\nReady"));
    assert!(seq.ends_with("\x1b\\"));
}

#[test]
fn iterm2_osc_omits_title_prefix_when_empty() {
    let seq = iterm2_osc("", "Hello");
    assert!(seq.contains("\n\nHello"));
    assert!(!seq.contains(":\nHello"));
}

#[test]
fn kitty_frames_use_same_id() {
    let title = kitty_title_osc(42, "Claude");
    let body = kitty_body_osc(42, "Ready");
    assert!(title.contains("i=42"));
    assert!(body.contains("i=42"));
    assert!(title.contains("p=title"));
    assert!(body.contains("p=body"));
}

#[test]
fn wrap_outside_multiplexer_is_identity() {
    let seq = "\x1b]9;1;hi\x1b\\";
    assert_eq!(wrap_for(seq, None), seq);
    assert_eq!(wrap_for(seq, Some(Multiplexer::Zellij)), seq);
}

#[test]
fn wrap_inside_tmux_doubles_escapes_for_passthrough() {
    assert_eq!(
        wrap_for("\x1b]9;1;hi\x1b\\", Some(Multiplexer::Tmux)),
        "\x1bPtmux;\x1b\x1b\x1b]9;1;hi\x1b\x1b\\\x1b\\"
    );
}

#[test]
fn wrap_inside_screen_uses_plain_dcs() {
    assert_eq!(
        wrap_for("\x1b]9;1;hi\x1b\\", Some(Multiplexer::Screen)),
        "\x1bP\x1b]9;1;hi\x1b\\\x1b\\"
    );
}

#[test]
fn backend_for_terminal_maps_each_known_terminal() {
    for (name, expected) in [
        (TerminalName::Iterm2, NotificationBackend::ITerm2),
        (TerminalName::WezTerm, NotificationBackend::ITerm2),
        (TerminalName::Kitty, NotificationBackend::Kitty),
        (TerminalName::Ghostty, NotificationBackend::Ghostty),
        (
            TerminalName::AppleTerminal,
            NotificationBackend::TerminalBell,
        ),
        (TerminalName::Warp, NotificationBackend::Osc9),
        (TerminalName::Alacritty, NotificationBackend::Disabled),
        (TerminalName::Unknown, NotificationBackend::Disabled),
    ] {
        assert_eq!(
            NotificationBackend::for_terminal(name),
            expected,
            "{name:?}"
        );
    }
}

#[test]
fn disabled_backend_is_no_op() {
    let mut buf = Vec::new();
    NotificationBackend::Disabled
        .send(&mut buf, "t", "m")
        .unwrap();
    assert!(buf.is_empty());
}

#[test]
fn osc9_is_plain_form_with_bel_terminator() {
    assert_eq!(osc9("coco", "done"), "\x1b]9;coco: done\x07");
    assert_eq!(osc9("", "done"), "\x1b]9;done\x07");
}

#[test]
fn bell_backend_writes_bel() {
    let mut buf = Vec::new();
    NotificationBackend::TerminalBell
        .send(&mut buf, "t", "m")
        .unwrap();
    assert_eq!(buf, b"\x07");
}
