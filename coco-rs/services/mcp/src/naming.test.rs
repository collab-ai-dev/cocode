use pretty_assertions::assert_eq;

use super::*;

// ── normalize_name_for_mcp ──

#[test]
fn test_normalize_alphanumeric_unchanged() {
    assert_eq!(normalize_name_for_mcp("slack", false), "slack");
    assert_eq!(normalize_name_for_mcp("my-server", false), "my-server");
    assert_eq!(normalize_name_for_mcp("tool_v2", false), "tool_v2");
}

#[test]
fn test_normalize_replaces_special_chars() {
    assert_eq!(normalize_name_for_mcp("my server", false), "my_server");
    assert_eq!(normalize_name_for_mcp("my.tool", false), "my_tool");
    assert_eq!(
        normalize_name_for_mcp("hello@world!2", false),
        "hello_world_2"
    );
}

#[test]
fn test_normalize_claudeai_collapses_underscores() {
    // claude.ai servers collapse consecutive underscores
    assert_eq!(
        normalize_name_for_mcp("claude.ai Gmail", true),
        "claude_ai_Gmail"
    );
    assert_eq!(
        normalize_name_for_mcp("claude.ai  spaced", true),
        "claude_ai_spaced"
    );
}

#[test]
fn test_normalize_claudeai_strips_leading_trailing() {
    assert_eq!(normalize_name_for_mcp(" test ", true), "test");
    assert_eq!(normalize_name_for_mcp("__test__", true), "test");
}

#[test]
fn test_normalize_non_claudeai_preserves_underscores() {
    // Non-claude.ai servers keep underscores as-is
    assert_eq!(normalize_name_for_mcp("a  b", false), "a__b");
}

// ── mcp_tool_id (with normalization) ──

#[test]
fn test_mcp_tool_id_normalizes() {
    // Spaces become underscores
    assert_eq!(
        mcp_tool_id("my server", "send msg"),
        "mcp__my_server__send_msg"
    );
}

#[test]
fn test_mcp_tool_id_simple() {
    assert_eq!(mcp_tool_id("slack", "send"), "mcp__slack__send");
    assert_eq!(mcp_tool_id("github", "create_pr"), "mcp__github__create_pr");
}

// ── mcp_tool_id_raw ──

#[test]
fn test_mcp_tool_id_raw_no_normalization() {
    assert_eq!(
        mcp_tool_id_raw("already_clean", "tool"),
        "mcp__already_clean__tool"
    );
}

// ── mcp_prefix ──

#[test]
fn test_mcp_prefix() {
    assert_eq!(mcp_prefix("slack"), "mcp__slack__");
    assert_eq!(mcp_prefix("my server"), "mcp__my_server__");
}

// ── mcp_display_name ──

#[test]
fn test_mcp_display_name() {
    assert_eq!(
        mcp_display_name("mcp__slack__send_message", "slack"),
        "send_message"
    );
}

#[test]
fn test_mcp_display_name_no_prefix() {
    assert_eq!(mcp_display_name("unknown_tool", "slack"), "unknown_tool");
}

// ── parse_mcp_tool_id ──

#[test]
fn test_parse_mcp_tool_id() {
    let (server, tool) = parse_mcp_tool_id("mcp__slack__send").unwrap();
    assert_eq!(server, "slack");
    assert_eq!(tool, "send");
}

#[test]
fn test_parse_non_mcp_id() {
    assert!(parse_mcp_tool_id("Read").is_none());
    assert!(parse_mcp_tool_id("mcp__incomplete").is_none());
}

// ── short_request_id ──

#[test]
fn test_short_request_id_is_five_alphabet_chars() {
    let id = short_request_id("toolu_01ABCDEF");
    assert_eq!(id.chars().count(), 5);
    assert!(
        id.chars().all(|c| ID_ALPHABET.contains(c)),
        "id {id} contains chars outside the alphabet"
    );
    // 'l' is excluded from the alphabet entirely.
    assert!(!id.contains('l'), "id {id} must not contain 'l'");
}

#[test]
fn test_short_request_id_is_deterministic() {
    assert_eq!(
        short_request_id("toolu_01ABCDEF"),
        short_request_id("toolu_01ABCDEF")
    );
}

#[test]
fn test_short_request_id_differs_by_input() {
    assert_ne!(short_request_id("toolu_aaa"), short_request_id("toolu_bbb"));
}

#[test]
fn test_short_request_id_avoids_blocklist() {
    // Whatever the input, the output must never contain a blocklisted substring.
    for input in ["ass", "fuck", "toolu_01", "x", "rape", "nazi"] {
        let id = short_request_id(input);
        assert!(
            !ID_AVOID_SUBSTRINGS.iter().any(|bad| id.contains(bad)),
            "id {id} from {input} hit the blocklist"
        );
    }
}

#[test]
fn test_short_request_id_rehashes_blocklisted_hash() {
    // Find an input whose first hash hits the blocklist, then prove
    // short_request_id returns a re-salted (different) id.
    let mut found = None;
    for n in 0..5000 {
        let input = format!("seed{n}");
        let raw = hash_to_id(&input);
        if ID_AVOID_SUBSTRINGS.iter().any(|bad| raw.contains(bad)) {
            found = Some(input);
            break;
        }
    }
    let input = found.expect("expected at least one blocklist-hitting hash in 5000 seeds");
    let raw = hash_to_id(&input);
    let resolved = short_request_id(&input);
    assert_ne!(raw, resolved, "blocklisted id should be re-hashed");
    assert!(
        !ID_AVOID_SUBSTRINGS.iter().any(|bad| resolved.contains(bad)),
        "resolved id {resolved} still hits the blocklist"
    );
}

// ── parse_permission_reply ──

#[test]
fn test_parse_permission_reply_accepts_yes_forms() {
    assert_eq!(
        parse_permission_reply("y abcde"),
        Some((true, "abcde".to_string()))
    );
    assert_eq!(
        parse_permission_reply("YES abcde"),
        Some((true, "abcde".to_string()))
    );
    assert_eq!(
        parse_permission_reply("  yes   ABCDE  "),
        Some((true, "abcde".to_string()))
    );
}

#[test]
fn test_parse_permission_reply_accepts_no_forms() {
    assert_eq!(
        parse_permission_reply("n zzzzz"),
        Some((false, "zzzzz".to_string()))
    );
    assert_eq!(
        parse_permission_reply("NO zzzzz"),
        Some((false, "zzzzz".to_string()))
    );
}

#[test]
fn test_parse_permission_reply_rejects_bad_inputs() {
    // Wrong verb.
    assert_eq!(parse_permission_reply("maybe abcde"), None);
    // Bare verb, no id.
    assert_eq!(parse_permission_reply("yes"), None);
    // Id too short / too long.
    assert_eq!(parse_permission_reply("y abcd"), None);
    assert_eq!(parse_permission_reply("y abcdef"), None);
    // Id contains the excluded 'l'.
    assert_eq!(parse_permission_reply("y abcle"), None);
    // Id contains a non-letter.
    assert_eq!(parse_permission_reply("y abc1e"), None);
    // Trailing chatter.
    assert_eq!(parse_permission_reply("y abcde please"), None);
    // No bare yes/no without an id.
    assert_eq!(parse_permission_reply("y"), None);
}
