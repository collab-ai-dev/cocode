//! Network fetch backend for marketplace + plugin sources.
//!
//! Materializes remote **marketplace** sources (git clone/pull, HTTP
//! download) and remote **plugin** sources (per-plugin git / npm / pip).
//!
//! git operations **shell out to
//! the `git` binary** — matching the `coco-git` crate (no `git2`/`gix`
//! dependency). HTTP downloads use `reqwest` behind the
//! `coco-hooks` `SsrfGuardedResolver` (a net hardening).

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::errors::PluginError;
use crate::schemas::MarketplaceSource;
use crate::schemas::PluginMarketplace;
use crate::schemas::RemotePluginSource;

type Result<T> = std::result::Result<T, PluginError>;

/// HTTP fetch timeout for URL marketplaces.
const URL_TIMEOUT: Duration = Duration::from_secs(10);
/// Package-manager (npm/pip) timeout — heavier than a shallow git clone.
const PKG_TIMEOUT: Duration = Duration::from_secs(300);
const USER_AGENT: &str = "CoCo-Plugin-Manager";

// ---------------------------------------------------------------------------
// Marketplace source fetch
// ---------------------------------------------------------------------------

/// Materialize a marketplace `source` under `cache_dir` (the `marketplaces/`
/// directory) and return the `install_location` to record in
/// `known_marketplaces.json`.
///
/// Idempotent: a git source with an existing clone is pulled in place;
/// otherwise it is freshly cloned. Local (`File`/`Directory`) sources are
/// returned as-is — they need no fetch.
pub async fn fetch_marketplace(
    source: &MarketplaceSource,
    name: &str,
    cache_dir: &Path,
) -> Result<PathBuf> {
    match source {
        MarketplaceSource::Url { url, headers } => {
            let dest = cache_dir.join(format!("{}.json", sanitize(name)));
            ensure_parent(&dest)?;
            fetch_marketplace_url(url, headers.as_ref(), &dest).await?;
            Ok(dest)
        }
        MarketplaceSource::Github {
            repo,
            git_ref,
            path,
            sparse_paths,
        } => {
            let dir = cache_dir.join(sanitize(name));
            git_materialize(
                &github_https_url(repo),
                &dir,
                git_ref.as_deref(),
                sparse_paths.as_deref(),
            )
            .await?;
            Ok(install_location_for(dir, path.as_deref()))
        }
        MarketplaceSource::Git {
            url,
            git_ref,
            path,
            sparse_paths,
        } => {
            let dir = cache_dir.join(sanitize(name));
            git_materialize(url, &dir, git_ref.as_deref(), sparse_paths.as_deref()).await?;
            Ok(install_location_for(dir, path.as_deref()))
        }
        MarketplaceSource::Npm { package } => Err(PluginError::generic(
            "marketplace",
            format!(
                "npm marketplace sources are not supported (package '{package}'); \
                 use a github/git/url/local source"
            ),
        )),
        MarketplaceSource::File { path } | MarketplaceSource::Directory { path } => {
            Ok(PathBuf::from(path))
        }
    }
}

/// Download a URL marketplace manifest to `dest`, validating it parses as a
/// [`PluginMarketplace`] before persisting. SSRF-guarded.
async fn fetch_marketplace_url(
    url: &str,
    headers: Option<&HashMap<String, String>>,
    dest: &Path,
) -> Result<()> {
    let body = http_get(url, headers).await?;
    serde_json::from_str::<PluginMarketplace>(&body).map_err(|e| {
        PluginError::ManifestValidationFailed {
            name: url.to_string(),
            reason: e.to_string(),
        }
    })?;
    std::fs::write(dest, body)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin source fetch (Phase 2 — per-plugin remote install)
// ---------------------------------------------------------------------------

/// Materialize a remote plugin `source` into `dest` (the versioned plugin
/// cache directory). `dest` is created fresh by the caller.
///
pub async fn fetch_plugin_source(source: &RemotePluginSource, dest: &Path) -> Result<()> {
    match source {
        RemotePluginSource::Github { repo, git_ref, sha } => {
            git_checkout_into(
                &github_https_url(repo),
                dest,
                git_ref.as_deref(),
                sha.as_deref(),
            )
            .await
        }
        RemotePluginSource::Url { url, git_ref, sha } => {
            git_checkout_into(url, dest, git_ref.as_deref(), sha.as_deref()).await
        }
        RemotePluginSource::GitSubdir {
            url,
            path,
            git_ref,
            sha,
        } => git_subdir_into(url, path, dest, git_ref.as_deref(), sha.as_deref()).await,
        RemotePluginSource::Npm {
            package,
            version,
            registry,
        } => npm_install_into(package, version.as_deref(), registry.as_deref(), dest).await,
        RemotePluginSource::Pip {
            package,
            version,
            registry,
        } => pip_install_into(package, version.as_deref(), registry.as_deref(), dest).await,
    }
}

// ---------------------------------------------------------------------------
// git primitives — delegate to coco-git's async remote operations
// ---------------------------------------------------------------------------

/// Map a `coco-git` remote error to a plugin `GitCloneFailed`, redacting the URL.
fn clone_err(url: &str, e: coco_git::GitToolingError) -> PluginError {
    PluginError::GitCloneFailed {
        url: coco_git::redact_credentials(url),
        message: e.to_string(),
    }
}

fn clone_opts(
    git_ref: Option<&str>,
    sparse_paths: Option<&[String]>,
    recurse_submodules: bool,
) -> coco_git::CloneOptions {
    coco_git::CloneOptions {
        git_ref: git_ref.map(str::to_string),
        sparse_paths: sparse_paths.map(<[String]>::to_vec).unwrap_or_default(),
        recurse_submodules,
    }
}

/// Clone `url` into `dir` (fresh), or pull when an existing clone is present.
async fn git_materialize(
    url: &str,
    dir: &Path,
    git_ref: Option<&str>,
    sparse_paths: Option<&[String]>,
) -> Result<()> {
    if dir.join(".git").is_dir() {
        match coco_git::pull(dir, git_ref).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                // Pull failed (history rewrite, corrupted clone) — discard and
                // re-clone on pull failure (history rewrite, corrupted clone).
                tracing::warn!(dir = %dir.display(), error = %e, "git pull failed; re-cloning");
                let _ = std::fs::remove_dir_all(dir);
            }
        }
    } else if dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
    coco_git::shallow_clone(url, dir, &clone_opts(git_ref, sparse_paths, true))
        .await
        .map_err(|e| clone_err(url, e))
}

/// Clone a remote repo and check out an optional `ref`/`sha` into `dest`.
async fn git_checkout_into(
    url: &str,
    dest: &Path,
    git_ref: Option<&str>,
    sha: Option<&str>,
) -> Result<()> {
    coco_git::shallow_clone(url, dest, &clone_opts(git_ref, None, true))
        .await
        .map_err(|e| clone_err(url, e))?;
    if let Some(sha) = sha {
        coco_git::fetch_and_checkout_sha(dest, sha)
            .await
            .map_err(|e| clone_err(url, e))?;
    }
    strip_git_dir(dest);
    Ok(())
}

/// Sparse-clone `url`, materialize only `subdir`, and move it to `dest`.
async fn git_subdir_into(
    url: &str,
    subdir: &str,
    dest: &Path,
    git_ref: Option<&str>,
    sha: Option<&str>,
) -> Result<()> {
    if crate::security::validate_paths(subdir) != crate::security::PathValidation::Ok {
        return Err(PluginError::PathTraversal {
            name: subdir.to_string(),
            path: PathBuf::from(subdir),
        });
    }
    let clone_dir = dest.with_extension("clone");
    let _ = std::fs::remove_dir_all(&clone_dir);
    coco_git::shallow_clone(
        url,
        &clone_dir,
        &clone_opts(git_ref, Some(&[subdir.to_string()]), false),
    )
    .await
    .map_err(|e| clone_err(url, e))?;
    if let Some(sha) = sha {
        coco_git::fetch_and_checkout_sha(&clone_dir, sha)
            .await
            .map_err(|e| clone_err(url, e))?;
    }
    let materialized = clone_dir.join(subdir);
    if !materialized.is_dir() {
        let _ = std::fs::remove_dir_all(&clone_dir);
        return Err(PluginError::generic(
            "marketplace",
            format!("git-subdir source did not contain path '{subdir}'"),
        ));
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let _ = std::fs::remove_dir_all(dest);
    if let Err(e) = std::fs::rename(&materialized, dest) {
        // cross-device rename can fail — fall back to a recursive copy.
        copy_tree(&materialized, dest)?;
        tracing::debug!(error = %e, "git-subdir rename fell back to copy");
    }
    let _ = std::fs::remove_dir_all(&clone_dir);
    strip_git_dir(dest);
    Ok(())
}

// ---------------------------------------------------------------------------
// npm / pip primitives (shell-out)
// ---------------------------------------------------------------------------

async fn npm_install_into(
    package: &str,
    version: Option<&str>,
    registry: Option<&str>,
    dest: &Path,
) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let spec = match version {
        Some(v) => format!("{package}@{v}"),
        None => package.to_string(),
    };
    let mut args = vec![
        "install".to_string(),
        spec,
        "--prefix".to_string(),
        dest.to_string_lossy().to_string(),
        "--no-audit".to_string(),
        "--no-fund".to_string(),
    ];
    if let Some(r) = registry {
        args.push("--registry".to_string());
        args.push(r.to_string());
    }
    run_pkg("npm", &args)
        .await
        .map_err(|message| PluginError::NpmInstallFailed {
            package: package.to_string(),
            message,
        })
}

async fn pip_install_into(
    package: &str,
    version: Option<&str>,
    registry: Option<&str>,
    dest: &Path,
) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let spec = match version {
        Some(v) => format!("{package}=={v}"),
        None => package.to_string(),
    };
    let mut args = vec![
        "install".to_string(),
        spec,
        "--target".to_string(),
        dest.to_string_lossy().to_string(),
    ];
    if let Some(r) = registry {
        args.push("--index-url".to_string());
        args.push(r.to_string());
    }
    run_pkg("pip", &args)
        .await
        .map_err(|message| PluginError::PipInstallFailed {
            package: package.to_string(),
            message,
        })
}

async fn run_pkg(bin: &str, args: &[String]) -> std::result::Result<(), String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn {bin}: {e} (is {bin} installed and on PATH?)"))?;
    let output = match tokio::time::timeout(PKG_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("{bin} execution error: {e}")),
        Err(_) => return Err(format!("{bin} timed out after {}s", PKG_TIMEOUT.as_secs())),
    };
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = stderr.trim();
        Err(if msg.is_empty() {
            format!("{bin} exited with a non-zero status")
        } else {
            msg.to_string()
        })
    }
}

// ---------------------------------------------------------------------------
// HTTP primitive (SSRF-guarded)
// ---------------------------------------------------------------------------

/// SSRF-guarded HTTP GET returning the response body as text.
///
/// Two-layer guard (mirrors `coco-hooks`): a pre-flight `check_url_ssrf`
/// catches IP-literal URLs (the connect-time resolver is not consulted for
/// those), and the `SsrfGuardedResolver` re-resolves hostnames at connect
/// time to close the DNS-rebinding TOCTOU. Redirects are disabled so an
/// allowlisted host cannot 3xx into an internal address.
async fn http_get(url: &str, headers: Option<&HashMap<String, String>>) -> Result<String> {
    match coco_hooks::ssrf::check_url_ssrf(url).await {
        Ok(true) => {
            return Err(PluginError::NetworkError {
                url: coco_git::redact_credentials(url),
                message: "URL resolves to a private/link-local address".to_string(),
            });
        }
        Ok(false) => {}
        Err(e) => tracing::debug!("SSRF pre-flight DNS failed for {url}: {e}"),
    }

    let client = reqwest::Client::builder()
        .timeout(URL_TIMEOUT)
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(std::sync::Arc::new(coco_hooks::ssrf::SsrfGuardedResolver))
        .build()
        .map_err(|e| PluginError::NetworkError {
            url: coco_git::redact_credentials(url),
            message: e.to_string(),
        })?;

    let mut req = client.get(url);
    if let Some(h) = headers {
        for (k, v) in h {
            req = req.header(k, v);
        }
    }
    let resp = req.send().await.map_err(|e| PluginError::NetworkError {
        url: coco_git::redact_credentials(url),
        message: e.to_string(),
    })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(PluginError::DownloadFailed {
            url: coco_git::redact_credentials(url),
            status: i32::from(status.as_u16()),
        });
    }
    resp.text().await.map_err(|e| PluginError::NetworkError {
        url: coco_git::redact_credentials(url),
        message: e.to_string(),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn github_https_url(repo: &str) -> String {
    format!("https://github.com/{repo}.git")
}

/// `install_location` for a git marketplace = clone dir, or `dir/path` when
/// the manifest lives in a subdirectory.
fn install_location_for(dir: PathBuf, path: Option<&str>) -> PathBuf {
    match path {
        Some(p) if !p.is_empty() => dir.join(p),
        _ => dir,
    }
}

fn ensure_parent(p: &Path) -> Result<()> {
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn strip_git_dir(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir.join(".git"));
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "fetch.test.rs"]
mod tests;
