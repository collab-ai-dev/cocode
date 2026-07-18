//! Stable short names for MCP channel permission requests.

/// 25-letter alphabet: a-z minus 'l' (looks like 1/I). 25^5 ≈ 9.8M space.
const ID_ALPHABET: &str = "abcdefghijkmnopqrstuvwxyz";

/// Substring blocklist — 5 random letters can spell things. If a generated ID
/// contains any of these, re-hash with a salt. Non-exhaustive; covers the
/// send-to-your-boss-by-accident tier.
const ID_AVOID_SUBSTRINGS: &[&str] = &[
    "fuck", "shit", "cunt", "cock", "dick", "twat", "piss", "crap", "bitch", "whore", "ass", "tit",
    "cum", "fag", "dyke", "nig", "kike", "rape", "nazi", "damn", "poo", "pee", "wank", "anus",
];

/// FNV-1a → u32, then base-25 encode into 5 letters. Not crypto — a stable
/// short letters-only ID. tool_use_ids are ASCII so byte iteration is correct.
fn hash_to_id(input: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in input.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let alphabet = ID_ALPHABET.as_bytes();
    let mut s = String::with_capacity(5);
    for _ in 0..5 {
        s.push(alphabet[(h % 25) as usize] as char);
        h /= 25;
    }
    s
}

/// Short ID from a tool_use_id. 5 letters from a 25-char alphabet (a-z minus
/// 'l'). Re-hashes with a salt suffix if the result contains a blocklisted
/// substring. Caps at 10 retries.
pub fn short_request_id(tool_use_id: &str) -> String {
    let mut candidate = hash_to_id(tool_use_id);
    for salt in 0..10 {
        if !ID_AVOID_SUBSTRINGS
            .iter()
            .any(|bad| candidate.contains(bad))
        {
            return candidate;
        }
        candidate = hash_to_id(&format!("{tool_use_id}:{salt}"));
    }
    candidate
}

/// Parse a channel permission reply matching
/// `/^\s*(y|yes|n|no)\s+([a-km-z]{5})\s*$/i`.
///
/// Returns `(approve, five_letter_id)` where `approve` is `true` for `y`/`yes`
/// and `false` for `n`/`no`. The id must be exactly 5 letters, each in `a-k` or
/// `m-z` (no `l`). Input is case-insensitive; the returned id is lowercased.
pub fn parse_permission_reply(input: &str) -> Option<(bool, String)> {
    let trimmed = input.trim();
    // Split into the verb and the id across the run of inner whitespace.
    let mut parts = trimmed.split_whitespace();
    let verb = parts.next()?;
    let id = parts.next()?;
    // Reject trailing chatter — exactly two whitespace-separated tokens.
    if parts.next().is_some() {
        return None;
    }

    let approve = match verb.to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => return None,
    };

    if id.chars().count() != 5 {
        return None;
    }
    let lower = id.to_ascii_lowercase();
    if !lower.chars().all(|c| matches!(c, 'a'..='k' | 'm'..='z')) {
        return None;
    }

    Some((approve, lower))
}

#[cfg(test)]
#[path = "naming.test.rs"]
mod tests;
