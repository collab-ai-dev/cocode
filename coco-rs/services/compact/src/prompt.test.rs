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
    assert!(prompt.starts_with("CRITICAL:"));
    assert!(prompt.contains("Do NOT call any tools"));
    assert!(prompt.contains("Primary Request and Intent"));
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
