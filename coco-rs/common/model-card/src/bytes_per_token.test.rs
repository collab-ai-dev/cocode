use super::bytes_per_token_for_model;
use pretty_assertions::assert_eq;

#[test]
fn claude_family_returns_four() {
    let cases = [
        "claude-3-opus",
        "claude-3-sonnet",
        "claude-3-haiku",
        "claude-3-5-sonnet",
        "claude-3-5-haiku",
        "claude-3-7-sonnet",
        "claude-opus-4-0",
        "claude-opus-4-5",
        "claude-opus-4-6",
        "claude-opus-4-7",
        "claude-sonnet-4-0",
        "claude-sonnet-4-5",
        "claude-sonnet-4-6",
        "claude-haiku-4-5",
        "claude-3-5-sonnet-20241022",
        "claude-opus-4-7-fast",
    ];
    for id in cases {
        assert_eq!(bytes_per_token_for_model(id), 4, "{id}");
    }
}

#[test]
fn provider_prefixed_claude_id_still_returns_four() {
    let cases = [
        "anthropic/claude-sonnet-4.5",
        "anthropic/claude-opus-4-7",
        "anthropic.claude-3-5-sonnet-v2:0",
    ];
    for id in cases {
        assert_eq!(bytes_per_token_for_model(id), 4, "{id}");
    }
}

#[test]
fn context_1m_variant_is_recognized_as_claude() {
    // 2.1.142 normalizes `[1m]` away before exact match; coco-rs
    // substring match doesn't need to — `claude` is already a hit.
    assert_eq!(bytes_per_token_for_model("claude-opus-4-7[1m]"), 4);
    assert_eq!(bytes_per_token_for_model("claude-sonnet-4-6[1m]"), 4);
}

#[test]
fn case_insensitive_match() {
    let cases = [
        "Claude-3-5-Sonnet",
        "CLAUDE-OPUS-4-7",
        "Claude-Haiku-4-5",
        "Opus-4",
    ];
    for id in cases {
        assert_eq!(bytes_per_token_for_model(id), 4, "{id}");
    }
}

#[test]
fn non_claude_returns_three() {
    let cases = [
        "gpt-4o",
        "gpt-4o-2024-11-20",
        "gpt-5",
        "gpt-5-codex",
        "o1",
        "o1-mini",
        "o3-pro",
        "gemini-2.5-pro",
        "gemini-2-0-flash",
        "doubao-seedance-1-0-pro",
        "grok-4",
        "deepseek-v3",
        "qwen3-next-80b-a3b-instruct",
    ];
    for id in cases {
        assert_eq!(bytes_per_token_for_model(id), 3, "{id}");
    }
}

#[test]
fn empty_id_falls_back_to_four() {
    assert_eq!(bytes_per_token_for_model(""), 4);
}

#[test]
fn keywords_match_anywhere_in_id() {
    // Each of the four keywords is sufficient on its own.
    assert_eq!(bytes_per_token_for_model("just-claude"), 4);
    assert_eq!(bytes_per_token_for_model("opus-only"), 4);
    assert_eq!(bytes_per_token_for_model("my-sonnet-fork"), 4);
    assert_eq!(bytes_per_token_for_model("custom-haiku-v2"), 4);
}
