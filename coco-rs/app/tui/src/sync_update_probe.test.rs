use super::*;

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
