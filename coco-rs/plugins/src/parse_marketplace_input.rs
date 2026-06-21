//! Parse a user-supplied marketplace input into a typed
//! [`crate::schemas::MarketplaceSource`].
//!
//! Recognised forms, checked in priority order:
//!
//! 1. **Git SSH URL**: `user@host:path[.git][#ref]`
//!    — any username (alphanumeric / dot / underscore / hyphen).
//! 2. **HTTP / HTTPS** with `.git` suffix or `/_git/` (Azure DevOps) →
//!    [`MarketplaceSource::Git`].
//! 3. **HTTPS GitHub URL** (host is `github.com` or `www.github.com`) →
//!    [`MarketplaceSource::Git`] with `.git` appended when absent.
//! 4. **Generic HTTP / HTTPS** → [`MarketplaceSource::Url`].
//! 5. **Local path** (starts with `/`, `./`, `../`, `~`, or a Windows
//!    drive / backslash-relative form). Stat-classifies into
//!    [`MarketplaceSource::File`] (when `.json`) or
//!    [`MarketplaceSource::Directory`].
//! 6. **GitHub shorthand** `owner/repo[#ref]` or `owner/repo@ref` →
//!    [`MarketplaceSource::Github`].
//!
//! Return shape: `Result<Option<MarketplaceSource>, ParseError>`.
//! `Ok(None)` means "unrecognised" so the caller can show a usage
//! message; `Err(_)` means "recognised local-path form but stat failed"
//! and carries a user-displayable reason.

use std::path::Path;
use std::path::PathBuf;

use thiserror::Error;

use crate::schemas::MarketplaceSource;

/// Errors from parsing a marketplace input.
///
/// Idiomatic typed errors — caller can `match` on the variant to render
/// localised messages, or fall back to `Display` for a single-line
/// summary.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    /// The path was a recognised local form but did not exist.
    #[error("Path does not exist: {path}")]
    PathDoesNotExist { path: String },
    /// The path was recognised but the OS refused to stat it.
    #[error("Cannot access path: {path} ({reason})")]
    PathInaccessible { path: String, reason: String },
    /// The path resolved to a non-`.json` file.
    #[error("File path must point to a .json file (marketplace.json), but got: {path}")]
    NonJsonFile { path: String },
    /// The path was neither a file nor a directory.
    #[error("Path is neither a file nor a directory: {path}")]
    NotFileOrDirectory { path: String },
    /// `~` was supplied but the home directory could not be resolved.
    #[error("Cannot expand ~: home directory is not set (input: {input})")]
    HomeUnresolvable { input: String },
}

/// Parse `input` into a [`MarketplaceSource`].
///
/// - `Ok(Some(source))` — successfully resolved.
/// - `Ok(None)` — input shape not recognised; caller shows usage hint.
/// - `Err(ParseError)` — recognised local-path form, but stat /
///   classification failed.
///
/// `expand_home` is the resolver for a leading `~` — production code
/// passes `dirs::home_dir`; tests inject a fixed path.
pub fn parse_marketplace_input<F>(
    input: &str,
    expand_home: F,
) -> Result<Option<MarketplaceSource>, ParseError>
where
    F: FnOnce() -> Option<PathBuf>,
{
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    if let Some((url, git_ref)) = parse_ssh_git(trimmed) {
        return Ok(Some(MarketplaceSource::Git {
            url,
            git_ref,
            path: None,
            sparse_paths: None,
        }));
    }

    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Ok(Some(parse_http_or_https(trimmed)));
    }

    if is_local_path_form(trimmed) {
        return parse_local_path(trimmed, expand_home).map(Some);
    }

    if let Some(src) = parse_github_shorthand(trimmed) {
        return Ok(Some(src));
    }

    Ok(None)
}

// ─── SSH ────────────────────────────────────────────────────────────────

/// Match `user@host:path[.git][#ref]`. Returns `(url_without_fragment, ref)`.
fn parse_ssh_git(s: &str) -> Option<(String, Option<String>)> {
    let (head, rest) = s.split_once('@')?;
    if head.is_empty() || !head.chars().all(is_ssh_user_char) {
        return None;
    }
    // After `@`: `host:path[.git][#ref]`
    let colon = rest.find(':')?;
    let (host, path_part) = rest.split_at(colon);
    if host.is_empty() {
        return None;
    }
    // Strip the leading ':'.
    let path_part = &path_part[1..];
    if path_part.is_empty() {
        return None;
    }
    let (path_no_frag, fragment) = match path_part.split_once('#') {
        Some((p, f)) if !f.is_empty() => (p, Some(f.to_string())),
        _ => (path_part, None),
    };
    let url = format!("{head}@{host}:{path_no_frag}");
    Some((url, fragment))
}

fn is_ssh_user_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

// ─── HTTP / HTTPS ───────────────────────────────────────────────────────

fn parse_http_or_https(s: &str) -> MarketplaceSource {
    let (url_no_frag, git_ref) = split_fragment(s);

    // Explicit git endpoints: `.git` suffix (GitHub/GitLab/Bitbucket
    // convention) or `/_git/` (Azure DevOps path segment).
    if url_no_frag.ends_with(".git") || url_no_frag.contains("/_git/") {
        return MarketplaceSource::Git {
            url: url_no_frag.to_string(),
            git_ref,
            path: None,
            sparse_paths: None,
        };
    }

    if let Some(host) = url_host(url_no_frag)
        && (host == "github.com" || host == "www.github.com")
        && let Some(_repo_seg) = github_repo_path_segment(url_no_frag)
    {
        // Keep HTTPS but route through `git` source (clones via
        // git, not fetched as JSON). Append `.git` when absent.
        let with_dotgit = if url_no_frag.ends_with(".git") {
            url_no_frag.to_string()
        } else {
            format!("{url_no_frag}.git")
        };
        return MarketplaceSource::Git {
            url: with_dotgit,
            git_ref,
            path: None,
            sparse_paths: None,
        };
    }

    MarketplaceSource::Url {
        url: url_no_frag.to_string(),
        headers: None,
    }
}

/// Split the URL at the first `#`. Empty fragments are dropped.
fn split_fragment(s: &str) -> (&str, Option<String>) {
    match s.split_once('#') {
        Some((url, frag)) if !frag.is_empty() => (url, Some(frag.to_string())),
        _ => (s, None),
    }
}

/// Return the lowercased host of a URL string, or `None` if it's not a
/// well-formed HTTP/HTTPS URL we can parse.
fn url_host(url: &str) -> Option<String> {
    let after_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    // Strip path / query / fragment markers.
    let host_with_port = after_scheme
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())?;
    let host = host_with_port.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// For a github.com URL, find the `owner/repo` path segment if present.
fn github_repo_path_segment(url: &str) -> Option<String> {
    let after_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let path = after_scheme
        .split_once('/')
        .map(|(_host, rest)| rest)
        .unwrap_or("");
    let mut parts = path.splitn(3, '/');
    let owner = parts.next()?;
    let repo_raw = parts.next()?;
    if owner.is_empty() || repo_raw.is_empty() {
        return None;
    }
    let repo = repo_raw.trim_end_matches(".git").trim_end_matches('/');
    if repo.is_empty() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

// ─── Local path ─────────────────────────────────────────────────────────

#[cfg(windows)]
fn is_local_path_form(s: &str) -> bool {
    is_unix_local_form(s) || is_windows_local_form(s)
}

#[cfg(not(windows))]
fn is_local_path_form(s: &str) -> bool {
    is_unix_local_form(s)
}

fn is_unix_local_form(s: &str) -> bool {
    s.starts_with("./") || s.starts_with("../") || s.starts_with('/') || s.starts_with('~')
}

#[cfg(windows)]
fn is_windows_local_form(s: &str) -> bool {
    if s.starts_with(".\\") || s.starts_with("..\\") {
        return true;
    }
    let mut chars = s.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(c1), Some(':'), Some(c3))
            if c1.is_ascii_alphabetic() && (c3 == '\\' || c3 == '/') =>
        {
            true
        }
        _ => false,
    }
}

fn parse_local_path<F>(s: &str, expand_home: F) -> Result<MarketplaceSource, ParseError>
where
    F: FnOnce() -> Option<PathBuf>,
{
    let resolved = if let Some(rest) = s.strip_prefix('~') {
        match expand_home() {
            Some(home) => {
                let rest = rest.trim_start_matches('/');
                if rest.is_empty() {
                    home
                } else {
                    home.join(rest)
                }
            }
            None => {
                return Err(ParseError::HomeUnresolvable {
                    input: s.to_string(),
                });
            }
        }
    } else {
        PathBuf::from(s)
    };
    let resolved = absolutize(&resolved);

    classify_local_path(&resolved)
}

fn absolutize(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    }
}

fn classify_local_path(path: &Path) -> Result<MarketplaceSource, ParseError> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => {
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                Ok(MarketplaceSource::File {
                    path: path.to_string_lossy().into_owned(),
                })
            } else {
                Err(ParseError::NonJsonFile {
                    path: path.display().to_string(),
                })
            }
        }
        Ok(meta) if meta.is_dir() => Ok(MarketplaceSource::Directory {
            path: path.to_string_lossy().into_owned(),
        }),
        Ok(_) => Err(ParseError::NotFileOrDirectory {
            path: path.display().to_string(),
        }),
        Err(e) => Err(match e.kind() {
            std::io::ErrorKind::NotFound => ParseError::PathDoesNotExist {
                path: path.display().to_string(),
            },
            _ => ParseError::PathInaccessible {
                path: path.display().to_string(),
                reason: e.to_string(),
            },
        }),
    }
}

// ─── GitHub shorthand ───────────────────────────────────────────────────

fn parse_github_shorthand(s: &str) -> Option<MarketplaceSource> {
    if !s.contains('/') || s.starts_with('@') || s.contains(':') {
        return None;
    }
    // Accept both `#ref` and `@ref` as the fragment separator
    // (display formatter emits `@`, users naturally copy it back).
    let (repo, git_ref) = if let Some((repo, r)) = s.split_once('#') {
        (repo, (!r.is_empty()).then(|| r.to_string()))
    } else if let Some((repo, r)) = s.split_once('@') {
        (repo, (!r.is_empty()).then(|| r.to_string()))
    } else {
        (s, None)
    };
    if !repo.contains('/') || repo.starts_with('/') || repo.ends_with('/') {
        return None;
    }
    let mut parts = repo.split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return None;
    }
    Some(MarketplaceSource::Github {
        repo: repo.to_string(),
        git_ref,
        path: None,
        sparse_paths: None,
    })
}

// ─── Marketplace name derivation ─────────────────────────────────────────

/// Best-effort marketplace-name derivation from a parsed source.
///
/// The CLI registers the marketplace by name before the first fetch, so we
/// synthesize a placeholder name from the source shape and let later updates
/// rewrite it from manifest content.
pub fn derive_marketplace_name(source: &MarketplaceSource) -> String {
    match source {
        MarketplaceSource::Github { repo, .. } => repo
            .split('/')
            .next_back()
            .unwrap_or(repo)
            .trim_end_matches(".git")
            .to_string(),
        MarketplaceSource::Git { url, .. } => derive_name_from_url(url),
        MarketplaceSource::Url { url, .. } => derive_name_from_url(url),
        MarketplaceSource::Npm { package } => package.clone(),
        MarketplaceSource::File { path } | MarketplaceSource::Directory { path } => {
            std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("marketplace")
                .to_string()
        }
    }
}

fn derive_name_from_url(url: &str) -> String {
    let no_frag = url.split('#').next().unwrap_or(url);
    let last = no_frag
        .trim_end_matches('/')
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("marketplace");
    last.trim_end_matches(".git")
        .trim_end_matches(".json")
        .to_string()
}

#[cfg(test)]
#[path = "parse_marketplace_input.test.rs"]
mod tests;
