// Truncate a &str to a byte budget at a char boundary (prefix)
#[inline]
pub fn take_bytes_at_char_boundary(s: &str, maxb: usize) -> &str {
    if s.len() <= maxb {
        return s;
    }
    let mut last_ok = 0;
    for (i, ch) in s.char_indices() {
        let nb = i + ch.len_utf8();
        if nb > maxb {
            break;
        }
        last_ok = nb;
    }
    &s[..last_ok]
}

// Take a suffix of a &str within a byte budget at a char boundary
#[inline]
pub fn take_last_bytes_at_char_boundary(s: &str, maxb: usize) -> &str {
    if s.len() <= maxb {
        return s;
    }
    let mut start = s.len();
    let mut used = 0usize;
    for (i, ch) in s.char_indices().rev() {
        let nb = ch.len_utf8();
        if used + nb > maxb {
            break;
        }
        start = i;
        used += nb;
        if start == 0 {
            break;
        }
    }
    &s[start..]
}

/// Sanitize a tag value to comply with metric tag validation rules:
/// only ASCII alphanumeric, '.', '_', '-', and '/' are allowed.
pub fn sanitize_metric_tag_value(value: &str) -> String {
    const MAX_LEN: usize = 256;
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | '/') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() || trimmed.chars().all(|ch| !ch.is_ascii_alphanumeric()) {
        return "unspecified".to_string();
    }
    if trimmed.len() <= MAX_LEN {
        trimmed.to_string()
    } else {
        trimmed[..MAX_LEN].to_string()
    }
}

/// Find all UUIDs in a string.
#[allow(clippy::unwrap_used)]
pub fn find_uuids(s: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex_lite::Regex::new(
            r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}",
        )
        .unwrap() // Unwrap is safe thanks to the tests.
    });

    re.find_iter(s).map(|m| m.as_str().to_string()).collect()
}

/// Convert a markdown-style `#L..` location suffix into a terminal-friendly
/// `:line[:column][-line[:column]]` suffix.
pub fn normalize_markdown_hash_location_suffix(suffix: &str) -> Option<String> {
    let fragment = suffix.strip_prefix('#')?;
    let (start, end) = match fragment.split_once('-') {
        Some((start, end)) => (start, Some(end)),
        None => (fragment, None),
    };
    let (start_line, start_column) = parse_markdown_hash_location_point(start)?;
    let mut normalized = String::from(":");
    normalized.push_str(start_line);
    if let Some(column) = start_column {
        normalized.push(':');
        normalized.push_str(column);
    }
    if let Some(end) = end {
        let (end_line, end_column) = parse_markdown_hash_location_point(end)?;
        normalized.push('-');
        normalized.push_str(end_line);
        if let Some(column) = end_column {
            normalized.push(':');
            normalized.push_str(column);
        }
    }
    Some(normalized)
}

fn parse_markdown_hash_location_point(point: &str) -> Option<(&str, Option<&str>)> {
    let point = point.strip_prefix('L')?;
    match point.split_once('C') {
        Some((line, column)) => Some((line, Some(column))),
        None => Some((point, None)),
    }
}

/// Truncate a string to a maximum byte length at a char boundary,
/// appending "..." if truncated.
pub fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..s.floor_char_boundary(max_len)])
    }
}

/// Truncate for structured logging: returns `"[{total} chars] {prefix}..."`
/// when over budget, original string otherwise.
/// Use at `debug!` level for summaries; log full content at `trace!`.
pub fn truncate_for_log(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let prefix = &s[..s.floor_char_boundary(max_chars)];
        format!("[{} chars] {prefix}...", s.len())
    }
}

/// Truncate by UTF-16 code-unit budget, appending `ellipsis` when truncated.
///
/// This matches JavaScript `str.length` / `slice` thresholds while preserving
/// Rust UTF-8 validity by never splitting a Unicode scalar value.
pub fn truncate_utf16_units_with_ellipsis(
    s: &str,
    max_units: usize,
    prefix_units: usize,
    ellipsis: &str,
) -> String {
    if s.encode_utf16().count() <= max_units {
        return s.to_string();
    }

    let mut prefix = String::new();
    let mut units = 0;
    for ch in s.chars() {
        let next = units + ch.len_utf16();
        if next > prefix_units {
            break;
        }
        prefix.push(ch);
        units = next;
    }
    format!("{prefix}{ellipsis}")
}

/// Encodes a byte slice as a lowercase hex string.
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Format an integer with `,` thousands separators (1234567 → "1,234,567").
pub fn format_thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let len = digits.len();
    let mut out = String::with_capacity(len + len / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, ch) in digits.chars().enumerate() {
        if i != 0 && (len - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Strip ANSI/ECMA-48 escape sequences from model-facing text.
///
/// Covers CSI (incl. private-mode `?` and colon parameters), OSC (BEL- and
/// ST-terminated), DCS/SOS/PM/APC (ST-terminated), other `ESC`-prefixed
/// sequences (nF intermediates + final), and stray 8-bit C1 controls
/// (U+0080–U+009F). Clean input — the overwhelmingly common case — returns
/// `Cow::Borrowed` without allocating.
///
/// Escape codes in tool results waste tokens and, worse, get copied by
/// models into file writes; strip *before* truncation so a byte cap can
/// never land mid-sequence and leave garbage.
pub fn strip_ansi(input: &str) -> std::borrow::Cow<'_, str> {
    fn is_c1(c: char) -> bool {
        ('\u{80}'..='\u{9f}').contains(&c)
    }
    if !input.chars().any(|c| c == '\u{1b}' || is_c1(c)) {
        return std::borrow::Cow::Borrowed(input);
    }

    /// Consume CSI parameter bytes (0x30–0x3F), intermediates (0x20–0x2F),
    /// and the final byte (0x40–0x7E). A malformed sequence stops without
    /// consuming the offending char, so real text is never eaten.
    fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        while let Some(&c) = chars.peek() {
            if ('\u{40}'..='\u{7e}').contains(&c) {
                chars.next(); // final byte
                break;
            }
            if ('\u{20}'..='\u{3f}').contains(&c) {
                chars.next();
                continue;
            }
            break; // malformed: leave the char to normal processing
        }
    }

    /// Consume until BEL, ST (`ESC \` or C1 0x9C), or end of input.
    fn skip_until_st(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, allow_bel: bool) {
        while let Some(c) = chars.next() {
            match c {
                '\u{07}' if allow_bel => break,
                '\u{9c}' => break,
                '\u{1b}' => {
                    if chars.peek() == Some(&'\\') {
                        chars.next();
                    }
                    break;
                }
                _ => {}
            }
        }
    }

    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\u{1b}' => match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    skip_csi(&mut chars);
                }
                Some(']') => {
                    chars.next();
                    skip_until_st(&mut chars, /*allow_bel*/ true);
                }
                Some('P' | 'X' | '^' | '_') => {
                    chars.next();
                    skip_until_st(&mut chars, /*allow_bel*/ false);
                }
                Some(c) if ('\u{20}'..='\u{2f}').contains(&c) => {
                    // nF sequence: intermediates then one final 0x30–0x7E.
                    while let Some(&c) = chars.peek() {
                        if ('\u{20}'..='\u{2f}').contains(&c) {
                            chars.next();
                            continue;
                        }
                        if ('\u{30}'..='\u{7e}').contains(&c) {
                            chars.next(); // final byte
                        }
                        break;
                    }
                }
                Some(c) if ('\u{30}'..='\u{7e}').contains(&c) => {
                    // Fp/Fe/Fs single-final escape.
                    chars.next();
                }
                _ => {} // bare trailing ESC: drop it
            },
            '\u{9b}' => skip_csi(&mut chars),
            '\u{9d}' => skip_until_st(&mut chars, /*allow_bel*/ true),
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                skip_until_st(&mut chars, /*allow_bel*/ false);
            }
            c if is_c1(c) => {} // stray single C1 control: drop
            c => out.push(c),
        }
    }
    std::borrow::Cow::Owned(out)
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
