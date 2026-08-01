use super::*;

#[test]
fn terminal_recovery_nudge_enforces_a_utf8_safe_hard_limit() {
    let fragment = TerminalRecoveryNudgeFragment::new(&"界".repeat(2_000));
    let rendered = fragment.render();

    assert_eq!(rendered.len(), 4_095);
    assert!(rendered.chars().all(|character| character == '界'));
}
