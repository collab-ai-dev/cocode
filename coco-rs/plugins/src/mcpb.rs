//! MCPB (`.mcpb` / `.dxt`) bundle loader.
//!
//! MCPB = ZIP container holding an MCP server bundled with its manifest and
//! optional `configSchema`. Pipeline:
//! 1. Download / read the archive.
//! 2. Parse `manifest.json` → [`McpbManifest`].
//! 3. Validate user config against `configSchema`.
//! 4. Extract files to a content-addressed cache dir.
//! 5. Generate the runtime MCP server config.
//!
//! Cache: `<cache_root>/<sha256>/`. Metadata sidecar tracks source URL +
//! sha + extracted_at + last_used.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

/// Bundle manifest read from `manifest.json` inside the archive.
///
/// `user_config` is a JSONSchema-style required-field map. The serde alias
/// keeps backward compatibility for in-tree fixtures using the old name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpbManifest {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub server: McpbServerSpec,
    /// JSONSchema-style config requirements.
    #[serde(default, alias = "config_schema", rename = "user_config")]
    pub user_config: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpbServerSpec {
    /// Executable path inside the archive (relative).
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// Sidecar metadata for cached MCPB archives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpbCacheMetadata {
    pub source_url: String,
    pub sha256: String,
    pub extracted_at: chrono::DateTime<chrono::Utc>,
    pub last_used: chrono::DateTime<chrono::Utc>,
}

/// Result of [`load_mcpb`].
#[derive(Debug, Clone)]
pub enum McpbLoadStatus {
    /// Bundle ready to use.
    Ready(McpbLoadResult),
    /// User config missing required fields per `config_schema`.
    NeedsConfig {
        config_schema: HashMap<String, serde_json::Value>,
        existing_config: HashMap<String, serde_json::Value>,
        validation_errors: Vec<String>,
    },
}

#[derive(Debug, Clone)]
pub struct McpbLoadResult {
    pub manifest: McpbManifest,
    pub mcp_config: serde_json::Value,
    pub extracted_path: PathBuf,
    pub content_hash: String,
}

/// Load and extract an MCPB bundle.
///
/// `archive_bytes` are the raw ZIP bytes. `cache_root` is where the bundle
/// will be extracted (e.g. `~/.coco/plugins/mcpb-cache/`). `user_config` is
/// the values the user has already provided for `config_schema` keys.
pub fn load_mcpb(
    source_url: &str,
    archive_bytes: &[u8],
    cache_root: &Path,
    user_config: &HashMap<String, serde_json::Value>,
) -> crate::Result<McpbLoadStatus> {
    let sha = compute_sha256(archive_bytes);
    let target_dir = cache_root.join(&sha);

    // If already cached, just touch metadata and reuse.
    let manifest = if target_dir.is_dir() {
        let manifest_path = target_dir.join("manifest.json");
        if manifest_path.is_file() {
            let raw = std::fs::read_to_string(&manifest_path)?;
            serde_json::from_str(&raw)?
        } else {
            extract_archive(archive_bytes, &target_dir)?
        }
    } else {
        extract_archive(archive_bytes, &target_dir)?
    };

    // Validate user config against schema.
    let errors = validate_config(&manifest.user_config, user_config);
    if !errors.is_empty() {
        let config_schema = manifest.user_config;
        return Ok(McpbLoadStatus::NeedsConfig {
            config_schema,
            existing_config: user_config.clone(),
            validation_errors: errors,
        });
    }

    // Build the MCP server config, expanding `${__dirname}` / `${user_config.X}`
    // placeholders in the command, args, and env.
    let extracted_dir = target_dir.to_string_lossy().into_owned();
    let command = substitute_template(
        &target_dir.join(&manifest.server.command).to_string_lossy(),
        &extracted_dir,
        user_config,
    );
    let args: Vec<String> = manifest
        .server
        .args
        .iter()
        .map(|a| substitute_template(a, &extracted_dir, user_config))
        .collect();
    let mcp_config = serde_json::json!({
        "command": command,
        "args": args,
        "env": merge_env(&manifest.server.env, &extracted_dir, user_config),
    });

    // Write/refresh cache sidecar.
    write_metadata(&target_dir, source_url, &sha)?;

    Ok(McpbLoadStatus::Ready(McpbLoadResult {
        manifest,
        mcp_config,
        extracted_path: target_dir,
        content_hash: sha,
    }))
}

fn compute_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    hex::encode(digest)
}

/// Extract `archive_bytes` (ZIP) into `target_dir`, returning the parsed
/// `manifest.json` from the root of the archive.
fn extract_archive(archive_bytes: &[u8], target_dir: &Path) -> crate::Result<McpbManifest> {
    use std::io::Read;
    use zip::ZipArchive;

    std::fs::create_dir_all(target_dir)?;
    let cursor = std::io::Cursor::new(archive_bytes);
    let mut archive = ZipArchive::new(cursor)?;
    let mut manifest: Option<McpbManifest> = None;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let entry_name = file
            .enclosed_name()
            .ok_or_else(|| {
                crate::PluginError::generic("mcpb", "MCPB archive contains unsafe path")
            })?
            .to_path_buf();
        // Path-traversal guard.
        if entry_name.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        }) {
            return Err(crate::PluginError::generic(
                "mcpb",
                format!(
                    "MCPB archive entry escapes target dir: {}",
                    entry_name.display()
                ),
            ));
        }
        let dest = target_dir.join(&entry_name);
        if file.is_dir() {
            std::fs::create_dir_all(&dest)?;
        } else {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf)?;
            std::fs::write(&dest, &buf)?;
            if entry_name == Path::new("manifest.json") {
                manifest = Some(serde_json::from_slice(&buf)?);
            }
        }
    }

    manifest
        .ok_or_else(|| crate::PluginError::generic("mcpb", "MCPB archive missing manifest.json"))
}

/// Validate user-provided values against the manifest's JSONSchema-style
/// `user_config` map: required-field presence + per-field type (`string`,
/// `string[]` when `multiple`, `number`, `boolean`, `file`/`directory` path),
/// plus numeric `min`/`max`. Error labels prefer the schema's `title`,
/// falling back to the key. Empty result = valid.
fn validate_config(
    schema: &HashMap<String, serde_json::Value>,
    user_config: &HashMap<String, serde_json::Value>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (key, prop) in schema {
        let label = prop
            .get("title")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(key);
        let required = prop
            .get("required")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        // A value is "not provided" when absent or an empty string.
        let value = match user_config.get(key) {
            None => {
                if required {
                    errors.push(format!("{label} is required but not provided"));
                }
                continue;
            }
            Some(serde_json::Value::String(s)) if s.is_empty() => {
                if required {
                    errors.push(format!("{label} is required but not provided"));
                }
                continue;
            }
            Some(v) => v,
        };

        match prop.get("type").and_then(serde_json::Value::as_str) {
            Some("string") => {
                let multiple = prop
                    .get("multiple")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                match value {
                    serde_json::Value::Array(items) => {
                        if !multiple {
                            errors.push(format!("{label} must be a string, not an array"));
                        } else if !items.iter().all(serde_json::Value::is_string) {
                            errors.push(format!("{label} must be an array of strings"));
                        }
                    }
                    serde_json::Value::String(_) => {}
                    _ => errors.push(format!("{label} must be a string")),
                }
            }
            Some("number") => {
                if let Some(n) = value.as_f64() {
                    if let Some(min) = prop.get("min").and_then(serde_json::Value::as_f64)
                        && n < min
                    {
                        errors.push(format!("{label} must be at least {min}"));
                    }
                    if let Some(max) = prop.get("max").and_then(serde_json::Value::as_f64)
                        && n > max
                    {
                        errors.push(format!("{label} must be at most {max}"));
                    }
                } else {
                    errors.push(format!("{label} must be a number"));
                }
            }
            Some("boolean") => {
                if !value.is_boolean() {
                    errors.push(format!("{label} must be a boolean"));
                }
            }
            Some("file") | Some("directory") => {
                if !value.is_string() {
                    errors.push(format!("{label} must be a path string"));
                }
            }
            _ => {}
        }
    }
    errors
}

/// Expand MCPB template placeholders in a command / arg / env value:
/// `${__dirname}` → the extracted bundle dir, `${user_config.KEY}` → the
/// user-provided value (string values verbatim, others JSON-stringified).
/// Unknown placeholders are left intact.
fn substitute_template(
    raw: &str,
    extracted_dir: &str,
    user_config: &HashMap<String, serde_json::Value>,
) -> String {
    let mut out = raw.replace("${__dirname}", extracted_dir);
    for (k, v) in user_config {
        let placeholder = format!("${{user_config.{k}}}");
        if out.contains(&placeholder) {
            let replacement = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&placeholder, &replacement);
        }
    }
    out
}

fn merge_env(
    base: &HashMap<String, String>,
    extracted_dir: &str,
    user_config: &HashMap<String, serde_json::Value>,
) -> serde_json::Value {
    // Only manifest-declared env values are emitted, with `${user_config.X}`
    // template substitution. plugins#234: the previous loop that exposed EVERY
    // string user_config value under its own env key was removed — it risked
    // leaking sensitive user_config values into the child process environment.
    // Sensitive values belong in the keyring split (Slice B), never in env.
    let merged: HashMap<String, String> = base
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                substitute_template(v, extracted_dir, user_config),
            )
        })
        .collect();
    serde_json::to_value(merged).unwrap_or(serde_json::Value::Null)
}

fn write_metadata(target_dir: &Path, source_url: &str, sha: &str) -> crate::Result<()> {
    let metadata_path = target_dir.join(".mcpb-metadata.json");
    let now = chrono::Utc::now();
    let existing: Option<McpbCacheMetadata> = std::fs::read_to_string(&metadata_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok());
    let metadata = McpbCacheMetadata {
        source_url: source_url.to_string(),
        sha256: sha.to_string(),
        extracted_at: existing.as_ref().map(|m| m.extracted_at).unwrap_or(now),
        last_used: now,
    };
    std::fs::write(&metadata_path, serde_json::to_string_pretty(&metadata)?)?;
    Ok(())
}

#[cfg(test)]
#[path = "mcpb.test.rs"]
mod tests;
