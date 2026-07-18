use pretty_assertions::assert_eq;

use super::*;

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
