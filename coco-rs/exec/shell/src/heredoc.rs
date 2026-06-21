//! Heredoc extraction from shell commands.
//!
//! Parses `<<DELIM...DELIM` and `<<'DELIM'...DELIM` patterns from
//! command strings. Quoted delimiters indicate no variable expansion.

/// A single heredoc extracted from a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeredocContent {
    /// The delimiter word (without quotes).
    pub delimiter: String,
    /// The content between the opening and closing delimiters.
    pub content: String,
    /// Whether the delimiter was quoted (no variable expansion).
    pub is_quoted: bool,
}

/// Extract heredocs from a command string.
///
/// Returns a tuple of (command with heredocs removed, list of heredocs).
/// Supports `<<DELIM`, `<<'DELIM'`, and `<<"DELIM"` forms.
pub fn extract_heredocs(command: &str) -> (String, Vec<HeredocContent>) {
    let mut heredocs = Vec::new();
    let mut remaining_command = String::new();
    let lines: Vec<&str> = command.lines().collect();

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        if let Some(heredoc_start) = find_heredoc_operator(line) {
            let (before_heredoc, raw_delim) = line.split_at(heredoc_start);

            // Parse the delimiter from the <<DELIM portion
            let after_operator = raw_delim.trim_start_matches('<').trim_start_matches('-');
            let after_operator = after_operator.trim();

            if after_operator.is_empty() {
                // Malformed heredoc, keep line as-is
                remaining_command.push_str(line);
                remaining_command.push('\n');
                i += 1;
                continue;
            }

            let (delimiter, is_quoted) = parse_delimiter(after_operator);

            if delimiter.is_empty() {
                remaining_command.push_str(line);
                remaining_command.push('\n');
                i += 1;
                continue;
            }

            // Keep the part before the heredoc operator
            let before = before_heredoc.trim_end();
            if !before.is_empty() {
                remaining_command.push_str(before);
                remaining_command.push('\n');
            }

            // Collect content until closing delimiter
            let mut content = String::new();
            i += 1;
            while i < lines.len() {
                if lines[i].trim() == delimiter {
                    break;
                }
                if !content.is_empty() {
                    content.push('\n');
                }
                content.push_str(lines[i]);
                i += 1;
            }

            heredocs.push(HeredocContent {
                delimiter,
                content,
                is_quoted,
            });
        } else {
            remaining_command.push_str(line);
            remaining_command.push('\n');
        }

        i += 1;
    }

    // Trim trailing newline if we added one
    if remaining_command.ends_with('\n') {
        remaining_command.pop();
    }

    (remaining_command, heredocs)
}

/// Find the position of a `<<` heredoc operator in a line.
/// Returns None if no heredoc operator is found.
fn find_heredoc_operator(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'<' && bytes[i + 1] == b'<' {
            // Make sure it's not <<< (herestring)
            if i + 2 < bytes.len() && bytes[i + 2] == b'<' {
                i += 3;
                continue;
            }
            // Check not inside quotes (simple heuristic: count unescaped quotes before)
            let before = &line[..i];
            let single_quotes = before.chars().filter(|c| *c == '\'').count();
            let double_quotes = before.chars().filter(|c| *c == '"').count();
            if single_quotes % 2 == 0 && double_quotes % 2 == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Parse a delimiter, handling quoted forms.
/// Returns (delimiter_without_quotes, is_quoted).
fn parse_delimiter(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();

    // Take only the first word (delimiter might be followed by other text)
    let first_token = trimmed.split_whitespace().next().unwrap_or("");

    if let Some(inner) = first_token
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
    {
        (inner.to_string(), true)
    } else if let Some(inner) = first_token
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
    {
        (inner.to_string(), true)
    } else {
        (first_token.to_string(), false)
    }
}

#[cfg(test)]
#[path = "heredoc.test.rs"]
mod tests;
