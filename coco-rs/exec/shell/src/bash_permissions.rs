//! Bash command permission checking pipeline.
//!
//! Implements wrapper stripping, env var filtering, command prefix extraction,
//! and compound command splitting for the permission rule matching system.

use std::collections::HashSet;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Safe environment variables — no code execution or library loading risk.
static SAFE_ENV_VARS: LazyLock<HashSet<&str>> = LazyLock::new(|| {
    [
        // Go
        "GOEXPERIMENT",
        "GOOS",
        "GOARCH",
        "CGO_ENABLED",
        "GO111MODULE",
        // Rust
        "RUST_BACKTRACE",
        "RUST_LOG",
        "CARGO_TERM_COLOR",
        // Node
        "NODE_ENV",
        "NODE_OPTIONS",
        // Python
        "PYTHONUNBUFFERED",
        "PYTHONDONTWRITEBYTECODE",
        "VIRTUAL_ENV",
        // Pytest, locale, terminal
        "PYTEST_DISABLE_PLUGIN_AUTOLOAD",
        "LANG",
        "LANGUAGE",
        "LC_ALL",
        "TERM",
        // Colors
        "NO_COLOR",
        "FORCE_COLOR",
        "LS_COLORS",
        "LSCOLORS",
        "GREP_COLORS",
        // Other
        "TZ",
        "CHARSET",
        "COLUMNS",
        "LINES",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
    ]
    .into_iter()
    .collect()
});

/// Binary hijack variables — env vars that can redirect binary execution.
const BINARY_HIJACK_PATTERNS: &[&str] = &["LD_", "DYLD_", "PATH"];

/// Dangerous bare shell prefixes — never allow bare-prefix rules for these.
static BARE_SHELL_PREFIXES: LazyLock<HashSet<&str>> = LazyLock::new(|| {
    [
        "sh",
        "bash",
        "zsh",
        "fish",
        "csh",
        "tcsh",
        "ksh",
        "dash",
        "cmd",
        "powershell",
        "pwsh",
        "env",
        "xargs",
        "nice",
        "stdbuf",
        "nohup",
        "timeout",
        "time",
        "sudo",
        "doas",
        "pkexec",
    ]
    .into_iter()
    .collect()
});

/// Strip safe wrapper commands from a command string.
///
/// Two-phase stripping:
/// Phase 1: Strip leading safe env vars + comments
/// Phase 2: Strip wrapper commands (timeout, time, nice, stdbuf, nohup)
///
/// Returns the inner command after stripping.
pub fn strip_safe_wrappers(command: &str) -> String {
    let mut result = command.trim().to_string();

    // Phase 1: Strip leading safe env vars
    loop {
        let before = result.clone();
        result = strip_one_safe_env_var(&result);
        if result == before {
            break;
        }
    }

    // Phase 2: Strip wrapper commands
    loop {
        let before = result.clone();
        result = strip_one_wrapper(&result);
        if result == before {
            break;
        }
    }

    result.trim().to_string()
}

/// Strip all leading env var assignments (safe or not).
///
/// Used for deny rule matching. Optionally checks against a blocklist of hijack variables.
pub fn strip_all_env_vars(command: &str, check_hijack: bool) -> String {
    let mut rest = command.trim();

    while let Some((var_name, after)) = try_parse_env_assignment(rest) {
        if check_hijack && is_binary_hijack_var(var_name) {
            break;
        }
        rest = after.trim_start();
    }

    rest.to_string()
}

/// Extract a stable `command subcommand` prefix for a permission suggestion
/// (e.g., `git commit` from `git commit -m msg`).
///
/// Splits on whitespace, skips leading SAFE env-var assignments (bails on the
/// first unsafe one), then requires the second token to be shaped like a
/// subcommand.
///
/// Returns `None` (caller falls back to an exact rule) when the command is a
/// single bare word, when the second token is a flag / path / filename / number
/// rather than a subcommand, or when an unsafe leading env assignment is present
/// (a prefix keyed on a specific env value is a poor rule).
///
/// Unlike the allow-rule matcher, this does NOT strip safe *wrappers*
/// (`timeout`/`nice`/`nohup`/…) — only env assignments are skipped here. A
/// wrapper command therefore becomes the first token: `nohup npm start` →
/// `nohup npm`, `timeout 60 cargo test` → `None` (the duration `60` is not a
/// subcommand). The bare-shell guard lives in [`get_first_word_prefix`], not here.
pub fn get_command_prefix(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();

    // Skip leading SAFE env-var assignments; bail on the first unsafe one.
    let mut i = 0;
    while let Some(name) = tokens.get(i).copied().and_then(env_assignment_name) {
        if !SAFE_ENV_VARS.contains(name) {
            return None;
        }
        i += 1;
    }

    let cmd = *tokens.get(i)?;
    // Second token must be shaped like a subcommand (`commit`, `run`,
    // `force-push`), not a flag (`-rf`), path (`/tmp`), filename (`a.txt`), or
    // number (`755`).
    let subcmd = *tokens.get(i + 1)?;
    if !looks_like_subcommand(subcmd) {
        return None;
    }
    Some(format!("{cmd} {subcmd}"))
}

/// Extract a single-word command prefix (e.g., `python3` from
/// `python3 script.py`) for the editable-prefix field's default value.
///
/// UI-only fallback used to seed the dialog's editable prefix input when
/// [`get_command_prefix`] declines (the second token isn't a subcommand). It
/// skips leading SAFE env assignments (bail on unsafe), requires the command
/// word to be a clean lowercase name (rejects paths, flags, numbers), and —
/// unlike `get_command_prefix` — rejects [`BARE_SHELL_PREFIXES`]: a bare
/// `bash:*` / `sudo:*` / `env:*` rule would allow arbitrary code via `-c` or by
/// wrapping, so the dialog falls back to suggesting the exact command instead.
pub fn get_first_word_prefix(command: &str) -> Option<String> {
    let tokens: Vec<&str> = command.split_whitespace().collect();

    let mut i = 0;
    while let Some(name) = tokens.get(i).copied().and_then(env_assignment_name) {
        if !SAFE_ENV_VARS.contains(name) {
            return None;
        }
        i += 1;
    }

    let cmd = *tokens.get(i)?;
    // Same shape check as the subcommand test: rejects paths (`./x`,
    // `/usr/bin/python`), flags, numbers, filenames.
    if !looks_like_subcommand(cmd) {
        return None;
    }
    if BARE_SHELL_PREFIXES.contains(cmd) {
        return None;
    }
    Some(cmd.to_string())
}

/// Extract a stable prefix from the words before a `<<` heredoc operator, or
/// `None` when the command has no heredoc / nothing usable precedes it. A
/// heredoc body changes every call, so an exact rule would never re-match — the
/// caller suggests this prefix instead.
pub fn heredoc_command_prefix(command: &str) -> Option<String> {
    let idx = command.find("<<")?;
    if idx == 0 {
        return None;
    }
    let before = command[..idx].trim();
    if before.is_empty() {
        return None;
    }
    if let Some(prefix) = get_command_prefix(before) {
        return Some(prefix);
    }
    // Fallback: skip safe env assignments, then take up to 2 tokens (preserves a
    // flag like `python3 -c`). An unsafe leading env var yields no prefix.
    let tokens: Vec<&str> = before.split_whitespace().collect();
    let mut i = 0;
    while let Some(name) = tokens.get(i).copied().and_then(env_assignment_name) {
        if !SAFE_ENV_VARS.contains(name) {
            return None;
        }
        i += 1;
    }
    let rest = tokens.get(i..)?;
    if rest.is_empty() {
        return None;
    }
    Some(rest.iter().take(2).copied().collect::<Vec<_>>().join(" "))
}

/// If `token` is a `NAME=value` env assignment (`^[A-Za-z_]\w*=`), return `NAME`.
fn env_assignment_name(token: &str) -> Option<&str> {
    let eq = token.find('=')?;
    let name = &token[..eq];
    (!name.is_empty()
        && name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(name)
}

/// Whether `token` has the subcommand shape `^[a-z][a-z0-9]*(-[a-z0-9]+)*$` —
/// a lowercase word with optional `-segments` (`commit`, `run`, `force-push`).
fn looks_like_subcommand(token: &str) -> bool {
    let bytes = token.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_lowercase() {
        return false;
    }
    let mut prev_dash = false;
    for &b in &bytes[1..] {
        match b {
            b'-' => {
                if prev_dash {
                    return false; // no `--`
                }
                prev_dash = true;
            }
            b'a'..=b'z' | b'0'..=b'9' => prev_dash = false,
            _ => return false,
        }
    }
    !prev_dash // no trailing dash
}

/// Split a compound command into subcommands.
///
/// Splits on &&, ||, ;, | operators (basic — does not handle quoting fully).
pub fn split_compound_command(command: &str) -> Vec<String> {
    let mut subcommands = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                current.push(c);
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
                current.push(c);
            }
            '&' if !in_single_quote && !in_double_quote => {
                if chars.peek() == Some(&'&') {
                    chars.next();
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        subcommands.push(trimmed);
                    }
                    current.clear();
                } else {
                    current.push(c);
                }
            }
            '|' if !in_single_quote && !in_double_quote => {
                if chars.peek() == Some(&'|') {
                    chars.next();
                }
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    subcommands.push(trimmed);
                }
                current.clear();
            }
            ';' if !in_single_quote && !in_double_quote => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    subcommands.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }

    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        subcommands.push(trimmed);
    }

    subcommands
}

/// Static summary of a shell compound command for permission decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundCommandAnalysis {
    /// Raw split subcommands, preserving user text.
    pub subcommands: Vec<String>,
    /// Subcommands after filtering exact no-op cwd changes like `cd <cwd>`.
    pub significant_subcommands: Vec<String>,
    /// Count of all cwd-changing subcommands (`cd`, `pushd`, `popd`).
    pub cwd_change_count: usize,
    /// Count of cwd-changing subcommands that are not exact no-ops.
    pub non_noop_cwd_change_count: usize,
    /// Whether any significant subcommand invokes `git`.
    pub has_git: bool,
}

impl CompoundCommandAnalysis {
    pub fn has_non_noop_cwd_change(&self) -> bool {
        self.non_noop_cwd_change_count > 0
    }

    pub fn has_multiple_cwd_changes(&self) -> bool {
        self.non_noop_cwd_change_count > 1
    }
}

/// Analyze compound command structure after env/wrapper normalization.
///
/// This mirrors the TS permission posture: exact no-op `cd <current cwd>`
/// segments are ignored before deciding whether a later `git` command is
/// really a cwd+git escape pattern or just a scoped prefix.
pub fn analyze_compound_command(command: &str, cwd: &str) -> CompoundCommandAnalysis {
    let subcommands = split_compound_command(command);
    let mut significant_subcommands = Vec::new();
    let mut cwd_change_count = 0;
    let mut non_noop_cwd_change_count = 0;
    let mut has_git = false;

    for sub in &subcommands {
        let normalized = normalize_subcommand_for_analysis(sub);
        let base = crate::mode_validation::extract_base_executable(&normalized);
        if base == "git" {
            has_git = true;
        }

        let is_cwd_change = matches!(base, "cd" | "pushd" | "popd");
        if is_cwd_change {
            cwd_change_count += 1;
            if is_noop_cwd_change(base, &normalized, cwd) {
                continue;
            }
            non_noop_cwd_change_count += 1;
        }
        significant_subcommands.push(sub.trim().to_string());
    }

    CompoundCommandAnalysis {
        subcommands,
        significant_subcommands,
        cwd_change_count,
        non_noop_cwd_change_count,
        has_git,
    }
}

fn normalize_subcommand_for_analysis(subcommand: &str) -> String {
    strip_safe_wrappers(&strip_all_env_vars(
        subcommand.trim(),
        /*check_hijack*/ false,
    ))
}

fn is_noop_cwd_change(base: &str, command: &str, cwd: &str) -> bool {
    if base != "cd" {
        return false;
    }
    let argv = command_argv(command);
    let Some(target) = argv.get(1) else {
        return false;
    };
    if target == "-" || target.starts_with('-') {
        return false;
    }
    cd_target_is_current_cwd(target, cwd)
}

fn command_argv(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|t| t.trim_matches(['\'', '"']).to_string())
        .collect()
}

fn cd_target_is_current_cwd(target: &str, cwd: &str) -> bool {
    let current = normalize_path_lexically(Path::new(cwd));
    let target_path = if target == "~" || target.starts_with("~/") {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(home) = home else {
            return false;
        };
        if target == "~" {
            home
        } else {
            home.join(target.trim_start_matches("~/"))
        }
    } else {
        let path = Path::new(target);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            Path::new(cwd).join(path)
        }
    };
    normalize_path_lexically(&target_path) == current
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let popped = out.pop();
                if !popped {
                    out.push(component.as_os_str());
                }
            }
            Component::RootDir | Component::Prefix(_) | Component::Normal(_) => {
                out.push(component.as_os_str());
            }
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

/// Remove unquoted output-redirection clauses (`> file`, `>> file`, `2> file`,
/// `&> file`, `2>&1`, `>&2`, `>&-`, …) from a command.
///
/// Used to build rule-matching candidates so e.g. `Bash(python:*)` matches
/// `python s.py > out.txt`. Quote-guarded: redirections inside single/double
/// quotes are preserved. Heredoc/subshell parsing is deliberately NOT
/// attempted — redirection *target* security lives in path validation.
pub fn strip_output_redirections(command: &str) -> String {
    if !command.contains('>') {
        return command.trim().to_string();
    }
    let chars: Vec<char> = command.chars().collect();
    let mut out = String::with_capacity(command.len());
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' && !in_double {
            in_single = !in_single;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            out.push(c);
            i += 1;
            continue;
        }
        if !in_single && !in_double {
            let next_is_gt = chars.get(i + 1) == Some(&'>');
            // `>`, `>>`, `&>`, `2>`, `2>>`, or a dup form like `2>&1`.
            let is_redir_start =
                c == '>' || (c == '&' && next_is_gt) || (c.is_ascii_digit() && next_is_gt);
            if is_redir_start {
                let mut j = i;
                if c != '>' {
                    j += 1; // skip leading `&` or FD digit; chars[j] is now `>`
                }
                j += 1; // past first `>`
                if chars.get(j) == Some(&'>') {
                    j += 1; // `>>`
                }
                if chars.get(j) == Some(&'&') {
                    // dup form `>&N` / `>&-` — no separate file token
                    j += 1;
                    while chars.get(j).is_some_and(char::is_ascii_digit) {
                        j += 1;
                    }
                    if chars.get(j) == Some(&'-') {
                        j += 1;
                    }
                } else {
                    // skip whitespace, then drop the target token
                    while matches!(chars.get(j), Some(' ' | '\t')) {
                        j += 1;
                    }
                    while j < chars.len()
                        && !chars[j].is_whitespace()
                        && !matches!(chars[j], '>' | '<' | '|' | ';' | '&')
                    {
                        j += 1;
                    }
                }
                i = j;
                if !out.ends_with(' ') {
                    out.push(' ');
                }
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    // Collapse the whitespace left behind by removed clauses.
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Extract the file targets of output redirections (`>`, `>>`, `>|`, `&>`,
/// `&>>`, `N>`, `N>>`, and the deprecated `>&file` / `N>&file` forms), skipping
/// file-descriptor *duplication* forms (`2>&1`, `>&-`) which have no file token.
///
/// Quote-aware: redirections inside single/double quotes are ignored, the same
/// way [`strip_output_redirections`] guards them. `/dev/null` is NOT filtered
/// here — the caller decides which targets are safe.
///
/// Returned tokens are raw (may still contain shell-expansion syntax); callers
/// in `coco-tools` apply `coco_permissions::has_shell_expansion` /
/// `is_path_within_allowed_dirs`. Lives here (not `coco-permissions`) because
/// `coco-shell` must not depend on `coco-permissions` (would cycle).
pub fn extract_output_redirect_targets(command: &str) -> Vec<String> {
    let mut targets = Vec::new();
    if !command.contains('>') {
        return targets;
    }
    let chars: Vec<char> = command.chars().collect();
    let mut in_single = false;
    let mut in_double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if !in_single && !in_double {
            let next_is_gt = chars.get(i + 1) == Some(&'>');
            // `>`, `>>`, `&>`, `2>`, `2>>`, or a dup form like `2>&1`.
            let is_redir_start =
                c == '>' || (c == '&' && next_is_gt) || (c.is_ascii_digit() && next_is_gt);
            if is_redir_start {
                let mut j = i;
                if c != '>' {
                    j += 1; // skip leading `&` or FD digit; chars[j] is now `>`
                }
                j += 1; // past first `>`
                if chars.get(j) == Some(&'>') {
                    j += 1; // `>>`
                }
                if chars.get(j) == Some(&'|') {
                    j += 1; // `>|` clobber
                }
                if chars.get(j) == Some(&'&') {
                    // `>&…`: a dup (`>&1`, `>&-`) has no file token; the
                    // deprecated `>&file` (non-digit, non-`-`) IS a file target.
                    let after_amp = j + 1;
                    let is_dup = chars.get(after_amp).is_some_and(char::is_ascii_digit)
                        || chars.get(after_amp) == Some(&'-');
                    if is_dup {
                        j = after_amp;
                        while chars.get(j).is_some_and(char::is_ascii_digit) {
                            j += 1;
                        }
                        if chars.get(j) == Some(&'-') {
                            j += 1;
                        }
                        i = j;
                        continue;
                    }
                    j = after_amp; // deprecated `>&file`: read the file token below
                } else {
                    while matches!(chars.get(j), Some(' ' | '\t')) {
                        j += 1;
                    }
                }
                let start = j;
                while j < chars.len()
                    && !chars[j].is_whitespace()
                    && !matches!(chars[j], '>' | '<' | '|' | ';' | '&' | '(' | ')')
                {
                    j += 1;
                }
                if j > start {
                    targets.push(chars[start..j].iter().collect());
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    targets
}

/// Detect process substitution: input `<(cmd)` or a redirect to output process
/// substitution `> >(cmd)`, allowing whitespace between operators. Quote-naive.
/// Such commands can execute arbitrary commands whose writes never appear as
/// redirect targets, so the caller forces an Ask.
pub fn has_process_substitution(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let n = chars.len();
    let skip_ws = |mut k: usize| {
        while k < n && matches!(chars.get(k), Some(' ' | '\t')) {
            k += 1;
        }
        k
    };
    let mut i = 0;
    while i < n {
        match chars[i] {
            // `<\s*\(` — input process substitution.
            '<' if chars.get(skip_ws(i + 1)) == Some(&'(') => return true,
            // `>\s*>\s*\(` — redirect to an output process substitution.
            '>' => {
                let j = skip_ws(i + 1);
                if chars.get(j) == Some(&'>') && chars.get(skip_ws(j + 1)) == Some(&'(') {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Check if a command prefix is a dangerous bare shell prefix.
pub fn is_dangerous_bare_prefix(prefix: &str) -> bool {
    BARE_SHELL_PREFIXES.contains(prefix)
}

/// Check if an env var name is a binary hijack variable.
fn is_binary_hijack_var(name: &str) -> bool {
    BINARY_HIJACK_PATTERNS.iter().any(|&pat| {
        if pat.ends_with('_') {
            name.starts_with(pat)
        } else {
            name == pat
        }
    })
}

/// Try to strip one safe env var assignment from the front of the command.
fn strip_one_safe_env_var(command: &str) -> String {
    let trimmed = command.trim_start();
    if let Some((var_name, after)) = try_parse_env_assignment(trimmed)
        && SAFE_ENV_VARS.contains(var_name)
    {
        return after.trim_start().to_string();
    }
    command.to_string()
}

type WrapperStripper = fn(&str) -> Option<&str>;

/// Try to strip one wrapper command from the front.
fn strip_one_wrapper(command: &str) -> String {
    let trimmed = command.trim_start();

    let wrappers: &[(&str, WrapperStripper)] = &[
        ("timeout ", strip_timeout_args),
        ("time ", strip_simple_wrapper),
        ("nice ", strip_nice_args),
        ("stdbuf ", strip_stdbuf_args),
        ("nohup ", strip_simple_wrapper),
    ];

    for &(prefix, stripper) in wrappers {
        if let Some(after_prefix) = trimmed.strip_prefix(prefix)
            && let Some(rest) = stripper(after_prefix)
        {
            return rest.to_string();
        }
    }

    command.to_string()
}

/// Try to parse "VAR=value " at the start, returning (var_name, rest).
fn try_parse_env_assignment(s: &str) -> Option<(&str, &str)> {
    let eq_pos = s.find('=')?;
    let name = &s[..eq_pos];

    // Name must be valid identifier
    if name.is_empty()
        || !name.chars().next().unwrap_or('0').is_ascii_alphabetic()
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }

    // Find end of value (next unquoted whitespace)
    let rest = &s[eq_pos + 1..];
    let value_end = find_value_end(rest);
    let after = &rest[value_end..];

    if after.is_empty() || after.starts_with(char::is_whitespace) {
        Some((name, after.trim_start()))
    } else {
        None
    }
}

fn find_value_end(s: &str) -> usize {
    let mut i = 0;
    let bytes = s.as_bytes();

    if i < bytes.len() && bytes[i] == b'\'' {
        // Single-quoted value
        i += 1;
        while i < bytes.len() && bytes[i] != b'\'' {
            i += 1;
        }
        if i < bytes.len() {
            i += 1;
        }
    } else if i < bytes.len() && bytes[i] == b'"' {
        // Double-quoted value
        i += 1;
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' {
                i += 1;
            }
            i += 1;
        }
        if i < bytes.len() {
            i += 1;
        }
    } else {
        // Unquoted value
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    i
}

fn strip_timeout_args(rest: &str) -> Option<&str> {
    // Skip timeout flags until we hit the actual command
    let mut remaining = rest.trim_start();
    // Skip optional flags like --signal=KILL, -k 5, etc.
    while remaining.starts_with('-') || remaining.starts_with("--") {
        let next_space = remaining.find(' ')?;
        remaining = remaining[next_space..].trim_start();
        // If the flag takes an argument (e.g., -k 5), skip that too
        if !remaining.starts_with('-')
            && remaining.chars().next().is_some_and(|c| c.is_ascii_digit())
            && let Some(ns) = remaining.find(' ')
        {
            remaining = remaining[ns..].trim_start();
        }
    }
    // Skip the duration argument
    if let Some(space) = remaining.find(' ') {
        Some(remaining[space..].trim_start())
    } else {
        None
    }
}

fn strip_simple_wrapper(rest: &str) -> Option<&str> {
    // Just skip "-- " if present, then return the rest
    let trimmed = rest.trim_start();
    if let Some(stripped) = trimmed.strip_prefix("-- ") {
        Some(stripped)
    } else {
        Some(trimmed)
    }
}

fn strip_nice_args(rest: &str) -> Option<&str> {
    let mut remaining = rest.trim_start();
    // Skip -n <priority> or -<digit>
    if remaining.starts_with("-n") {
        remaining = remaining[2..].trim_start();
        if let Some(space) = remaining.find(' ') {
            remaining = remaining[space..].trim_start();
        }
    } else if remaining.starts_with('-')
        && remaining.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
        && let Some(space) = remaining.find(' ')
    {
        remaining = remaining[space..].trim_start();
    }
    if let Some(stripped) = remaining.strip_prefix("-- ") {
        Some(stripped)
    } else {
        Some(remaining)
    }
}

fn strip_stdbuf_args(rest: &str) -> Option<&str> {
    let mut remaining = rest.trim_start();
    // Skip -i/-o/-e flags with values
    while remaining.starts_with('-') && remaining.len() > 1 {
        let flag_char = remaining.as_bytes()[1];
        if matches!(flag_char, b'i' | b'o' | b'e') {
            // Skip flag + value (e.g., -iL or -i0)
            let end = remaining[2..]
                .find(' ')
                .map(|p| p + 2)
                .unwrap_or(remaining.len());
            remaining = remaining[end..].trim_start();
        } else {
            break;
        }
    }
    if let Some(stripped) = remaining.strip_prefix("-- ") {
        Some(stripped)
    } else {
        Some(remaining)
    }
}

#[cfg(test)]
#[path = "bash_permissions.test.rs"]
mod tests;
