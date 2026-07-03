use super::*;

#[test]
fn test_format_compact_summary_strips_analysis() {
    let raw = "<analysis>thinking...</analysis>\n<summary>\n1. Intent: fix bug\n</summary>";
    let result = format_compact_summary(raw);
    assert!(result.contains("Summary:"));
    assert!(result.contains("fix bug"));
    assert!(!result.contains("thinking"));
}

#[test]
fn test_format_compact_summary_no_tags() {
    let raw = "Just plain text summary";
    let result = format_compact_summary(raw);
    assert_eq!(result, "Just plain text summary");
}

#[test]
fn test_get_compact_prompt_includes_preamble() {
    let prompt = get_compact_prompt(None);
    assert!(prompt.starts_with(COMPACT_DIRECTIVE_OPEN));
    assert!(prompt.contains("CRITICAL: Respond with TEXT ONLY"));
    assert!(prompt.contains("Do NOT call any tools"));
    assert!(prompt.contains("Primary Request and Intent"));
}

#[test]
fn test_compact_prompts_wrapped_in_directive_tags() {
    use coco_messages::PartialCompactDirection;
    let prompts = [
        get_compact_prompt(None),
        get_partial_compact_prompt(None, PartialCompactDirection::Newest),
        get_partial_compact_prompt(None, PartialCompactDirection::Oldest),
    ];
    for prompt in &prompts {
        assert!(prompt.starts_with(COMPACT_DIRECTIVE_OPEN));
        assert!(prompt.ends_with(COMPACT_DIRECTIVE_CLOSE));
        // Exactly one occurrence each: the prose deliberately avoids the
        // literal sentinel strings so the scrub / anomaly detector can
        // treat any occurrence in *output* as an echo.
        assert_eq!(prompt.matches(COMPACT_DIRECTIVE_OPEN).count(), 1);
        assert_eq!(prompt.matches(COMPACT_DIRECTIVE_CLOSE).count(), 1);
    }
}

#[test]
fn test_custom_instructions_land_inside_directive() {
    let prompt = get_compact_prompt(Some("Focus on Rust code changes"));
    let instructions_at = prompt
        .find("Focus on Rust code changes")
        .expect("instructions present");
    let close_at = prompt.find(COMPACT_DIRECTIVE_CLOSE).expect("close tag");
    assert!(
        instructions_at < close_at,
        "custom instructions must sit inside the directive envelope"
    );
}

#[test]
fn test_section6_excludes_directive_in_all_templates() {
    use coco_messages::PartialCompactDirection;
    let prompts = [
        get_compact_prompt(None),
        get_partial_compact_prompt(None, PartialCompactDirection::Newest),
        get_partial_compact_prompt(None, PartialCompactDirection::Oldest),
    ];
    for prompt in &prompts {
        assert!(prompt.contains(SECTION6_EXCLUSION));
        assert!(prompt.contains(EXAMPLE_PLACEHOLDER_NOTE));
        // Regression: the old role-defined section 6 invited literal
        // models to list the summarization request itself.
        assert!(!prompt.contains("List ALL user messages that are not tool results"));
        // Placeholder substitution left no residue.
        assert!(!prompt.contains("{ANALYSIS_INSTRUCTION}"));
        assert!(!prompt.contains("{SECTION6_EXCLUSION}"));
        assert!(!prompt.contains("{PLACEHOLDER_NOTE}"));
    }
}

#[test]
fn test_format_compact_summary_scrubs_echoed_directive() {
    let raw = format!(
        "<summary>\n6. All user messages:\n    - real user message\n    - {COMPACT_DIRECTIVE_OPEN}CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.{COMPACT_DIRECTIVE_CLOSE}\n\n7. Pending Tasks:\n   - follow up on retries\n</summary>"
    );
    let result = format_compact_summary(&raw);
    assert!(result.contains("real user message"));
    assert!(result.contains("follow up on retries"));
    assert!(!result.contains(COMPACT_DIRECTIVE_OPEN));
    assert!(!result.contains("CRITICAL: Respond with TEXT ONLY"));
}

#[test]
fn test_format_compact_summary_scrub_runs_before_summary_extract() {
    // The echoed directive body contains the literal "<summary>" string
    // (NO_TOOLS text). Sitting ahead of the real block, it would derail
    // find()-based extraction unless the scrub runs first.
    let raw = format!(
        "<analysis>a</analysis>\n{COMPACT_DIRECTIVE_OPEN}an <analysis> block followed by a <summary> block{COMPACT_DIRECTIVE_CLOSE}\n<summary>the real body</summary>"
    );
    let result = format_compact_summary(&raw);
    assert_eq!(result, "Summary:\nthe real body");
}

#[test]
fn test_format_compact_summary_orphan_tag_stripped() {
    let raw = format!("before {COMPACT_DIRECTIVE_OPEN} after");
    let result = format_compact_summary(&raw);
    assert_eq!(result, "before  after");
}

#[test]
fn test_format_compact_summary_unwraps_mirror_wrapped_summary() {
    // Literal models sometimes mirror the envelope: the whole real
    // answer wrapped in the sentinel pair. The interior carries no
    // directive-body marker, so only the tags are dropped — the real
    // summary must survive, not scrub to empty (which would silently
    // replace the entire history with boilerplate).
    let raw = format!(
        "{COMPACT_DIRECTIVE_OPEN}\n<analysis>a</analysis>\n<summary>the real nine sections</summary>\n{COMPACT_DIRECTIVE_CLOSE}"
    );
    let result = format_compact_summary(&raw);
    assert_eq!(result, "Summary:\nthe real nine sections");
}

#[test]
fn test_format_compact_summary_pure_echo_falls_back_nonempty() {
    // Summary consisting solely of the echoed request: the span scrub
    // consumes everything, so the fallback must re-format the raw text
    // with only bare tags dropped (pre-fix behavior floor) rather than
    // returning an empty summary.
    let raw = get_compact_prompt(None);
    let result = format_compact_summary(&raw);
    assert!(!result.trim().is_empty());
    assert!(!result.contains(COMPACT_DIRECTIVE_OPEN));
    assert!(!result.contains(COMPACT_DIRECTIVE_CLOSE));
}

#[test]
fn test_format_compact_summary_keeps_content_between_quoted_sentinels() {
    // Dogfooding false positive: a session editing prompt.rs quotes both
    // sentinel constants in code snippets, with real summary content in
    // between. The interior carries no directive-body marker, so only
    // the tag strings may be dropped — the content must survive.
    let raw = format!(
        "<summary>\n3. Files and Code Sections:\n   - services/compact/src/prompt.rs\n      - pub const COMPACT_DIRECTIVE_OPEN: &str = \"{COMPACT_DIRECTIVE_OPEN}\";\n   real content between the quoted constants\n      - pub const COMPACT_DIRECTIVE_CLOSE: &str = \"{COMPACT_DIRECTIVE_CLOSE}\";\n\n8. Current Work:\n   editing the compact prompt module\n</summary>"
    );
    let result = format_compact_summary(&raw);
    assert!(result.contains("real content between the quoted constants"));
    assert!(result.contains("editing the compact prompt module"));
}

#[test]
fn test_format_compact_summary_deletes_echo_span_with_directive_body() {
    // A span whose interior carries directive-body text IS an echo and
    // must be deleted wholesale, not unwrapped.
    let raw = format!(
        "<summary>\n6. All user messages:\n    - real message\n{COMPACT_DIRECTIVE_OPEN}\nYou are performing a CONTEXT CHECKPOINT COMPACTION — an out-of-band maintenance procedure.\n{COMPACT_DIRECTIVE_CLOSE}\n7. Pending Tasks:\n   - None\n</summary>"
    );
    let result = format_compact_summary(&raw);
    assert!(result.contains("real message"));
    assert!(!result.contains("CONTEXT CHECKPOINT COMPACTION"));
}

#[test]
fn test_get_compact_prompt_with_custom() {
    let prompt = get_compact_prompt(Some("Focus on Rust code changes"));
    assert!(prompt.contains("Focus on Rust code changes"));
}

#[test]
fn test_user_summary_with_transcript() {
    let msg = get_compact_user_summary_message(
        "test summary",
        false,
        Some("/tmp/transcript.jsonl"),
        false,
    );
    assert!(msg.contains("read the full transcript at: /tmp/transcript.jsonl"));
}

#[test]
fn test_user_summary_suppress_followup() {
    let msg = get_compact_user_summary_message("test", true, None, false);
    assert!(msg.contains("Continue the conversation"));
    assert!(msg.contains("without asking"));
}

#[test]
fn test_user_summary_recent_preserved() {
    let msg = get_compact_user_summary_message("test", false, None, true);
    assert!(msg.contains("Recent messages are preserved verbatim."));
    let no_preserve = get_compact_user_summary_message("test", false, None, false);
    assert!(!no_preserve.contains("preserved verbatim"));
}

#[test]
fn test_partial_compact_prompt_directions_differ() {
    use coco_messages::PartialCompactDirection;
    let from_prompt = get_partial_compact_prompt(None, PartialCompactDirection::Newest);
    let up_to_prompt = get_partial_compact_prompt(None, PartialCompactDirection::Oldest);
    assert!(from_prompt.contains("Current Work"));
    assert!(up_to_prompt.contains("Work Completed"));
    assert!(up_to_prompt.contains("Context for Continuing Work"));
    assert!(!from_prompt.contains("Context for Continuing Work"));
}

#[test]
fn test_compact_prompt_includes_example_block() {
    let prompt = get_compact_prompt(None);
    assert!(prompt.contains("<example>"));
    assert!(prompt.contains("</example>"));
    assert!(prompt.contains("Compact Instructions"));
}
