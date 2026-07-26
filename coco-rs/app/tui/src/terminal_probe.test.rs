use coco_tui_ui::system_theme::SystemTheme;
use pretty_assertions::assert_eq;

use super::da1_reply_complete;
use super::extract_osc11_payload;
use super::find_subslice;
use super::has_kitty_keyboard_reply;
use super::parse_decrpm_2026;
use super::parse_probe_reply;

#[test]
fn extracts_rgb_payload_from_bel_terminated_reply() {
    // xterm-style OSC 11 reply: ESC ] 11 ; rgb:.../.../... BEL
    let reply = b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07";
    assert_eq!(
        extract_osc11_payload(reply).as_deref(),
        Some("rgb:1e1e/1e1e/1e1e")
    );
}

#[test]
fn extracts_payload_from_st_terminated_reply() {
    // ST (ESC \) terminator variant.
    let reply = b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\";
    assert_eq!(
        extract_osc11_payload(reply).as_deref(),
        Some("rgb:ffff/ffff/ffff")
    );
}

#[test]
fn extracts_payload_ignoring_leading_noise() {
    // A stray byte before the introducer (e.g. coalesced input) is skipped.
    let reply = b"x\x1b]11;rgb:0000/0000/0000\x07";
    assert_eq!(
        extract_osc11_payload(reply).as_deref(),
        Some("rgb:0000/0000/0000")
    );
}

#[test]
fn no_osc11_introducer_returns_none() {
    assert_eq!(extract_osc11_payload(b"random bytes"), None);
    assert_eq!(extract_osc11_payload(b"\x1b]10;rgb:0/0/0\x07"), None);
}

#[test]
fn find_subslice_basics() {
    assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
    assert_eq!(find_subslice(b"abcdef", b"xy"), None);
    assert_eq!(find_subslice(b"ab", b""), None);
    assert_eq!(find_subslice(b"a", b"abc"), None);
}

#[test]
fn parse_decrpm_2026_recognizes_supported_modes() {
    // Ps 1/2/3/4 all mean the mode is recognized → synchronized output works.
    assert_eq!(parse_decrpm_2026(b"\x1b[?2026;1$y"), Some(true));
    assert_eq!(parse_decrpm_2026(b"\x1b[?2026;2$y"), Some(true));
    assert_eq!(parse_decrpm_2026(b"\x1b[?2026;3$y"), Some(true));
    assert_eq!(parse_decrpm_2026(b"\x1b[?2026;4$y"), Some(true));
}

#[test]
fn parse_decrpm_2026_treats_zero_as_unsupported() {
    assert_eq!(parse_decrpm_2026(b"\x1b[?2026;0$y"), Some(false));
}

#[test]
fn parse_decrpm_2026_absent_for_da1_only_reply() {
    // DA1 answered but no DECRPM block: no mode-2026 info to parse.
    assert_eq!(parse_decrpm_2026(b"\x1b[?62;1;6c"), None);
}

#[test]
fn parse_decrpm_2026_reads_block_preceding_da1() {
    // Real ordering: DECRPM reply, then the DA1 fence.
    assert_eq!(
        parse_decrpm_2026(b"\x1b[?2026;2$y\x1b[?62;1;6c"),
        Some(true)
    );
}

#[test]
fn da1_reply_complete_waits_for_terminator() {
    // The DECRPM reply alone (ends in `y`) is not the fence.
    assert!(!da1_reply_complete(b"\x1b[?2026;1$y"));
    assert!(da1_reply_complete(b"\x1b[?2026;1$y\x1b[?62;c"));
}

#[test]
fn da1_fence_is_not_confused_by_hex_c_in_background_reply() {
    // The regression the merged probe introduces: a grey background answers
    // `rgb:cccc/…`, and scanning for a bare `c` would end the read before the
    // DECRPM and kitty answers arrived.
    let osc_only = b"\x1b]11;rgb:cccc/cccc/cccc\x07";
    assert!(!da1_reply_complete(osc_only));
    assert!(da1_reply_complete(
        b"\x1b]11;rgb:cccc/cccc/cccc\x07\x1b[?62;c"
    ));
}

#[test]
fn kitty_reply_detected_only_when_terminal_answers() {
    assert!(has_kitty_keyboard_reply(b"\x1b[?0u\x1b[?62;c"));
    assert!(has_kitty_keyboard_reply(b"\x1b[?11u"));
    // DA1 alone: the query was ignored, so the protocol is unsupported.
    assert!(!has_kitty_keyboard_reply(b"\x1b[?62;1;6c"));
}

#[test]
fn full_reply_yields_all_three_answers() {
    // A kitty-class terminal: dark background, mode 2026, keyboard protocol.
    let reply = b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07\x1b[?2026;2$y\x1b[?1u\x1b[?62;c";
    let probe = parse_probe_reply(reply);
    assert_eq!(probe.background, Some(SystemTheme::Dark));
    assert_eq!(probe.synchronized_update, Some(true));
    assert_eq!(probe.keyboard_enhancement, Some(true));
}

#[test]
fn da1_only_reply_means_no_support_but_no_background_claim() {
    // An old xterm answers the fence and nothing else. Unanswered queries are
    // negative capability answers; the unanswered background is NOT a claim
    // that the terminal is dark.
    let probe = parse_probe_reply(b"\x1b[?62;1;6c");
    assert_eq!(probe.background, None);
    assert_eq!(probe.synchronized_update, Some(false));
    assert_eq!(probe.keyboard_enhancement, Some(false));
}

#[test]
fn light_background_is_classified_from_the_osc_reply() {
    let probe = parse_probe_reply(b"\x1b]11;rgb:ffff/ffff/ffff\x07\x1b[?62;c");
    assert_eq!(probe.background, Some(SystemTheme::Light));
}
