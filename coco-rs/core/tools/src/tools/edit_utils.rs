//! Edit tool utilities.
//!
//! Provides patch generation, quote style preservation, whitespace
//! normalization for matching, input equivalence checking, and
//! de-sanitization of model output.

use std::collections::HashMap;

// ── Curly quote constants ──
// Claude cannot output curly quotes, so the model uses straight quotes.
// We normalize curly → straight for matching, then restore the file's style.

const LEFT_SINGLE_CURLY: char = '\u{2018}';
const RIGHT_SINGLE_CURLY: char = '\u{2019}';
const LEFT_DOUBLE_CURLY: char = '\u{201C}';
const RIGHT_DOUBLE_CURLY: char = '\u{201D}';

// ── Quote normalization ──

/// Normalize curly quotes to straight quotes for matching.
pub fn normalize_quotes(s: &str) -> String {
    s.replace([LEFT_SINGLE_CURLY, RIGHT_SINGLE_CURLY], "'")
        .replace([LEFT_DOUBLE_CURLY, RIGHT_DOUBLE_CURLY], "\"")
}

/// Find the actual string in file content that matches the search string,
/// accounting for quote normalization.
///
/// Returns the actual substring from the file (preserving its original quotes),
/// or None if not found even after normalization.
pub fn find_actual_string<'a>(file_content: &'a str, search_string: &str) -> Option<&'a str> {
    // Fast path: exact match
    if let Some(pos) = file_content.find(search_string) {
        return Some(&file_content[pos..pos + search_string.len()]);
    }

    // Normalize both sides and search again
    let normalized_search = normalize_quotes(search_string);
    let normalized_file = normalize_quotes(file_content);

    if let Some(pos) = normalized_file.find(&normalized_search) {
        // Map byte position back to the original file content.
        // Since normalization is char-by-char replacement preserving length in
        // most cases, byte offset mapping is approximate. Use char counting
        // for correctness.
        let char_offset = normalized_file[..pos].chars().count();
        let char_len = normalized_search.chars().count();

        let start_byte = file_content
            .char_indices()
            .nth(char_offset)
            .map(|(i, _)| i)?;
        let end_byte = file_content
            .char_indices()
            .nth(char_offset + char_len)
            .map(|(i, _)| i)
            .unwrap_or(file_content.len());

        Some(&file_content[start_byte..end_byte])
    } else {
        None
    }
}

/// Preserve the quote style of the original file when applying replacements.
///
/// If `old_string` differs from `actual_old_string` (i.e., normalization was needed),
/// apply the same curly quote style to `new_string`.
pub fn preserve_quote_style(old_string: &str, actual_old_string: &str, new_string: &str) -> String {
    if old_string == actual_old_string {
        return new_string.to_string();
    }

    let has_double = actual_old_string.contains(LEFT_DOUBLE_CURLY)
        || actual_old_string.contains(RIGHT_DOUBLE_CURLY);
    let has_single = actual_old_string.contains(LEFT_SINGLE_CURLY)
        || actual_old_string.contains(RIGHT_SINGLE_CURLY);

    if !has_double && !has_single {
        return new_string.to_string();
    }

    let mut result = new_string.to_string();
    if has_double {
        result = apply_curly_double_quotes(&result);
    }
    if has_single {
        result = apply_curly_single_quotes(&result);
    }
    result
}

fn is_opening_context(chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }
    matches!(
        chars[index - 1],
        ' ' | '\t' | '\n' | '\r' | '(' | '[' | '{' | '\u{2014}' | '\u{2013}'
    )
}

fn apply_curly_double_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len());
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '"' {
            result.push(if is_opening_context(&chars, i) {
                LEFT_DOUBLE_CURLY
            } else {
                RIGHT_DOUBLE_CURLY
            });
        } else {
            result.push(ch);
        }
    }
    result
}

fn apply_curly_single_quotes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len());
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '\'' {
            // Apostrophe in contraction (letter-quote-letter) → right curly
            let prev_is_letter = i > 0 && chars[i - 1].is_alphabetic();
            let next_is_letter = i + 1 < chars.len() && chars[i + 1].is_alphabetic();
            if prev_is_letter && next_is_letter {
                result.push(RIGHT_SINGLE_CURLY);
            } else {
                result.push(if is_opening_context(&chars, i) {
                    LEFT_SINGLE_CURLY
                } else {
                    RIGHT_SINGLE_CURLY
                });
            }
        } else {
            result.push(ch);
        }
    }
    result
}

// ── Whitespace normalization ──

/// Strip trailing whitespace from each line while preserving line endings.
pub fn strip_trailing_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut line_start = 0;

    for (i, ch) in s.char_indices() {
        if ch == '\n' {
            // Trim trailing whitespace from the line (before \n)
            let line = &s[line_start..i];
            result.push_str(line.trim_end());
            result.push('\n');
            line_start = i + 1;
        } else if ch == '\r' {
            // Handle \r\n
            let next = s[i + 1..].starts_with('\n');
            let line = &s[line_start..i];
            result.push_str(line.trim_end());
            if next {
                result.push_str("\r\n");
                line_start = i + 2;
            } else {
                result.push('\r');
                line_start = i + 1;
            }
        }
    }
    // Last line (no trailing newline)
    if line_start < s.len() {
        result.push_str(s[line_start..].trim_end());
    }
    result
}

// ── Edit application ──

/// A single file edit operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub old_string: String,
    pub new_string: String,
    pub replace_all: bool,
}

/// Apply a single edit to file content.
///
/// When `new_string` is empty (deletion) and `old_string` doesn't end with '\n'
/// but is followed by '\n' in the content, the trailing newline is also removed.
pub fn apply_edit_to_file(content: &str, old: &str, new: &str, replace_all: bool) -> String {
    if new.is_empty() {
        // Deletion: also strip trailing newline if applicable
        let target = if !old.ends_with('\n') && content.contains(&format!("{old}\n")) {
            format!("{old}\n")
        } else {
            old.to_string()
        };

        if replace_all {
            content.replace(&target, "")
        } else {
            content.replacen(&target, "", 1)
        }
    } else if replace_all {
        content.replace(old, new)
    } else {
        content.replacen(old, new, 1)
    }
}

/// Apply a sequence of edits to file content, validating each one actually changes something.
///
/// Returns the updated file content, or an error if an edit doesn't match.
pub fn apply_edits(content: &str, edits: &[FileEdit]) -> Result<String, EditError> {
    let mut updated = content.to_string();
    let mut applied_new_strings: Vec<String> = Vec::new();

    // Special case: empty file creation (old="" and new="" on empty content)
    if content.is_empty() && edits.len() == 1 {
        let edit = &edits[0];
        if edit.old_string.is_empty() && edit.new_string.is_empty() {
            return Ok(String::new());
        }
    }

    for edit in edits {
        // Guard: old_string should not be a substring of a previously applied new_string
        let old_stripped = edit.old_string.trim_end_matches('\n');
        if !old_stripped.is_empty() {
            for prev_new in &applied_new_strings {
                if prev_new.contains(old_stripped) {
                    return Err(EditError::SubstringConflict);
                }
            }
        }

        let previous = updated.clone();
        updated = if edit.old_string.is_empty() {
            edit.new_string.clone()
        } else {
            apply_edit_to_file(
                &updated,
                &edit.old_string,
                &edit.new_string,
                edit.replace_all,
            )
        };

        if updated == previous {
            return Err(EditError::StringNotFound);
        }

        applied_new_strings.push(edit.new_string.clone());
    }

    if updated == content {
        return Err(EditError::NoChange);
    }

    Ok(updated)
}

/// Errors from edit application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// old_string was not found in the file.
    StringNotFound,
    /// Edits produced no change (original == result).
    NoChange,
    /// An edit's old_string is a substring of a previous edit's new_string.
    SubstringConflict,
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StringNotFound => write!(f, "String not found in file. Failed to apply edit."),
            Self::NoChange => {
                write!(
                    f,
                    "Original and edited file match exactly. Failed to apply edit."
                )
            }
            Self::SubstringConflict => write!(
                f,
                "Cannot edit file: old_string is a substring of a new_string from a previous edit."
            ),
        }
    }
}

impl std::error::Error for EditError {}

impl coco_error::StackError for EditError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn coco_error::StackError> {
        None
    }
}

impl coco_error::ErrorExt for EditError {
    fn status_code(&self) -> coco_error::StatusCode {
        coco_error::StatusCode::InvalidArguments
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ── De-sanitization ──

/// Sanitized tag mapping for reversing API-layer sanitization.
fn desanitization_map() -> &'static HashMap<&'static str, &'static str> {
    use std::sync::LazyLock;
    static MAP: LazyLock<HashMap<&str, &str>> = LazyLock::new(|| {
        let mut m = HashMap::new();
        m.insert("<fnr>", "<function_results>");
        m.insert("<n>", "<name>");
        m.insert("</n>", "</name>");
        m.insert("<o>", "<output>");
        m.insert("</o>", "</output>");
        m.insert("<e>", "<error>");
        m.insert("</e>", "</error>");
        m.insert("<s>", "<system>");
        m.insert("</s>", "</system>");
        m.insert("<r>", "<result>");
        m.insert("</r>", "</result>");
        m.insert("< META_START >", "<META_START>");
        m.insert("< META_END >", "<META_END>");
        m.insert("< EOT >", "<EOT>");
        m.insert("< META >", "<META>");
        m.insert("< SOS >", "<SOS>");
        m.insert("\n\nH:", "\n\nHuman:");
        m.insert("\n\nA:", "\n\nAssistant:");
        m
    });
    &MAP
}

/// Attempt to desanitize `old_string` and `new_string` for matching.
///
/// If the model outputs sanitized forms (e.g. `<fnr>` instead of
/// `<function_results>`), we reverse the sanitization so the edit
/// can match the actual file content.
///
/// Returns the (possibly desanitized) old/new strings and whether any
/// replacements were applied.
pub fn desanitize_for_edit(
    old_string: &str,
    new_string: &str,
    file_content: &str,
) -> (String, String, bool) {
    let map = desanitization_map();
    let mut desanitized_old = old_string.to_string();
    let mut applied: Vec<(&str, &str)> = Vec::new();

    for (&from, &to) in map.iter() {
        if desanitized_old.contains(from) {
            desanitized_old = desanitized_old.replace(from, to);
            applied.push((from, to));
        }
    }

    if applied.is_empty() || !file_content.contains(&desanitized_old) {
        return (old_string.to_string(), new_string.to_string(), false);
    }

    // Apply the same replacements to new_string
    let mut desanitized_new = new_string.to_string();
    for (from, to) in &applied {
        desanitized_new = desanitized_new.replace(from, to);
    }

    (desanitized_old, desanitized_new, true)
}

/// Input normalization for a single edit before matching.
///
///   1. Strip trailing whitespace from `new_string` UNLESS the file is
///      a Markdown file (`.md` / `.mdx`). Markdown uses two trailing
///      spaces as a hard line break, so stripping would silently
///      change semantics.
///
///   2. If the raw `old_string` already matches in the file, keep it
///      as-is with the whitespace-normalized `new_string`.
///
///   3. Otherwise, try desanitizing `old_string` (unescape common LLM
///      sanitization bugs like `<fnr>` → `<function_results>`). If the
///      desanitized version matches in the file, apply the SAME
///      replacements to `new_string` too so the final edit is
///      self-consistent.
///
///   4. Otherwise, keep the raw strings (with normalized new_string).
///      The Edit tool's regular matching fallback chain (quote
///      normalization, whitespace fuzzy) handles the remaining cases.
///
/// This wrapper operates on a single edit. The caller is responsible
/// for reading the file and passing the content.
///
/// Returns `(normalized_old, normalized_new)`.
pub fn normalize_file_edit_input(
    file_path: &str,
    file_content: &str,
    old_string: &str,
    new_string: &str,
) -> (String, String) {
    // Markdown trailing-space rule: two trailing spaces mean "hard line
    // break". Stripping them would silently corrupt the diff.
    let is_markdown = matches_markdown_extension(file_path);
    let normalized_new = if is_markdown {
        new_string.to_string()
    } else {
        strip_trailing_whitespace(new_string)
    };

    // Fast path: raw old_string already matches.
    if file_content.contains(old_string) {
        return (old_string.to_string(), normalized_new);
    }

    // Try desanitization. Only returns `replaced = true` when the
    // desanitized form actually matches in the file. On success,
    // it has already applied the same replacements to new_string.
    let (desanitized_old, desanitized_new, replaced) =
        desanitize_for_edit(old_string, &normalized_new, file_content);
    if replaced {
        return (desanitized_old, desanitized_new);
    }

    // Neither the raw nor the desanitized form matches — return the
    // raw old_string + normalized new_string. The caller's matching
    // fallback chain (quote normalization, whitespace fuzzy) takes
    // over from here.
    (old_string.to_string(), normalized_new)
}

/// Case-insensitive check for `.md` and `.mdx` file extensions.
///
/// Case-insensitive so `Notes.MD` gets the same preservation as `notes.md`.
fn matches_markdown_extension(file_path: &str) -> bool {
    // Find the last `.` AFTER the last `/` (so we don't trip on
    // `.md` in a parent directory name).
    let file_name = file_path.rsplit(['/', '\\']).next().unwrap_or(file_path);
    let Some(dot) = file_name.rfind('.') else {
        return false;
    };
    let ext = &file_name[dot + 1..];
    ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("mdx")
}

#[cfg(test)]
#[path = "edit_utils.test.rs"]
mod tests;
