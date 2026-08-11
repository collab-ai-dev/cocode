//! Fail-closed expansion of deny-read globs.
//!
//! Linux mount namespaces require concrete paths, so glob rules are resolved
//! immediately before each command is wrapped. Literal prefixes narrow each
//! walk (`secrets/**` starts at `secrets/`), while depth, entry, and match
//! limits bound startup work. macOS uses anchored Seatbelt regexes from the
//! same validated glob subset, covering matches created after launch.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use globset::Glob;
use globset::GlobBuilder;
use globset::GlobMatcher;

use crate::config::SandboxConfig;
use crate::error::SandboxError;

const MAX_DENY_GLOB_MATCHES: usize = 4_096;
const MAX_DENY_GLOB_ENTRIES: usize = 2_000_000;

#[derive(Debug)]
pub struct GlobExpansionError {
    message: String,
}

impl GlobExpansionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for GlobExpansionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for GlobExpansionError {}

#[derive(Clone, Copy)]
struct ExpansionLimits {
    depth: usize,
    matches: usize,
    entries: usize,
}

struct CompiledGlob {
    raw: String,
    matcher: GlobMatcher,
}

/// Classify a deny-read entry. Pure paths remain literal; patterns with
/// gitignore-style metacharacters are expanded at the platform-wrap boundary.
pub fn looks_like_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

/// Compile the portable deny-glob subset shared by Linux globset expansion
/// and macOS Seatbelt regexes. Reject syntax whose meaning would diverge
/// between the two enforcers instead of weakening one platform silently.
fn compile_deny_glob(raw: &str) -> Result<Glob, GlobExpansionError> {
    if !looks_like_glob(raw) {
        return Err(GlobExpansionError::new(format!(
            "deny-read glob {raw:?} contains no glob metacharacter"
        )));
    }
    if let Some(ch) = raw
        .chars()
        .find(|ch| ch.is_control() || matches!(ch, '?' | '[' | '{' | '}' | '\\'))
    {
        return Err(GlobExpansionError::new(format!(
            "deny-read glob {raw:?} uses unsupported character {ch:?}"
        )));
    }
    for (index, segment) in raw.split('/').enumerate() {
        if segment.is_empty() && !(index == 0 && raw.starts_with('/')) {
            return Err(GlobExpansionError::new(format!(
                "deny-read glob {raw:?} contains an empty path segment"
            )));
        }
        if matches!(segment, "." | "..") {
            return Err(GlobExpansionError::new(format!(
                "deny-read glob {raw:?} may not contain {segment:?} segments"
            )));
        }
        if segment.contains("**") && segment != "**" {
            return Err(GlobExpansionError::new(format!(
                "deny-read glob {raw:?} must use ** as a complete path segment"
            )));
        }
    }

    GlobBuilder::new(raw)
        .literal_separator(true)
        .build()
        .map_err(|error| {
            GlobExpansionError::new(format!("invalid deny-read glob {raw:?}: {error}"))
        })
}

/// Resolve glob rules into concrete deny paths once, before Linux builds its
/// command wrapper. With no globs the original config is borrowed.
pub(crate) fn resolve_config(
    config: &SandboxConfig,
) -> crate::error::Result<Cow<'_, SandboxConfig>> {
    if config.denied_read_globs.is_empty() {
        return Ok(Cow::Borrowed(config));
    }
    let depth = usize::try_from(config.glob_scan_max_depth)
        .ok()
        .filter(|depth| *depth > 0)
        .ok_or_else(|| {
            SandboxError::apply_error(format!(
                "deny-read globs require a positive scan depth, got {}",
                config.glob_scan_max_depth
            ))
        })?;
    let roots: Vec<PathBuf> = config
        .writable_roots
        .iter()
        .map(|root| root.path.clone())
        .collect();
    let expanded = expand(&roots, &config.denied_read_globs, depth).map_err(|error| {
        SandboxError::apply_error(format!("deny-read glob expansion failed: {error}"))
    })?;

    let mut resolved = config.clone();
    resolved.denied_read_paths.extend(expanded);
    resolved.denied_read_paths.sort();
    resolved.denied_read_paths.dedup();
    resolved.denied_read_globs.clear();
    Ok(Cow::Owned(resolved))
}

/// Expand `globs` under their literal roots. Relative patterns are evaluated
/// once per writable root; absolute patterns start at their own literal
/// prefix, including paths outside writable roots.
pub fn expand(
    roots: &[PathBuf],
    globs: &[String],
    max_depth: usize,
) -> Result<Vec<PathBuf>, GlobExpansionError> {
    expand_with_limits(
        roots,
        globs,
        ExpansionLimits {
            depth: max_depth,
            matches: MAX_DENY_GLOB_MATCHES,
            entries: MAX_DENY_GLOB_ENTRIES,
        },
    )
}

fn expand_with_limits(
    roots: &[PathBuf],
    globs: &[String],
    limits: ExpansionLimits,
) -> Result<Vec<PathBuf>, GlobExpansionError> {
    if globs.is_empty() {
        return Ok(Vec::new());
    }
    if limits.depth == 0 {
        return Err(GlobExpansionError::new(
            "deny-read glob scan depth must be greater than zero",
        ));
    }

    let mut compiled = Vec::with_capacity(globs.len());
    let mut plans: BTreeMap<(PathBuf, Option<PathBuf>), Vec<usize>> = BTreeMap::new();
    for raw in globs {
        let matcher = compile_deny_glob(raw)?.compile_matcher();
        let index = compiled.len();
        compiled.push(CompiledGlob {
            raw: raw.clone(),
            matcher,
        });

        if raw.starts_with('/') {
            plans
                .entry((literal_scan_root(Path::new("/"), raw), None))
                .or_default()
                .push(index);
        } else {
            if roots.is_empty() {
                return Err(GlobExpansionError::new(format!(
                    "relative deny-read glob {raw:?} has no writable root to anchor it"
                )));
            }
            for base in roots {
                plans
                    .entry((literal_scan_root(base, raw), Some(base.clone())))
                    .or_default()
                    .push(index);
            }
        }
    }

    let mut visited = 0usize;
    let mut matches = BTreeSet::new();
    for ((scan_root, relative_base), pattern_indexes) in plans {
        if !scan_root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&scan_root)
            .max_depth(limits.depth)
            .follow_links(false)
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    return Err(GlobExpansionError::new(format!(
                        "walk error while expanding deny-read globs: {error}"
                    )));
                }
            };
            visited += 1;
            if visited > limits.entries {
                return Err(GlobExpansionError::new(format!(
                    "deny-read glob expansion visited over {} entries at {}; use a narrower literal prefix",
                    limits.entries,
                    entry.path().display()
                )));
            }
            if entry.depth() >= limits.depth && entry.file_type().is_dir() {
                return Err(GlobExpansionError::new(format!(
                    "{} reaches the {}-level deny-read glob scan limit; deeper matches could be hidden",
                    entry.path().display(),
                    limits.depth
                )));
            }
            if entry.path().to_str().is_none() {
                return Err(GlobExpansionError::new(format!(
                    "deny-read glob walk encountered a non-UTF-8 path: {:?}",
                    entry.path()
                )));
            }

            let candidate = match &relative_base {
                Some(base) => entry.path().strip_prefix(base).unwrap_or(entry.path()),
                None => entry.path(),
            };
            let matched = pattern_indexes
                .iter()
                .any(|index| compiled[*index].matcher.is_match(candidate));
            if !matched {
                continue;
            }
            insert_match(&mut matches, entry.path())?;
            if matches.len() > limits.matches {
                let patterns: Vec<&str> = pattern_indexes
                    .iter()
                    .map(|index| compiled[*index].raw.as_str())
                    .collect();
                return Err(GlobExpansionError::new(format!(
                    "deny-read globs {patterns:?} matched over {} paths; use narrower globs or deny a parent path",
                    limits.matches
                )));
            }
        }
    }
    Ok(matches.into_iter().collect())
}

/// Translate deny globs into anchored Seatbelt regex filters. Relative globs
/// are anchored below each writable root. Literal-root aliases and canonical
/// symlink targets are included so `/tmp`/`/private/tmp` and symlinked roots
/// cannot bypass a rule. Unlike Linux's bind-over expansion, these filters
/// also cover matching paths created after the command starts.
#[cfg(any(target_os = "macos", test))]
pub(crate) fn seatbelt_regex_filters(
    roots: &[PathBuf],
    globs: &[String],
) -> Result<Vec<String>, GlobExpansionError> {
    let mut filters = BTreeSet::new();
    for raw in globs {
        compile_deny_glob(raw)?;
        let bases: Vec<&Path> = if raw.starts_with('/') {
            vec![Path::new("/")]
        } else {
            if roots.is_empty() {
                return Err(GlobExpansionError::new(format!(
                    "relative deny-read glob {raw:?} has no writable root to anchor it"
                )));
            }
            roots.iter().map(PathBuf::as_path).collect()
        };
        for base in bases {
            let (root, tail) = split_glob_root(base, raw)?;
            let canonical = canonicalize_existing_ancestor(&root);
            let tail_regex = glob_tail_to_regex(&tail);
            for form in macos_deny_aliases(&root, &canonical) {
                let Some(form) = form.to_str() else {
                    return Err(GlobExpansionError::new(format!(
                        "deny-read glob root is not valid UTF-8: {form:?}"
                    )));
                };
                if form.chars().any(char::is_control) {
                    return Err(GlobExpansionError::new(format!(
                        "deny-read glob root contains a control character: {form:?}"
                    )));
                }
                let root_regex = escape_regex_literal(form);
                let separator = if root_regex.ends_with('/') { "" } else { "/" };
                let body = format!("^{root_regex}{separator}{tail_regex}$");
                filters.insert(format!("(regex #\"{}\")", body.replace('"', "\\\"")));
            }
        }
    }
    Ok(filters.into_iter().collect())
}

#[cfg(any(target_os = "macos", test))]
fn split_glob_root(base: &Path, raw: &str) -> Result<(PathBuf, String), GlobExpansionError> {
    let mut root = base.to_path_buf();
    let relative = raw.strip_prefix('/').unwrap_or(raw);
    let segments: Vec<&str> = relative.split('/').collect();
    let Some(first_glob) = segments.iter().position(|segment| looks_like_glob(segment)) else {
        return Err(GlobExpansionError::new(format!(
            "deny-read glob {raw:?} contains no glob metacharacter"
        )));
    };
    for segment in &segments[..first_glob] {
        root.push(segment);
    }
    Ok((root, segments[first_glob..].join("/")))
}

#[cfg(any(target_os = "macos", test))]
fn glob_tail_to_regex(tail: &str) -> String {
    let mut regex = String::new();
    let mut chars = tail.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if chars.peek() == Some(&'/') {
                    chars.next();
                    regex.push_str("(.*/)?");
                } else {
                    regex.push_str(".*");
                }
            }
            '*' => regex.push_str("[^/]*"),
            literal => push_regex_literal(&mut regex, literal),
        }
    }
    regex
}

#[cfg(any(target_os = "macos", test))]
fn escape_regex_literal(text: &str) -> String {
    let mut escaped = String::new();
    for ch in text.chars() {
        push_regex_literal(&mut escaped, ch);
    }
    escaped
}

#[cfg(any(target_os = "macos", test))]
fn push_regex_literal(output: &mut String, ch: char) {
    if matches!(
        ch,
        '.' | '+' | '*' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\'
    ) {
        output.push('\\');
    }
    output.push(ch);
}

#[cfg(any(target_os = "macos", test))]
fn canonicalize_existing_ancestor(root: &Path) -> PathBuf {
    let mut ancestor = root;
    let mut suffix = Vec::new();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(ancestor) {
            return suffix
                .iter()
                .rev()
                .fold(canonical, |path, component| path.join(component));
        }
        let Some(component) = ancestor.file_name() else {
            return root.to_path_buf();
        };
        suffix.push(component);
        let Some(parent) = ancestor.parent() else {
            return root.to_path_buf();
        };
        ancestor = parent;
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_deny_aliases(path: &Path, canonical: &Path) -> BTreeSet<PathBuf> {
    let mut aliases = BTreeSet::from([path.to_path_buf(), canonical.to_path_buf()]);
    for candidate in aliases.clone() {
        if let Some(alias) = toggle_private_prefix(&candidate) {
            aliases.insert(alias);
        }
    }
    aliases
}

#[cfg(any(target_os = "macos", test))]
fn toggle_private_prefix(path: &Path) -> Option<PathBuf> {
    let text = path.to_str()?;
    for directory in ["tmp", "var", "etc"] {
        if let Some(rest) = text.strip_prefix(&format!("/private/{directory}"))
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return Some(PathBuf::from(format!("/{directory}{rest}")));
        }
        if let Some(rest) = text.strip_prefix(&format!("/{directory}"))
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return Some(PathBuf::from(format!("/private/{directory}{rest}")));
        }
    }
    None
}

fn literal_scan_root(base: &Path, pattern: &str) -> PathBuf {
    let mut root = base.to_path_buf();
    let relative = pattern.strip_prefix('/').unwrap_or(pattern);
    for segment in relative.split('/') {
        if segment.is_empty() {
            continue;
        }
        if looks_like_glob(segment) {
            break;
        }
        root.push(segment);
    }
    root
}

fn insert_match(matches: &mut BTreeSet<PathBuf>, path: &Path) -> Result<(), GlobExpansionError> {
    matches.insert(path.to_path_buf());
    let canonical = std::fs::canonicalize(path).map_err(|error| {
        GlobExpansionError::new(format!(
            "could not resolve deny-read glob match {}: {error}",
            path.display()
        ))
    })?;
    if canonical != path {
        if canonical.to_str().is_none() {
            return Err(GlobExpansionError::new(format!(
                "deny-read glob match resolves to a non-UTF-8 path: {canonical:?}"
            )));
        }
        matches.insert(canonical);
    }
    Ok(())
}

#[cfg(test)]
#[path = "glob_expansion.test.rs"]
mod tests;
