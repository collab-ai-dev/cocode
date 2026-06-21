//! Async remote git operations: shallow clone, pull, fetch-and-checkout.
//!
//! Distinct from the rest of `coco-git` (synchronous, local-repo operations on
//! an existing repository) — these spawn `git` via `tokio::process` with a
//! hard timeout and interactive-prompt hardening, for fetching **remote**
//! repositories (e.g. plugin marketplaces and remote-sourced plugins).
//!
//! Credentials embedded in a URL (`user:pass@host`) are redacted from every
//! error message and the echoed command line.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use crate::GitToolingError;

/// Default timeout for a remote git operation (clone / pull / fetch).
pub const DEFAULT_REMOTE_TIMEOUT: Duration = Duration::from_secs(120);

/// Options for [`shallow_clone`].
#[derive(Debug, Default, Clone)]
pub struct CloneOptions {
    /// Branch or tag to clone (`--branch <ref>`).
    pub git_ref: Option<String>,
    /// Cone-mode sparse-checkout paths. When non-empty the clone uses
    /// `--filter=blob:none --no-checkout`, then `sparse-checkout set --cone`
    /// followed by a checkout — only the listed directories materialize.
    pub sparse_paths: Vec<String>,
    /// Recurse + shallow-fetch submodules on a full (non-sparse) clone.
    pub recurse_submodules: bool,
}

/// Shallow-clone `url` into `dest` (created if absent).
///
/// `dest` must not already exist as a non-empty directory (git's own
/// constraint). Callers materializing into a reused path should clear it first.
pub async fn shallow_clone(
    url: &str,
    dest: &Path,
    opts: &CloneOptions,
) -> Result<(), GitToolingError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let sparse = !opts.sparse_paths.is_empty();
    let mut args = vec![
        "-c".to_string(),
        "credential.helper=".to_string(),
        "clone".to_string(),
        "--depth".to_string(),
        "1".to_string(),
    ];
    if let Some(r) = &opts.git_ref {
        args.push("--branch".to_string());
        args.push(r.clone());
    }
    if sparse {
        args.push("--filter=blob:none".to_string());
        args.push("--no-checkout".to_string());
    } else if opts.recurse_submodules {
        args.push("--recurse-submodules".to_string());
        args.push("--shallow-submodules".to_string());
    }
    args.push(url.to_string());
    args.push(dest.to_string_lossy().to_string());

    run_git(&args, None, DEFAULT_REMOTE_TIMEOUT).await?;

    if sparse {
        let mut sc = vec![
            "sparse-checkout".to_string(),
            "set".to_string(),
            "--cone".to_string(),
            "--".to_string(),
        ];
        sc.extend(opts.sparse_paths.iter().cloned());
        run_git(&sc, Some(dest), DEFAULT_REMOTE_TIMEOUT).await?;
        run_git(
            &["checkout".to_string(), "HEAD".to_string()],
            Some(dest),
            DEFAULT_REMOTE_TIMEOUT,
        )
        .await?;
    }
    Ok(())
}

/// Pull the latest into an existing clone at `dir`. With `git_ref`,
/// fetch + checkout + pull that ref; otherwise pull `origin HEAD`.
pub async fn pull(dir: &Path, git_ref: Option<&str>) -> Result<(), GitToolingError> {
    match git_ref {
        Some(r) => {
            run_git(
                &["fetch".to_string(), "origin".to_string(), r.to_string()],
                Some(dir),
                DEFAULT_REMOTE_TIMEOUT,
            )
            .await?;
            run_git(
                &["checkout".to_string(), r.to_string()],
                Some(dir),
                DEFAULT_REMOTE_TIMEOUT,
            )
            .await?;
            run_git(
                &["pull".to_string(), "origin".to_string(), r.to_string()],
                Some(dir),
                DEFAULT_REMOTE_TIMEOUT,
            )
            .await
        }
        None => {
            run_git(
                &["pull".to_string(), "origin".to_string(), "HEAD".to_string()],
                Some(dir),
                DEFAULT_REMOTE_TIMEOUT,
            )
            .await
        }
    }
}

/// Fetch a specific commit and check it out — shallow clones lack arbitrary
/// history, so an exact `sha` must be fetched on demand.
pub async fn fetch_and_checkout_sha(dir: &Path, sha: &str) -> Result<(), GitToolingError> {
    run_git(
        &[
            "fetch".to_string(),
            "--depth".to_string(),
            "1".to_string(),
            "origin".to_string(),
            sha.to_string(),
        ],
        Some(dir),
        DEFAULT_REMOTE_TIMEOUT,
    )
    .await?;
    run_git(
        &["checkout".to_string(), sha.to_string()],
        Some(dir),
        DEFAULT_REMOTE_TIMEOUT,
    )
    .await
}

/// Run `git <args>` asynchronously with interactive credential / host-key
/// prompts disabled and a hard timeout. Returns the credential-redacted
/// stderr on a non-zero exit.
pub async fn run_git(
    args: &[String],
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<(), GitToolingError> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.args(args);
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    // Fail-closed: never block on an interactive credential / host-key prompt.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let command = redact_credentials(&format!("git {}", args.join(" ")));
    let child = cmd.spawn()?;
    let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(GitToolingError::Io(e)),
        Err(_) => {
            return Err(GitToolingError::GitTimeout {
                command,
                seconds: timeout.as_secs(),
            });
        }
    };
    if output.status.success() {
        Ok(())
    } else {
        Err(GitToolingError::GitCommand {
            command,
            status: output.status,
            stderr: redact_credentials(String::from_utf8_lossy(&output.stderr).trim()),
        })
    }
}

/// Parse an `owner/repo` slug from a git remote URL. Handles the SSH
/// (`git@host:owner/repo.git`) and HTTPS (`https://host/owner/repo.git`)
/// forms; returns `None` for shapes without at least two path segments.
///
/// TS parity: `utils/git.ts getGithubRepo()` derives the team-memory
/// `?repo=` key from `remote.origin.url`.
pub fn parse_origin_slug(url: &str) -> Option<String> {
    let url = url.trim();
    let after_host = match url.split_once("://") {
        // scheme://[user@]host/<path>
        Some((_, rest)) => rest.split_once('/').map(|(_, p)| p)?,
        None => match url.split_once('@') {
            // [user@]host:<path>  (scp-like SSH)
            Some((_, rest)) => rest.split_once(':').map(|(_, p)| p)?,
            // host/<path>
            None => url.split_once('/').map(|(_, p)| p).unwrap_or(url),
        },
    };
    let path = after_host.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() >= 2 {
        Some(format!("{}/{}", segs[segs.len() - 2], segs[segs.len() - 1]))
    } else {
        None
    }
}

/// Best-effort `owner/repo` slug from the `origin` remote of the repo at
/// `dir`. Returns `None` when `dir` is not a repo, has no `origin`, or the
/// URL does not parse to a slug. Fail-closed: hardened against interactive
/// prompts, 5s timeout.
pub async fn github_origin_slug(dir: &Path) -> Option<String> {
    let mut cmd = tokio::process::Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .arg("config")
        .arg("--get")
        .arg("remote.origin.url");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_ASKPASS", "");
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    let output = tokio::time::timeout(Duration::from_secs(5), cmd.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_origin_slug(&String::from_utf8_lossy(&output.stdout))
}

/// Redact `scheme://user:pass@` userinfo from a URL or git error message.
pub fn redact_credentials(s: &str) -> String {
    static RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    // The pattern is a valid compile-time constant; `Option` keeps the
    // initializer infallible (no unwrap/expect) — a `None` just means the
    // input passes through unredacted, which is a safe degradation.
    match RE.get_or_init(|| regex::Regex::new(r"(\w+://)[^/\s@]+@").ok()) {
        Some(re) => re.replace_all(s, "$1***@").into_owned(),
        None => s.to_string(),
    }
}

#[cfg(test)]
#[path = "remote.test.rs"]
mod tests;
