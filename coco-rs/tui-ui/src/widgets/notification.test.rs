//! Tests for notification backends.

use super::NotificationBackend;
use super::iterm2_osc;
use super::kitty_body_osc;
use super::kitty_title_osc;
use super::wrap;

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
    // SAFETY in tests: we expect TMUX/STY to be unset in the test runner env.
    if std::env::var_os("TMUX").is_none() && std::env::var_os("STY").is_none() {
        let seq = "\x1b]9;1;hi\x1b\\";
        assert_eq!(wrap(seq), seq);
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
fn bell_backend_writes_bel() {
    let mut buf = Vec::new();
    NotificationBackend::TerminalBell
        .send(&mut buf, "t", "m")
        .unwrap();
    assert_eq!(buf, b"\x07");
}
