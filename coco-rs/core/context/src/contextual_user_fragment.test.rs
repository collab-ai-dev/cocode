use super::*;

#[test]
fn terminal_recovery_nudge_enforces_a_utf8_safe_hard_limit() {
    let fragment = TerminalRecoveryNudgeFragment::new(&"界".repeat(2_000));
    let rendered = fragment.render();

    assert_eq!(rendered.len(), 4_095);
    assert!(rendered.chars().all(|character| character == '界'));
}

#[test]
fn skill_listing_enforces_aggregate_byte_and_token_limits() {
    let fragment = SkillListingFragment::new(&"界 alpha beta gamma ".repeat(2_000));
    let rendered = fragment.render();

    assert!(rendered.len() <= SkillListingFragment::MAX_BYTES);
    assert!(fragment.estimated_tokens() <= SkillListingFragment::MAX_TOKENS);
    assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
}
