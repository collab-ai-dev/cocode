use pretty_assertions::assert_eq;

use super::extract_osc11_payload;
use super::find_subslice;
use super::osc_reply_complete;

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
fn reply_complete_detects_both_terminators() {
    assert!(osc_reply_complete(b"\x1b]11;rgb:0/0/0\x07"));
    assert!(osc_reply_complete(b"\x1b]11;rgb:0/0/0\x1b\\"));
    assert!(!osc_reply_complete(b"\x1b]11;rgb:0/0/0"));
}

#[test]
fn find_subslice_basics() {
    assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
    assert_eq!(find_subslice(b"abcdef", b"xy"), None);
    assert_eq!(find_subslice(b"ab", b""), None);
    assert_eq!(find_subslice(b"a", b"abc"), None);
}
