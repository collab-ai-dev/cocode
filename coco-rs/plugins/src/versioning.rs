//! Plugin version calculation per source type.
//!
//! Each source kind gets a deterministic version string used to compute the
//! versioned cache path `~/.coco/plugins/<name>/<version>/`.

use sha2::Digest;
use sha2::Sha256;

/// Source-aware identifier for version computation.
#[derive(Debug, Clone)]
pub enum VersionSource<'a> {
    /// Git source — use the short SHA when known, else `ref` name.
    Git {
        sha: Option<&'a str>,
        ref_: Option<&'a str>,
    },
    /// npm or pip — use the package version string.
    Package { version: &'a str },
    /// Local path or URL — content-hash the manifest payload.
    LocalOrUrl { manifest_bytes: &'a [u8] },
}

/// Compute the deterministic version string.
pub fn calculate_plugin_version(source: VersionSource<'_>) -> String {
    match source {
        VersionSource::Git { sha: Some(s), .. } => short_sha(s).to_string(),
        VersionSource::Git {
            sha: None,
            ref_: Some(r),
        } => format!("ref-{r}"),
        VersionSource::Git {
            sha: None,
            ref_: None,
        } => "head".to_string(),
        VersionSource::Package { version } => version.to_string(),
        VersionSource::LocalOrUrl { manifest_bytes } => {
            let mut h = Sha256::new();
            h.update(manifest_bytes);
            let digest = h.finalize();
            // 12 hex chars is plenty for a content-addressed dir name.
            hex::encode(&digest[..6])
        }
    }
}

fn short_sha(sha: &str) -> &str {
    if sha.len() > 12 { &sha[..12] } else { sha }
}

/// Build the versioned cache path under a base dir.
pub fn versioned_cache_path(
    base: &std::path::Path,
    name: &str,
    version: &str,
) -> std::path::PathBuf {
    base.join(name).join(version)
}

#[cfg(test)]
#[path = "versioning.test.rs"]
mod tests;
