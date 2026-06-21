//! User input processing.
//!
//! Pre-processes user input before sending to the model.

/// Process raw user input text before sending to the model.
///
/// Steps:
/// 1. Trim whitespace
/// 2. Detect /commands (slash prefix)
/// 3. Expand paste references
/// 4. Detect @mentions (file paths, URLs)
/// 5. Detect image pastes
pub fn process_user_input(raw: &str) -> ProcessedInput {
    let trimmed = raw.trim();

    if trimmed.is_empty() {
        return ProcessedInput {
            text: String::new(),
            is_command: false,
            command_name: None,
            command_args: None,
            mentions: Vec::new(),
            has_images: false,
        };
    }

    // Check for slash commands
    if let Some(stripped) = trimmed.strip_prefix('/') {
        let parts: Vec<&str> = stripped.splitn(2, ' ').collect();
        let cmd_name = parts[0].to_string();
        let cmd_args = parts.get(1).map(std::string::ToString::to_string);
        return ProcessedInput {
            text: trimmed.to_string(),
            is_command: true,
            command_name: Some(cmd_name),
            command_args: cmd_args,
            mentions: Vec::new(),
            has_images: false,
        };
    }

    // Extract @mentions (file paths)
    let mentions = extract_mentions(trimmed);

    ProcessedInput {
        text: trimmed.to_string(),
        is_command: false,
        command_name: None,
        command_args: None,
        mentions,
        has_images: false,
    }
}

/// Processed user input with metadata.
#[derive(Debug, Clone)]
pub struct ProcessedInput {
    pub text: String,
    pub is_command: bool,
    pub command_name: Option<String>,
    pub command_args: Option<String>,
    pub mentions: Vec<Mention>,
    pub has_images: bool,
}

/// A mention in user input (file path, URL, etc).
#[derive(Debug, Clone)]
pub struct Mention {
    pub text: String,
    pub mention_type: MentionType,
    pub start: usize,
    pub end: usize,
    /// Start line for line-range mentions (`@file#L10` → Some(10)).
    pub line_start: Option<i32>,
    /// End line for line-range mentions (`@file#L10-20` → Some(20)).
    pub line_end: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MentionType {
    FilePath,
    Url,
    Symbol,
    Agent,
    /// MCP resource reference of the form `@server:uri` (no `://`).
    McpResource {
        server: String,
        uri: String,
    },
}

/// Extract @mentions from text.
///
/// Supports:
/// - `@file.rs` — simple file path
/// - `@"path with spaces.rs"` — quoted path
/// - `@file.rs#L10` or `@file.rs#L10-20` — line ranges
/// - `@agent-type` — agent mentions
/// - `@https://url` — URL mentions
fn extract_mentions(text: &str) -> Vec<Mention> {
    let mut mentions = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();

    while i < bytes.len() {
        // @ must be at start or preceded by whitespace
        if bytes[i] == b'@' && i + 1 < bytes.len() && (i == 0 || bytes[i - 1].is_ascii_whitespace())
        {
            let start = i;
            i += 1; // skip @

            let mention_text = if bytes[i] == b'"' {
                // Quoted path: @"path with spaces"
                i += 1; // skip opening "
                let quote_start = i;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                if i >= bytes.len() {
                    // Unclosed quote — skip this malformed mention
                    continue;
                }
                let raw = &text[quote_start..i];
                i += 1; // skip closing "
                raw
            } else {
                // Unquoted: collect until whitespace
                let token_start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                &text[token_start..i]
            };

            if mention_text.is_empty() {
                continue;
            }

            // Parse line range from fragment: file#L10 or file#L10-20.
            // Also strips trailing non-`#L` fragments (`file.rs#heading` →
            // `file.rs`) to match the `parseAtMentionedFileLines` regex.
            let (path_text, line_start, line_end) = parse_line_range(mention_text);

            // Classify mention type. Order matters:
            //   1. Agent  — `agent-` prefix or `(agent)` suffix
            //   2. Url    — `scheme://...`
            //   3. McpResource — `server:uri` (no `://`)
            //   4. FilePath — has `/` or `.` (path-ish)
            //   5. Symbol — fallback
            let is_agent = path_text.starts_with("agent-") || mention_text.ends_with(" (agent)");
            let final_text = if is_agent {
                strip_agent_suffix(path_text)
            } else {
                path_text.to_string()
            };
            let mention_type = if is_agent {
                MentionType::Agent
            } else if path_text.contains("://") {
                MentionType::Url
            } else if let Some((server, uri)) = parse_mcp_resource(path_text) {
                mentions.push(Mention {
                    text: path_text.to_string(),
                    mention_type: MentionType::McpResource {
                        server: server.to_string(),
                        uri: uri.to_string(),
                    },
                    start,
                    end: i,
                    line_start: None,
                    line_end: None,
                });
                continue;
            } else if path_text.contains('/') || path_text.contains('.') {
                MentionType::FilePath
            } else {
                MentionType::Symbol
            };

            mentions.push(Mention {
                text: final_text,
                mention_type,
                start,
                end: i,
                line_start,
                line_end,
            });
        } else {
            i += 1;
        }
    }
    mentions
}

/// Parse `#L10` or `#L10-20` suffix from a mention token.
///
/// Regex: `^([^#]+)(?:#L(\d+)(?:-(\d+))?)?(?:#[^#]*)?$`
/// — strips trailing non-`#L` fragments such as `#heading`.
///
/// Returns `(path_without_fragment, line_start, line_end)`. When only
/// `#L10` is present, `line_end` is set to `Some(line_start)` so a
/// single-line mention reads exactly that line (`lineEnd ?? lineStart`).
fn parse_line_range(mention: &str) -> (&str, Option<i32>, Option<i32>) {
    let Some(hash_pos) = mention.find('#') else {
        return (mention, None, None);
    };
    // Requires at least one non-`#` char before the fragment.
    // `#sym` (leading `#`) is not a fragment, it's a symbol mention;
    // preserve the whole token.
    if hash_pos == 0 {
        return (mention, None, None);
    }
    let path = &mention[..hash_pos];
    let fragment = &mention[hash_pos + 1..]; // after the first `#`

    // `#L<digits>` or `#L<digits>-<digits>` line-range fragment.
    if let Some(rest) = fragment.strip_prefix('L') {
        let (start_str, end_str) = match rest.find('-') {
            Some(dash) => (&rest[..dash], Some(&rest[dash + 1..])),
            None => (rest, None),
        };
        if let Ok(start) = start_str.parse::<i32>() {
            let end = match end_str {
                Some(s) => s.parse::<i32>().ok().or(Some(start)),
                // `lineEnd` defaults to `lineStart` for single-line.
                None => Some(start),
            };
            return (path, Some(start), end);
        }
    }

    // Non-`#L` fragment (e.g. `#heading`) — strip and return path only.
    (path, None, None)
}

/// Strip a trailing ` (agent)` suffix from an agent mention's text.
/// Returns the bare type without the suffix.
fn strip_agent_suffix(text: &str) -> String {
    text.strip_suffix(" (agent)").unwrap_or(text).to_string()
}

/// Try to parse `server:uri` (without `://`) as an MCP resource mention.
/// Returns `Some((server, uri))` when:
///   - `mention` contains exactly one `:` (no `://`)
///   - both sides are non-empty
fn parse_mcp_resource(mention: &str) -> Option<(&str, &str)> {
    if mention.contains("://") {
        return None;
    }
    let colon = mention.find(':')?;
    let server = &mention[..colon];
    let uri = &mention[colon + 1..];
    if server.is_empty() || uri.is_empty() {
        return None;
    }
    // Server must be a plain identifier (no dots/slashes) so that paths
    // like `src/main.rs` aren't misclassified — they don't contain `:`.
    if server.contains('/') || server.contains('.') {
        return None;
    }
    Some((server, uri))
}

#[cfg(test)]
#[path = "user_input.test.rs"]
mod tests;
