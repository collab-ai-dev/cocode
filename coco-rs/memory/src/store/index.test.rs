use super::*;
use pretty_assertions::assert_eq;

#[test]
fn truncation_passes_through_under_caps() {
    let content = "# Memory Index\n\n- [a](a.md) — one\n- [b](b.md) — two\n";
    let out = truncate_entrypoint_content(content);
    assert!(!out.line_truncated);
    assert!(!out.byte_truncated);
    assert_eq!(out.content, content.trim());
}

#[test]
fn truncates_when_line_cap_exceeded() {
    let mut content = String::from("# Memory Index\n\n");
    for i in 0..MAX_ENTRYPOINT_LINES + 50 {
        content.push_str(&format!("- [t{i}](f{i}.md) — h\n"));
    }
    let out = truncate_entrypoint_content(&content);
    assert!(out.line_truncated);
    assert!(out.content.contains("> WARNING:"));
    let reported_lines = out.content.lines().count();
    // One header line + MAX_ENTRYPOINT_LINES preserved + warning lines
    assert!(reported_lines <= MAX_ENTRYPOINT_LINES + 5);
}

#[test]
fn truncates_when_byte_cap_exceeded_with_few_lines() {
    // One huge line bigger than the byte cap.
    let huge = "x".repeat(MAX_ENTRYPOINT_BYTES + 1_000);
    let content = format!("# Memory Index\n\n- [a](a.md) — {huge}\n");
    let out = truncate_entrypoint_content(&content);
    assert!(out.byte_truncated);
    assert!(out.content.contains("> WARNING:"));
}

#[test]
fn parses_well_formed_pointer_lines() {
    let content = "# Memory Index\n\n- [User Role](user_role.md) — data scientist\n- [No Mocks](feedback_no_mocks.md) — integration tests must hit a real db\n";
    let idx = parse_memory_index(content);
    assert_eq!(idx.entries.len(), 2);
    assert_eq!(idx.entries[0].title, "User Role");
    assert_eq!(idx.entries[0].file, "user_role.md");
    assert_eq!(idx.entries[0].hook, "data scientist");
    assert_eq!(idx.entries[1].file, "feedback_no_mocks.md");
}

#[test]
fn ignores_commentary_lines() {
    let content = "# Memory Index\n\nSome prose.\n- [a](a.md) — h\n";
    let idx = parse_memory_index(content);
    assert_eq!(idx.entries.len(), 1);
}
