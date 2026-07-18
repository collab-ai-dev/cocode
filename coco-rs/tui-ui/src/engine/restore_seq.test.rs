use super::*;

#[test]
fn restore_seq_is_the_concatenation_of_the_ledger() {
    let joined: Vec<u8> = seq::ORDERED
        .iter()
        .flat_map(|s| s.iter().copied())
        .collect();
    assert_eq!(RESTORE_SEQ, joined.as_slice());
}

#[test]
fn end_sync_update_leads() {
    // Invariant 1: `?2026l` must be the very first thing emitted so a relaying
    // multiplexer closes the open synchronized-update window before anything
    // else.
    assert_eq!(seq::ORDERED.first(), Some(&seq::END_SYNC_UPDATE));
    assert!(RESTORE_SEQ.starts_with(seq::END_SYNC_UPDATE));
}

#[test]
fn show_cursor_is_last() {
    // Invariant 3: the cursor is made visible only after every mode is reset.
    assert_eq!(seq::ORDERED.last(), Some(&seq::SHOW_CURSOR));
    assert!(RESTORE_SEQ.ends_with(seq::SHOW_CURSOR));
}

#[test]
fn kitty_pop_precedes_alternate_screen_leave() {
    // Invariant 2: `?1049l` stays out of the constant (conditional), but if a
    // caller appends it, the kitty pop must already have happened. We encode the
    // weaker, checkable form: the kitty pop is present and `?1049` never is.
    let kitty = find_subsequence(RESTORE_SEQ, seq::KITTY_POP_ALL).expect("kitty pop present");
    assert!(
        find_subsequence(RESTORE_SEQ, b"\x1b[?1049").is_none(),
        "alt-screen leave must not be baked into the unconditional restore"
    );
    // Kitty pop comes before the cursor show (a stand-in for "before any later
    // screen-scoped teardown").
    let show = find_subsequence(RESTORE_SEQ, seq::SHOW_CURSOR).expect("show cursor present");
    assert!(kitty < show);
}

#[test]
fn every_ledger_sequence_appears_once_in_order() {
    let mut cursor = 0usize;
    for part in seq::ORDERED {
        let at = find_subsequence(&RESTORE_SEQ[cursor..], part)
            .map(|i| i + cursor)
            .unwrap_or_else(|| panic!("missing ledger sequence: {part:?}"));
        assert!(at >= cursor, "ledger sequences out of order at {part:?}");
        cursor = at + part.len();
        // No second occurrence anywhere.
        assert!(
            find_subsequence(&RESTORE_SEQ[cursor..], part).is_none(),
            "duplicate ledger sequence: {part:?}"
        );
    }
}

#[test]
fn every_escape_is_well_formed() {
    // Each `\x1b[` (CSI) opener must be followed by a final byte in 0x40..=0x7e
    // before the next escape — no truncated sequence a signal handler could
    // half-write.
    let mut i = 0;
    let mut escapes = 0;
    while i < RESTORE_SEQ.len() {
        if RESTORE_SEQ[i] == 0x1b {
            assert_eq!(RESTORE_SEQ.get(i + 1), Some(&b'['), "CSI opener at {i}");
            let mut j = i + 2;
            while j < RESTORE_SEQ.len() && !(0x40..=0x7e).contains(&RESTORE_SEQ[j]) {
                j += 1;
            }
            assert!(j < RESTORE_SEQ.len(), "unterminated escape at {i}");
            escapes += 1;
            i = j + 1;
        } else {
            i += 1;
        }
    }
    assert_eq!(escapes, seq::ORDERED.len(), "one escape per ledger entry");
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
