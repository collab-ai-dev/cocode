use pretty_assertions::assert_eq;

use super::*;

// --- extract_operation tests ---

#[test]
fn test_extract_operation_with_deny_and_parenthesized_code() {
    let line = "2024-01-15 10:30:45.123 Df sandbox[1234] Sandbox: bash(5678) deny(1) file-write-data /tmp/foo";
    let op = extract_operation(line);
    assert_eq!(op.as_deref(), Some("file-write-data"));
}

#[test]
fn test_extract_operation_network_outbound() {
    let line =
        "2024-01-15 10:30:45.456 Df sandbox[1234] Sandbox: bash(5678) deny(1) network-outbound";
    let op = extract_operation(line);
    assert_eq!(op.as_deref(), Some("network-outbound"));
}

#[test]
fn test_extract_operation_without_parenthesized_code() {
    let line = "Sandbox: bash(5678) deny file-read-data /etc/passwd";
    let op = extract_operation(line);
    assert_eq!(op.as_deref(), Some("file-read-data"));
}

#[test]
fn test_extract_operation_no_deny() {
    let line = "some random log line without the keyword";
    let op = extract_operation(line);
    assert_eq!(op, None);
}

#[test]
fn test_extract_operation_deny_at_end_of_line() {
    let line = "something deny";
    let op = extract_operation(line);
    assert_eq!(op, None);
}

#[test]
fn test_extract_operation_multi_digit_code() {
    let line = "Sandbox: node(999) deny(42) process-exec /usr/bin/env";
    let op = extract_operation(line);
    assert_eq!(op.as_deref(), Some("process-exec"));
}

// --- extract_path tests ---

#[test]
fn test_extract_path_present() {
    let line = "deny(1) file-write-data /tmp/foo/bar.txt rest";
    let path = extract_path(line, "file-write-data");
    assert_eq!(path.as_deref(), Some("/tmp/foo/bar.txt"));
}

#[test]
fn test_extract_path_absent_for_network() {
    let line = "deny(1) network-outbound 10.0.0.1:443";
    let path = extract_path(line, "network-outbound");
    assert_eq!(path, None);
}

#[test]
fn test_extract_path_at_end_of_line() {
    let line = "deny(1) file-read-data /etc/passwd";
    let path = extract_path(line, "file-read-data");
    assert_eq!(path.as_deref(), Some("/etc/passwd"));
}

#[test]
fn test_extract_path_operation_not_found() {
    let line = "deny(1) file-write-data /tmp/foo";
    let path = extract_path(line, "network-outbound");
    assert_eq!(path, None);
}

// --- Session tag generation ---

#[test]
fn test_generate_session_tag_format() {
    let tag = generate_session_tag();
    assert!(tag.starts_with('_'));
    assert!(tag.ends_with("_SBX"));
    // _<8 alphanumeric>_SBX = 1 + 8 + 4 = 13 chars
    assert_eq!(tag.len(), 13);
}

#[test]
fn test_generate_session_tag_uniqueness() {
    let tag1 = generate_session_tag();
    let tag2 = generate_session_tag();
    assert_ne!(tag1, tag2);
}

// --- Command tag encoding/decoding ---

#[test]
fn test_generate_command_tag_format() {
    let tag = generate_command_tag("echo hello", "_a1b2c3d4_SBX");
    assert!(tag.starts_with("CMD64_"));
    assert!(tag.ends_with("_a1b2c3d4_SBX"));
}

#[test]
fn test_command_tag_roundtrip() {
    let session_tag = "_abcdef01_SBX";
    let command = "npm install --save lodash";
    let tag = generate_command_tag(command, session_tag);
    let decoded = decode_command_tag(&tag);
    assert_eq!(decoded.as_deref(), Some(command));
}

#[test]
fn test_command_tag_roundtrip_special_chars() {
    let session_tag = "_12345678_SBX";
    let command = "echo 'hello world' | grep -c 'hello' && rm -rf /tmp/*";
    let tag = generate_command_tag(command, session_tag);
    let decoded = decode_command_tag(&tag);
    assert_eq!(decoded.as_deref(), Some(command));
}

#[test]
fn test_decode_command_tag_invalid() {
    assert_eq!(decode_command_tag("not a tag"), None);
    assert_eq!(decode_command_tag("CMD64_"), None);
    assert_eq!(decode_command_tag("CMD64_invalid_base64_END_tag"), None);
}

#[test]
fn test_command_tag_truncates_long_commands() {
    let long = "x".repeat(500);
    let tag = generate_command_tag(&long, "_test_SBX");
    let decoded = decode_command_tag(&tag).unwrap();
    assert_eq!(decoded.len(), 100, "should truncate to 100 chars");
    assert_eq!(decoded, "x".repeat(100));
}

#[test]
fn test_command_tag_truncation_at_utf8_boundary() {
    // Each emoji is 4 bytes; 25 emojis = 100 bytes exactly
    let emojis = "🎉".repeat(25);
    assert_eq!(emojis.len(), 100);
    let tag = generate_command_tag(&emojis, "_test_SBX");
    let decoded = decode_command_tag(&tag).unwrap();
    assert_eq!(decoded, emojis, "100-byte input should be preserved fully");

    // 26 emojis = 104 bytes; should truncate to 25 emojis (100 bytes)
    let emojis_over = "🎉".repeat(26);
    let tag_over = generate_command_tag(&emojis_over, "_test_SBX");
    let decoded_over = decode_command_tag(&tag_over).unwrap();
    assert_eq!(
        decoded_over,
        "🎉".repeat(25),
        "should truncate at char boundary, not mid-emoji"
    );
}

#[test]
fn test_command_tag_exact_boundary() {
    let exactly_100 = "a".repeat(100);
    let tag = generate_command_tag(&exactly_100, "_t_SBX");
    let decoded = decode_command_tag(&tag).unwrap();
    assert_eq!(decoded.len(), 100);

    let exactly_101 = "a".repeat(101);
    let tag = generate_command_tag(&exactly_101, "_t_SBX");
    let decoded = decode_command_tag(&tag).unwrap();
    assert_eq!(decoded.len(), 100);
}

// --- Log predicate ---

#[test]
fn test_build_log_predicate() {
    let pred = build_log_predicate("_a1b2c3d4_SBX");
    assert_eq!(pred, "eventMessage ENDSWITH \"_a1b2c3d4_SBX\"");
}

// --- extract_command_tag ---

#[test]
fn test_extract_command_tag_present() {
    let line = "deny(1) file-write-data /tmp/foo CMD64_ZWNobyBoZWxsbw==_END_a1b2c3d4_SBX";
    let tag = extract_command_tag(line);
    assert_eq!(
        tag.as_deref(),
        Some("CMD64_ZWNobyBoZWxsbw==_END_a1b2c3d4_SBX")
    );
}

#[test]
fn test_extract_command_tag_absent() {
    let line = "deny(1) file-write-data /tmp/foo";
    let tag = extract_command_tag(line);
    assert_eq!(tag, None);
}

// --- parse_violation_line tests ---

#[test]
fn test_parse_violation_line_file_write() {
    let line = "2024-01-15 10:30:45.123 Df sandbox[1234] Sandbox: bash(5678) deny(1) file-write-data /tmp/foo";
    let violation = parse_violation_line(line).expect("should parse");
    assert_eq!(violation.operation, "file-write-data");
    assert_eq!(violation.path.as_deref(), Some("/tmp/foo"));
    assert!(!violation.benign);
    assert!(violation.command_tag.is_none());
}

#[test]
fn test_parse_violation_line_with_command_tag() {
    let line = "Sandbox: bash(5678) deny(1) file-write-data /tmp/foo CMD64_ZWNobyBoZWxsbw==_END_abc123_SBX";
    let violation = parse_violation_line(line).expect("should parse");
    assert_eq!(violation.operation, "file-write-data");
    assert!(violation.command_tag.is_some());
    assert!(violation.command_tag.unwrap().starts_with("CMD64_"));
}

#[test]
fn test_parse_violation_line_network() {
    let line =
        "2024-01-15 10:30:45.456 Df sandbox[1234] Sandbox: bash(5678) deny(1) network-outbound";
    let violation = parse_violation_line(line).expect("should parse");
    assert_eq!(violation.operation, "network-outbound");
    assert_eq!(violation.path, None);
    assert!(!violation.benign);
}

#[test]
fn test_parse_violation_line_benign_mdnsresponder() {
    let line = "2024-01-15 Df mDNSResponder[99] Sandbox: deny(1) network-outbound";
    let violation = parse_violation_line(line).expect("should parse");
    assert!(violation.benign);
}

#[test]
fn test_parse_violation_line_benign_diagnosticd() {
    let line = "2024-01-15 Df diagnosticd[100] Sandbox: deny(1) file-read-data /some/path";
    let violation = parse_violation_line(line).expect("should parse");
    assert!(violation.benign);
}

#[test]
fn test_parse_violation_line_benign_analyticsd() {
    let line = "2024-01-15 Df analyticsd[101] Sandbox: deny(1) file-write-data /var/log";
    let violation = parse_violation_line(line).expect("should parse");
    assert!(violation.benign);
}

#[test]
fn test_parse_violation_line_benign_trustd() {
    let line = "2024-01-15 Df com.apple.trustd[102] Sandbox: deny(1) network-outbound";
    let violation = parse_violation_line(line).expect("should parse");
    assert!(violation.benign);
}

#[test]
fn test_parse_violation_line_no_deny() {
    let line = "2024-01-15 10:30:45 Normal log line with Sandbox mentioned";
    let violation = parse_violation_line(line);
    assert!(violation.is_none());
}

#[test]
fn test_parse_violation_line_empty() {
    let violation = parse_violation_line("");
    assert!(violation.is_none());
}

#[test]
fn test_parse_violation_line_process_exec() {
    let line = "Sandbox: node(1234) deny(1) process-exec /usr/bin/git";
    let violation = parse_violation_line(line).expect("should parse");
    assert_eq!(violation.operation, "process-exec");
    assert_eq!(violation.path.as_deref(), Some("/usr/bin/git"));
    assert!(!violation.benign);
}

// --- ViolationMonitor Linux (passive mode) ---

#[cfg(target_os = "linux")]
#[tokio::test]
async fn test_monitor_start_returns_some_on_linux() {
    let store = Arc::new(Mutex::new(ViolationStore::new()));
    let token = CancellationToken::new();
    let tag = generate_session_tag();
    let monitor = ViolationMonitor::start(store, token, tag);
    // Linux uses passive mode (no background process, but returns Some)
    assert!(monitor.is_some());
}

// --- ViolationMonitor unsupported platforms ---

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[tokio::test]
async fn test_monitor_start_returns_none_on_unsupported() {
    let store = Arc::new(Mutex::new(ViolationStore::new()));
    let token = CancellationToken::new();
    let tag = generate_session_tag();
    let monitor = ViolationMonitor::start(store, token, tag);
    assert!(monitor.is_none());
}

// --- ViolationMonitor macOS ---

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_monitor_start_returns_some_on_macos() {
    let store = Arc::new(Mutex::new(ViolationStore::new()));
    let token = CancellationToken::new();
    let tag = generate_session_tag();
    let monitor = ViolationMonitor::start(store, token.clone(), tag);
    assert!(monitor.is_some());
    // Clean up
    token.cancel();
    if let Some(mut m) = monitor {
        m.stop().await;
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_monitor_stop_is_idempotent() {
    let store = Arc::new(Mutex::new(ViolationStore::new()));
    let token = CancellationToken::new();
    let tag = generate_session_tag();
    let mut monitor = ViolationMonitor::start(store, token, tag).expect("macOS should start");
    monitor.stop().await;
    // Second stop should not panic.
    monitor.stop().await;
}

// --- Violation helpers (cross-platform) ---

#[test]
fn test_seccomp_violation_fields() {
    let violation = seccomp_violation(Some("CMD64_test_END_tag".to_string()));
    assert_eq!(violation.operation, "seccomp-kill");
    assert!(violation.path.is_none());
    assert!(!violation.benign);
    assert_eq!(violation.command_tag.as_deref(), Some("CMD64_test_END_tag"));
}

#[test]
fn test_network_deny_violation_fields() {
    let violation = network_deny_violation("evil.com", None);
    assert_eq!(violation.operation, "network-denied");
    assert_eq!(violation.path.as_deref(), Some("evil.com"));
    assert!(!violation.benign);
    assert!(violation.command_tag.is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn test_is_seccomp_violation_sigsys() {
    use std::os::unix::process::ExitStatusExt;
    // SIGSYS = signal 31 on Linux
    let status = std::process::ExitStatus::from_raw(31);
    assert!(is_seccomp_violation(&status));
}

#[cfg(target_os = "linux")]
#[test]
fn test_is_seccomp_violation_normal_exit() {
    use std::os::unix::process::ExitStatusExt;
    // Normal exit (code 0, encoded as 0<<8 = 0)
    let status = std::process::ExitStatus::from_raw(0);
    assert!(!is_seccomp_violation(&status));
}

#[test]
fn test_is_seccomp_violation_non_linux() {
    // On non-Linux platforms, should always return false
    if !cfg!(target_os = "linux") {
        let status = std::process::Command::new("true")
            .status()
            .expect("run true");
        assert!(!is_seccomp_violation(&status));
    }
}
