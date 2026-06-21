//! djb2 hash + JS-equivalent base36 stringification.
//!
//! `simpleHash`: `Math.abs(djb2Hash(str)).toString(36)`.
//! `djb2Hash` uses the canonical algorithm: `hash * 33 + ch` with
//! `| 0` int32 truncation at each step.

/// djb2 hash on UTF-16 code units, with i32 wrap-on-overflow
/// matching JS `| 0` semantics.
///
/// Returns the 32-bit signed integer the JS impl would compute.
pub fn djb2(input: &str) -> i32 {
    let mut hash: i32 = 5381;
    for code_unit in input.encode_utf16() {
        // JS: `hash = (hash * 33 + ch) | 0` — wrap on i32.
        hash = hash.wrapping_mul(33).wrapping_add(i32::from(code_unit));
    }
    hash
}

/// `Math.abs(djb2(input)).toString(36)` — the suffix appended by
/// `sanitize_path` when the sanitized length exceeds 200.
///
/// JS `Math.abs` on `i32::MIN` returns `2_147_483_648` as a Number;
/// we cast through `i64` to avoid the overflow that
/// `(i32::MIN).abs()` would trigger in Rust.
pub fn simple_hash(input: &str) -> String {
    let signed = djb2(input);
    let abs = (signed as i64).unsigned_abs();
    to_base36_lower(abs)
}

fn to_base36_lower(mut n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    // u64 max in base36 is 13 chars. Push char-by-char (each pushed
    // value is in `[0-9a-z]`, all ASCII), then reverse — avoids a
    // `from_utf8` step that clippy's workspace lints reject.
    let mut chars: Vec<char> = Vec::with_capacity(13);
    while n > 0 {
        let digit = (n % 36) as u8;
        let c = if digit < 10 {
            char::from(b'0' + digit)
        } else {
            char::from(b'a' + (digit - 10))
        };
        chars.push(c);
        n /= 36;
    }
    chars.into_iter().rev().collect()
}

#[cfg(test)]
#[path = "djb2.test.rs"]
mod tests;
