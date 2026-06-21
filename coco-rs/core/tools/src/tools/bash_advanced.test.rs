use super::*;

// ── is_silent_command tests ──

#[test]
fn test_silent_mv() {
    assert!(is_silent_command("mv a b"));
}

#[test]
fn test_silent_compound() {
    assert!(is_silent_command("mkdir -p dir && touch dir/file"));
}

#[test]
fn test_non_silent_echo() {
    assert!(!is_silent_command("echo hello"));
}

#[test]
fn test_non_silent_with_output() {
    assert!(!is_silent_command("cat file"));
}

// Command-semantics interpretation moved to the canonical
// `coco_shell::semantics` (see `exec/shell/src/semantics.test.rs`); the
// duplicate interpreter that lived here was removed.

// ── is_image_output tests ──

#[test]
fn test_image_output_detection() {
    assert!(is_image_output("data:image/png;base64,iVBOR..."));
    assert!(!is_image_output("just some text"));
    assert!(!is_image_output("data:text/plain;base64,abc"));
}

// ── parse_data_uri tests ──

#[test]
fn test_parse_data_uri() {
    let (media, data) = parse_data_uri("data:image/png;base64,abc123").unwrap();
    assert_eq!(media, "image/png");
    assert_eq!(data, "abc123");
}

#[test]
fn test_parse_data_uri_invalid() {
    assert!(parse_data_uri("not a data uri").is_none());
}

// ── detect_blocked_sleep_pattern tests ──

#[test]
fn test_blocked_sleep_standalone() {
    let result = detect_blocked_sleep_pattern("sleep 5");
    assert_eq!(result.as_deref(), Some("standalone sleep 5"));
}

#[test]
fn test_blocked_sleep_with_followup() {
    let result = detect_blocked_sleep_pattern("sleep 5 && echo done");
    // split_simple gives ["sleep 5", "echo done"]
    // first = "sleep 5", secs=5 >= 2, rest = "echo done"
    assert_eq!(result.as_deref(), Some("sleep 5 followed by: echo done"));
}

#[test]
fn test_allowed_short_sleep() {
    // Sub-2s sleeps are fine
    assert!(detect_blocked_sleep_pattern("sleep 1").is_none());
}

// ── command_has_any_cd tests ──

#[test]
fn test_cd_detection() {
    assert!(command_has_any_cd("cd /tmp && ls"));
    assert!(command_has_any_cd("cd src"));
    assert!(!command_has_any_cd("cat cd_file.txt"));
}

// ── extract_description tests ──

#[test]
fn test_extract_description_with_provided() {
    assert_eq!(
        extract_description("ls -la", Some("List all files")),
        "List all files"
    );
}

#[test]
fn test_extract_description_fallback() {
    assert_eq!(extract_description("ls", None), "ls");
}

#[test]
fn test_extract_description_long_command() {
    let long_cmd = "a".repeat(200);
    let desc = extract_description(&long_cmd, None);
    assert!(desc.len() < 200);
    assert!(desc.ends_with("..."));
}

// ── progress tracker tests ──

#[test]
fn test_progress_tracker_initial_state() {
    let tracker = BashProgressTracker::new();
    assert!(!tracker.should_emit_progress());
    assert!(!tracker.was_progress_emitted());
}

// ── get_command_type_for_logging tests ──

#[test]
fn test_command_type_known() {
    assert_eq!(get_command_type_for_logging("npm install"), "npm");
    assert_eq!(get_command_type_for_logging("cargo build"), "cargo");
}

#[test]
fn test_command_type_unknown() {
    assert_eq!(get_command_type_for_logging("my-custom-tool"), "other");
}
