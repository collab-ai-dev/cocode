//! `@import` directive parsing for memory files.
//!
//! Memory files can include other files via `@path` syntax in text
//! nodes. This module:
//! - extracts `@path` references from a markdown body, skipping fenced
//!   code blocks and inline code spans (skips code-type AST nodes);
//! - resolves the four syntax variants (`@./rel`, `@~/home`,
//!   `@/abs`, `@bare/path` = relative);
//! - recursively expands includes with [`MAX_INCLUDE_DEPTH`] depth
//!   guard + a per-traversal `processed` set to break cycles;
//! - filters by [`TEXT_FILE_EXTENSIONS`] so binary blobs (images,
//!   PDFs) can't be loaded into the system prompt.
//!
//! ## Ordering
//!
//! The **parent** is pushed first, then children are recursed into:
//! caller appends to a flat `Vec` so the natural `.extend` order
//! matches.
//!
//! ## What we don't (yet) implement
//!
//! - Symlink-target dedup (we dedup by canonicalized path which
//!   collapses symlinks the same way).
//! - `claudeMdExcludes` user-setting filter — that's a P3 follow-up
//!   (settings plumb-through). Files that match the path pattern
//!   simply load.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

/// Maximum recursion depth for `@import` expansion.
pub const MAX_INCLUDE_DEPTH: u32 = 5;

/// Extensions allowed for `@import` targets — prevents binary blobs
/// (images, PDFs, archives) from being loaded into the system prompt.
///
/// A representative subset; missing entries can be added as needed.
/// All matches are case-insensitive on the extension only.
pub const TEXT_FILE_EXTENSIONS: &[&str] = &[
    // Markdown / text
    "md",
    "txt",
    "text",
    "rst",
    "adoc",
    "asciidoc",
    "org",
    // Data formats
    "json",
    "yaml",
    "yml",
    "toml",
    "xml",
    "csv",
    // Web
    "html",
    "htm",
    "css",
    "scss",
    "sass",
    "less",
    // JavaScript / TypeScript
    "js",
    "ts",
    "tsx",
    "jsx",
    "mjs",
    "cjs",
    "mts",
    "cts",
    // Python / Ruby / Go / Rust
    "py",
    "pyi",
    "pyw",
    "rb",
    "erb",
    "rake",
    "go",
    "rs",
    // Java / Kotlin / Scala / C / C++ / C#
    "java",
    "kt",
    "kts",
    "scala",
    "c",
    "cpp",
    "cc",
    "cxx",
    "h",
    "hpp",
    "hxx",
    "cs",
    // Swift / Shell
    "swift",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    // Config
    "env",
    "ini",
    "cfg",
    "conf",
    "config",
    "properties",
    // Database / protocol / frontend
    "sql",
    "graphql",
    "gql",
    "proto",
    "vue",
    "svelte",
    "astro",
    // Lock / docs / misc
    "lock",
    "log",
    "diff",
    "patch",
    "tex",
    "latex",
];

/// Extract `@path` references from a memory-file body, ignoring
/// fenced code blocks and inline code spans.
///
/// Walks the body and only inspects text nodes (skipping code /
/// codespan). The line/region scanner achieves this skip without
/// pulling in a markdown parser dep.
pub fn extract_include_paths(body: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut in_fence = false;
    let mut fence_marker = String::new();

    for line in body.lines() {
        let trimmed = line.trim_start();

        // Toggle fenced state on ``` or ~~~ markers (3+ chars). Match
        // the same character for both open/close so ```rust...``` works
        // and ``` inside a ~~~ block doesn't toggle.
        if let Some(marker_char) = fence_marker_char(trimmed) {
            if !in_fence {
                in_fence = true;
                fence_marker = marker_char.to_string();
                continue;
            } else if trimmed.starts_with(&fence_marker) {
                in_fence = false;
                fence_marker.clear();
                continue;
            }
        }
        if in_fence {
            continue;
        }

        // Strip inline code spans (`...`) before scanning for @paths.
        let cleaned = strip_inline_code(line);
        for path in scan_at_paths(&cleaned) {
            paths.push(path);
        }
    }

    paths
}

/// Returns the fence character (' ` ' or ' ~ ') if `line` opens or
/// closes a code fence, else None.
fn fence_marker_char(line: &str) -> Option<char> {
    if line.starts_with("```") {
        Some('`')
    } else if line.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

/// Replace text inside `` `...` `` with spaces so subsequent regex
/// scans never see content protected by inline backticks.
fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for ch in line.chars() {
        if ch == '`' {
            in_code = !in_code;
            out.push(' ');
        } else if in_code {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Find all `@path` substrings in `text`, returning each path string
/// (without the leading `@`). Validated forms:
/// - `./relative`
/// - `~/home/path`
/// - `/absolute/path` (single `/` rejected)
/// - bare `name` not starting with `@` or punctuation
fn scan_at_paths(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'@' {
            i += 1;
            continue;
        }
        // The `@` must start a token: previous char is whitespace, BOL,
        // or punctuation. Avoids matching `email@example.com`.
        if i > 0 {
            let prev = bytes[i - 1];
            let is_separator = prev.is_ascii_whitespace()
                || prev == b'('
                || prev == b'['
                || prev == b'{'
                || prev == b'<'
                || prev == b','
                || prev == b';'
                || prev == b'>';
            if !is_separator {
                i += 1;
                continue;
            }
        }
        // Consume the path: any char until whitespace or terminating
        // markdown punctuation. Permissive token shape — refined by the
        // validator below.
        let start = i + 1;
        let mut end = start;
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_whitespace()
                || matches!(c, b')' | b']' | b'}' | b'>' | b',' | b';' | b'`')
            {
                break;
            }
            end += 1;
        }
        let path = &text[start..end];
        // Strip a trailing `#fragment` explicitly so the file-resolver
        // doesn't try to open `path#section`.
        let path_clean = path.split('#').next().unwrap_or(path);
        if is_valid_at_path(path_clean) {
            out.push(path_clean.to_string());
        }
        i = end;
    }
    out
}

/// Validation for an `@path` token.
fn is_valid_at_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    if path.starts_with("./") {
        return true;
    }
    if path.starts_with("~/") {
        return true;
    }
    if path.starts_with('/') && path != "/" {
        return true;
    }
    // Bare token: must not start with `@` or pure punctuation, and
    // first char must be alphanumeric / `.`/ `_`/ `-`.
    let first = path.as_bytes()[0];
    if first == b'@' {
        return false;
    }
    let valid_first = first.is_ascii_alphanumeric() || matches!(first, b'.' | b'_' | b'-');
    if !valid_first {
        return false;
    }
    true
}

/// Resolve an `@path` token to an absolute filesystem path, given the
/// directory of the file that contains the directive.
///
/// - `~/...` ⇒ relative to `$HOME`.
/// - `/...` ⇒ used as-is.
/// - `./...` or bare ⇒ relative to `base_dir`.
pub fn resolve_at_path(token: &str, base_dir: &Path) -> Option<PathBuf> {
    if let Some(rest) = token.strip_prefix("~/") {
        let home = std::env::var("HOME").ok().map(PathBuf::from)?;
        return Some(home.join(rest));
    }
    if token.starts_with('/') {
        return Some(PathBuf::from(token));
    }
    let rel = token.strip_prefix("./").unwrap_or(token);
    Some(base_dir.join(rel))
}

/// True when `path`'s extension is in [`TEXT_FILE_EXTENSIONS`]
/// (case-insensitive). Files with no extension are accepted (markdown
/// `README` files etc.).
pub fn is_text_extension(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        // No extension → conservatively allow (the tokenizer doesn't gate
        // on extension at all).
        return true;
    };
    TEXT_FILE_EXTENSIONS
        .iter()
        .any(|e| e.eq_ignore_ascii_case(ext))
}

/// Recursively expand a memory file's `@imports`, returning a flat
/// list of `(path, content)` entries in parent-first order, then each
/// include (recursively).
///
/// `processed` carries the canonicalized paths visited so far; cycles
/// are silently broken by skipping repeats. `depth` is checked against
/// [`MAX_INCLUDE_DEPTH`] — content is still loaded at depth ==
/// MAX_INCLUDE_DEPTH but its own includes aren't recursed into.
pub fn expand_imports(
    path: &Path,
    content: &str,
    processed: &mut HashSet<PathBuf>,
    depth: u32,
    cwd: &Path,
    allow_external: bool,
) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !processed.insert(canonical) {
        return out; // cycle: skip
    }

    // Strip HTML comments so authorial `<!-- … -->` never reaches the model
    // prompt. @imports inside a comment are inactive because extraction runs
    // on the stripped body.
    let stripped = strip_html_comments(content);
    out.push((path.to_path_buf(), stripped.clone()));

    if depth >= MAX_INCLUDE_DEPTH {
        return out;
    }

    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    for token in extract_include_paths(&stripped) {
        let Some(resolved) = resolve_at_path(&token, base_dir) else {
            continue;
        };
        if !is_text_extension(&resolved) {
            continue;
        }
        // Security boundary: an @import resolving OUTSIDE the project cwd is
        // "external" — skip it unless the caller allows external includes
        // (only user-global memory does). Prevents an untrusted project
        // CLAUDE.md from pulling host files (SSH keys, cloud creds) into the
        // prompt.
        if !allow_external && !path_within(&resolved, cwd) {
            continue;
        }
        let Ok(child_content) = std::fs::read_to_string(&resolved) else {
            continue;
        };
        out.extend(expand_imports(
            &resolved,
            &child_content,
            processed,
            depth + 1,
            cwd,
            allow_external,
        ));
    }

    out
}

/// Strip block-level HTML comments (`<!-- … -->`) from memory content,
/// preserving fenced code blocks and leaving an unclosed `<!--` intact.
///
/// Block-level only, skips fenced code, non-greedy, keeps a dangling
/// `<!--` so a stray marker can't eat the rest of the file.
pub fn strip_html_comments(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    let mut in_fence = false;
    let mut fence_marker: Option<char> = None;
    let mut at_bol = true;
    while !rest.is_empty() {
        if at_bol {
            let trimmed = rest.trim_start_matches([' ', '\t']);
            if let Some(mc) = fence_marker_char(trimmed) {
                match fence_marker {
                    None => {
                        in_fence = true;
                        fence_marker = Some(mc);
                    }
                    Some(open) if open == mc => {
                        in_fence = false;
                        fence_marker = None;
                    }
                    _ => {}
                }
            }
        }
        if !in_fence && rest.starts_with("<!--") {
            match rest[4..].find("-->") {
                Some(end) => {
                    rest = &rest[4 + end + 3..];
                    at_bol = false;
                    continue;
                }
                // Unclosed comment: leave the remainder verbatim.
                None => {
                    out.push_str(rest);
                    break;
                }
            }
        }
        let ch = rest.chars().next().unwrap_or('\u{fffd}');
        out.push(ch);
        at_bol = ch == '\n';
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// Lexically resolve `.`/`..` without touching the filesystem, so an
/// `@import` cannot escape the cwd via `../../..`.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Best-effort canonicalization for a path that may not exist yet: resolve
/// the longest existing prefix (so symlinks like macOS `/tmp`→`/private/tmp`
/// match the canonicalized root), then re-append the remainder.
fn canonicalize_best_effort(path: &Path) -> PathBuf {
    let lexical = lexical_normalize(path);
    if let Ok(c) = lexical.canonicalize() {
        return c;
    }
    if let (Some(parent), Some(name)) = (lexical.parent(), lexical.file_name())
        && let Ok(pc) = parent.canonicalize()
    {
        return pc.join(name);
    }
    lexical
}

/// True when `path` resolves inside `root`'s subtree.
fn path_within(path: &Path, root: &Path) -> bool {
    let root_n = root
        .canonicalize()
        .unwrap_or_else(|_| lexical_normalize(root));
    canonicalize_best_effort(path).starts_with(&root_n)
}

#[cfg(test)]
#[path = "memory_imports.test.rs"]
mod tests;
