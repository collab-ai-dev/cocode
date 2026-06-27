//! Sed in-place edit command parser.
//!
//! Parses `sed -i 's/pattern/replacement/flags' file` commands.

/// Parsed sed edit information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SedEditInfo {
    /// The file being edited.
    pub file_path: String,
    /// Search pattern (regex).
    pub pattern: String,
    /// Replacement string.
    pub replacement: String,
    /// Substitution flags (g, i, etc.).
    pub flags: String,
    /// Whether -E or -r flag was used (extended regex).
    pub extended_regex: bool,
}

/// Check if a command is a sed in-place edit.
pub fn is_sed_in_place_edit(command: &str) -> bool {
    let trimmed = command.trim();
    trimmed.starts_with("sed ") && (trimmed.contains(" -i") || trimmed.contains(" --in-place"))
}

/// Parse a sed in-place edit command.
///
/// Returns `None` if the command is not a valid simple sed -i edit.
pub fn parse_sed_edit_command(command: &str) -> Option<SedEditInfo> {
    let trimmed = command.trim();
    if !trimmed.starts_with("sed ") {
        return None;
    }

    let args: Vec<String> = split_shell_args(&trimmed["sed ".len()..]);
    if args.is_empty() {
        return None;
    }

    let mut has_in_place = false;
    let mut extended_regex = false;
    let mut expression: Option<String> = None;
    let mut file_path: Option<String> = None;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "-i" | "--in-place" => {
                has_in_place = true;
                if i + 1 < args.len() && args[i + 1].starts_with('.') {
                    i += 1; // skip backup suffix
                }
            }
            "-E" | "-r" => {
                extended_regex = true;
            }
            "-e" | "--expression" => {
                if i + 1 < args.len() {
                    i += 1;
                    expression = Some(args[i].clone());
                }
            }
            _ if arg.starts_with('-') => {
                // Unknown flag — skip
            }
            _ => {
                if expression.is_none() {
                    expression = Some(args[i].clone());
                } else if file_path.is_none() {
                    file_path = Some(args[i].clone());
                }
            }
        }
        i += 1;
    }

    if !has_in_place {
        return None;
    }

    let expr = expression?;
    let file = file_path?;

    // Parse the substitution expression: s/pattern/replacement/flags
    let sub = parse_substitution(&expr)?;

    Some(SedEditInfo {
        file_path: file,
        pattern: sub.0,
        replacement: sub.1,
        flags: sub.2,
        extended_regex,
    })
}

/// Parse a sed substitution expression: s/pattern/replacement/flags
fn parse_substitution(expr: &str) -> Option<(String, String, String)> {
    let bytes = expr.as_bytes();
    if bytes.is_empty() || bytes[0] != b's' {
        return None;
    }
    if bytes.len() < 2 {
        return None;
    }

    let delimiter = bytes[1] as char;
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut i = 2;

    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            // Escaped character
            current.push(bytes[i] as char);
            current.push(bytes[i + 1] as char);
            i += 2;
        } else if bytes[i] as char == delimiter {
            parts.push(std::mem::take(&mut current));
            i += 1;
        } else {
            current.push(bytes[i] as char);
            i += 1;
        }
    }

    // Remaining text after last delimiter is flags
    if parts.len() < 2 {
        return None;
    }

    let pattern = parts[0].clone();
    let replacement = parts[1].clone();
    let flags = if !current.is_empty() {
        current
    } else if parts.len() > 2 {
        parts[2].clone()
    } else {
        String::new()
    };

    // Validate flags
    if !flags
        .chars()
        .all(|c| matches!(c, 'g' | 'p' | 'i' | 'I' | 'm' | 'M' | '1'..='9'))
    {
        return None;
    }

    Some((pattern, replacement, flags))
}

/// Whether a `sed` command contains an operation requiring approval: a shell
/// **execute** (`e` command / `s///e` flag — always dangerous) or a file
/// **write/read** (`w`/`W`/`r`/`R` command, `s///w` flag), the latter only when
/// `allow_file_writes` is false (in acceptEdits, in-place edits are expected).
///
/// Conservative superset — may over-prompt on exotic scripts but never
/// under-approves a dangerous one.
pub fn has_dangerous_sed(command: &str, allow_file_writes: bool) -> bool {
    for sub in crate::bash_permissions::split_compound_command(command) {
        let trimmed = sub.trim();
        if crate::mode_validation::extract_base_executable(trimmed) != "sed" {
            continue;
        }
        for script in sed_scripts(trimmed) {
            if sed_script_executes(&script) {
                return true;
            }
            if !allow_file_writes && sed_script_writes(&script) {
                return true;
            }
        }
    }
    false
}

/// Collect the sed scripts: every `-e`/`--expression` value plus the first bare
/// positional (the implicit script). Leading env-vars + the `sed` token are
/// stripped first.
fn sed_scripts(command: &str) -> Vec<String> {
    let stripped =
        crate::bash_permissions::strip_all_env_vars(command.trim(), /*check_hijack*/ false);
    let rest = stripped
        .split_once(char::is_whitespace)
        .map(|(_, r)| r)
        .unwrap_or("");
    // Quote-aware: a sed script like `'e rm -rf /'` or `'s/a/b/w /tmp/o'` holds
    // spaces inside quotes — a naive whitespace split would shred the `e`/`w`
    // command out of view.
    let args = split_args_quote_aware(rest);
    let mut scripts = Vec::new();
    let mut took_positional = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-e" | "--expression" => {
                if i + 1 < args.len() {
                    i += 1;
                    scripts.push(args[i].clone());
                }
            }
            "-f" | "--file" => i += 1, // external script file — name skipped
            "-i" | "--in-place" => {
                if i + 1 < args.len() && args[i + 1].starts_with('.') {
                    i += 1; // backup suffix
                }
            }
            _ if a.starts_with('-') => {}
            _ => {
                if !took_positional {
                    took_positional = true;
                    scripts.push(args[i].clone());
                }
            }
        }
        i += 1;
    }
    scripts
}

/// Flags of a substitution `s<d>pat<d>rep<d>FLAGS` (chars after the closing
/// delimiter), if `script` is a substitution; else `None`.
fn substitution_flags(script: &str) -> Option<String> {
    let bytes = script.as_bytes();
    if bytes.first() != Some(&b's') || bytes.len() < 2 {
        return None;
    }
    let delim = bytes[1];
    let mut seen = 0;
    let mut i = 2;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == delim {
            seen += 1;
            if seen == 2 {
                return Some(
                    script[i + 1..]
                        .chars()
                        .take_while(char::is_ascii_alphanumeric)
                        .collect(),
                );
            }
        }
        i += 1;
    }
    None
}

fn sed_script_executes(script: &str) -> bool {
    if let Some(flags) = substitution_flags(script)
        && flags.contains('e')
    {
        return true;
    }
    sed_command_letters(script).contains(&'e')
}

fn sed_script_writes(script: &str) -> bool {
    if let Some(flags) = substitution_flags(script)
        && (flags.contains('w') || flags.contains('W'))
    {
        return true;
    }
    sed_command_letters(script)
        .iter()
        .any(|c| matches!(c, 'w' | 'W' | 'r' | 'R'))
}

/// The leading command letter of each `;`/newline-separated sed statement,
/// after skipping an optional address (`/regex/`, `$`, line numbers, `,`, `!`).
fn sed_command_letters(script: &str) -> Vec<char> {
    let mut letters = Vec::new();
    for stmt in script.split([';', '\n']) {
        let bytes = stmt.trim_start().as_bytes();
        let mut i = 0;
        if bytes.first() == Some(&b'/') {
            i = 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                    continue;
                }
                if bytes[i] == b'/' {
                    i += 1;
                    break;
                }
                i += 1;
            }
        }
        while i < bytes.len() && matches!(bytes[i], b'0'..=b'9' | b'$' | b',' | b' ' | b'!') {
            i += 1;
        }
        if let Some(&c) = bytes.get(i) {
            letters.push(c as char);
        }
    }
    letters
}

/// Shell argument splitting with basic quote stripping.
fn split_shell_args(command: &str) -> Vec<String> {
    command.split_whitespace().map(strip_quotes).collect()
}

/// Quote-aware shell-arg split: respects single/double quotes (keeping a quoted
/// span — including its inner spaces — as one token) and strips the surrounding
/// quotes. Sufficient for sed script extraction, where the script body must
/// survive intact to inspect its command letters.
fn split_args_quote_aware(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut in_token = false;
    for c in s.chars() {
        match c {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            c if c.is_whitespace() && !in_single && !in_double => {
                if in_token {
                    args.push(std::mem::take(&mut cur));
                    in_token = false;
                }
                continue;
            }
            c => cur.push(c),
        }
        in_token = true;
    }
    if in_token {
        args.push(cur);
    }
    args
}

/// Strip surrounding single or double quotes from an argument.
fn strip_quotes(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
#[path = "sed_parser.test.rs"]
mod tests;
