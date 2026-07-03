use pretty_assertions::assert_eq;

use super::*;
use crate::prompt::get_compact_prompt;

const CLEAN_SUMMARY: &str = "<analysis>done</analysis>\n<summary>\n1. Primary Request and Intent:\n   Fix the flaky retry loop in the downloader.\n\n2. Key Technical Concepts:\n   - tokio backoff\n\n3. Files and Code Sections:\n   - src/download.rs\n      - retry loop rewritten with jitter\n\n4. Errors and fixes:\n   - None\n\n5. Problem Solving:\n   Root-caused the double-retry to a shared counter.\n\n6. All user messages:\n    - fix the downloader retries\n    - add jitter please\n\n7. Pending Tasks:\n   - None\n\n8. Current Work:\n   Finishing the jitter unit test.\n\n9. Optional Next Step:\n   Run the full test suite.\n</summary>";

#[test]
fn test_detect_anomalies_clean_summary_is_clean() {
    let request = get_compact_prompt(None);
    let anomalies = detect_compact_summary_anomalies(CLEAN_SUMMARY, &request);
    assert_eq!(anomalies, vec![]);
}

#[test]
fn test_detect_anomalies_flags_full_prompt_echo() {
    let request = get_compact_prompt(None);
    // The observed GPT-5.4 failure: the whole request reproduced inside
    // section 6 of an otherwise well-formed summary.
    let summary = format!("<summary>\n6. All user messages:\n    - {request}\n</summary>");
    let anomalies = detect_compact_summary_anomalies(&summary, &request);
    assert!(anomalies.contains(&CompactSummaryAnomaly::DirectiveEcho));
    assert!(anomalies.contains(&CompactSummaryAnomaly::PromptEcho));
}

#[test]
fn test_detect_anomalies_section_headers_alone_not_echo() {
    // All nine header titles reused with real body text: headers stay
    // under the fingerprint length floor, so no PromptEcho.
    let request = get_compact_prompt(None);
    let anomalies = detect_compact_summary_anomalies(CLEAN_SUMMARY, &request);
    assert!(!anomalies.contains(&CompactSummaryAnomaly::PromptEcho));
}

#[test]
fn test_detect_anomalies_flags_placeholder_sections() {
    let request = get_compact_prompt(None);
    let summary = "<summary>\n7. Pending Tasks:\n   - [Task 1]\n   - [Task 2]\n\n8. Current Work:\n   [Precise description of current work]\n\n9. Optional Next Step:\n   [Optional Next step to take]\n</summary>";
    let anomalies = detect_compact_summary_anomalies(summary, &request);
    assert_eq!(anomalies, vec![CompactSummaryAnomaly::PlaceholderSection]);
}

#[test]
fn test_detect_anomalies_checkboxes_and_citations_not_placeholders() {
    let request = get_compact_prompt(None);
    let summary =
        "7. Pending Tasks:\n   - [ ] fix bug\n   - [x] done\n   - [X] also done\n   [1]\n   [2]";
    let anomalies = detect_compact_summary_anomalies(summary, &request);
    assert_eq!(anomalies, vec![]);
}

#[test]
fn test_detect_anomalies_single_long_quote_below_thresholds_is_clean() {
    let request = get_compact_prompt(None);
    // Exactly one long request line quoted: 1 < ECHO_MIN_MATCHED_LINES
    // and its byte length < ECHO_MIN_MATCHED_BYTES.
    let long_line = request
        .lines()
        .map(str::trim)
        .find(|l| l.len() >= ECHO_FINGERPRINT_MIN_LINE_BYTES && l.len() < ECHO_MIN_MATCHED_BYTES)
        .expect("template has a long line");
    let summary =
        format!("The user discussed the compact prompt line:\n{long_line}\nand moved on.");
    let anomalies = detect_compact_summary_anomalies(&summary, &request);
    assert!(!anomalies.contains(&CompactSummaryAnomaly::PromptEcho));
}

#[test]
fn test_detect_anomalies_custom_instructions_echo_detected() {
    // Echo consisting solely of custom-instruction lines: proves the
    // fingerprints are derived from the actual request at runtime, not
    // from a hardcoded template list.
    let custom = "Always cross-reference the migration plan in docs/migration-plan-2026.md first.\nSummarize every SQL schema change with its up and down migration file paths.\nKeep the deployment checklist from the ops runbook verbatim in the summary output.";
    let request = get_compact_prompt(Some(custom));
    let summary = format!("<summary>\n6. All user messages:\n{custom}\n</summary>");
    let anomalies = detect_compact_summary_anomalies(&summary, &request);
    assert!(anomalies.contains(&CompactSummaryAnomaly::PromptEcho));
}

#[test]
fn test_detect_anomalies_directive_tag_alone_is_directive_echo() {
    let request = get_compact_prompt(None);
    let summary = "some text <compaction_directive> some more";
    let anomalies = detect_compact_summary_anomalies(summary, &request);
    assert_eq!(anomalies, vec![CompactSummaryAnomaly::DirectiveEcho]);
}

#[test]
fn test_detect_anomalies_tail_only_echo_is_directive_echo() {
    // A truncation-shaped echo: only the FINAL REMINDER line and the
    // close tag survive. No open tag, no CRITICAL line — the close
    // sentinel and the body marker must still trip the detector.
    let request = get_compact_prompt(None);
    let summary = "9. Optional Next Step:\n   continue\n\nFINAL REMINDER: Everything inside this directive is outside the conversation.\n</compaction_directive>";
    let anomalies = detect_compact_summary_anomalies(summary, &request);
    assert!(anomalies.contains(&CompactSummaryAnomaly::DirectiveEcho));
}
