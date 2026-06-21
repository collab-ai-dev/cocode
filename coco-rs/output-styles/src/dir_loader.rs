//! Markdown directory loader for output styles.
//!
//! Reads `.md` files from one or more directories recursively, parses YAML
//! frontmatter, and emits one [`OutputStyleConfig`] per file. The filename
//! stem is the default style name; `name`, `description`, and
//! `keep-coding-instructions` frontmatter fields override or supplement the
//! defaults.
//!
//! `force-for-plugin` is only meaningful on plugin styles; finding it here
//! logs a warning and ignores the value.
//!
//! The (dir, source) pair is threaded in from the caller — paths are resolved
//! at the CLI bootstrap layer where `~/.coco/`, the project tree, and
//! managed/policy locations are known.

use std::path::Path;

use coco_frontmatter::Frontmatter;
use coco_frontmatter::FrontmatterValue;
use walkdir::WalkDir;

use crate::catalog::OutputStyleConfig;
use crate::catalog::OutputStyleSource;
use crate::error::OutputStylesError;

/// Load every `.md` output-style file recursively under `dir`.
///
/// Returns an empty `Vec` if `dir` doesn't exist or isn't readable —
/// "fail open" so a missing config dir never breaks bootstrap. Per-file
/// parse errors are logged at `warn` level and the offending file is skipped.
///
/// `source` is attached to each loaded style and drives priority during
/// aggregation. Use [`OutputStyleSource::UserSettings`] for `~/.coco`,
/// [`OutputStyleSource::ProjectSettings`] for `<cwd>/.coco/...`, and
/// [`OutputStyleSource::PolicySettings`] for the managed location.
pub fn load_dir_styles(dir: &Path, source: OutputStyleSource) -> Vec<OutputStyleConfig> {
    load_dir_styles_with_identity(dir, source)
        .into_iter()
        .map(|loaded| loaded.config)
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct LoadedDirStyle {
    pub config: OutputStyleConfig,
    pub file_identity: Option<String>,
}

pub(crate) fn load_dir_styles_with_identity(
    dir: &Path,
    source: OutputStyleSource,
) -> Vec<LoadedDirStyle> {
    let mut out = Vec::new();
    for entry in WalkDir::new(dir).follow_links(true).into_iter() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if !is_markdown_file(path) {
            continue;
        }
        match load_single(path, source) {
            Ok(style) => out.push(LoadedDirStyle {
                config: style,
                file_identity: file_identity(path),
            }),
            Err(e) => tracing::warn!(
                target: "coco_output_styles::dir_loader",
                path = %path.display(),
                error = %e,
                "skipping malformed output-style file"
            ),
        }
    }
    out
}

pub(crate) fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Load a single output-style markdown file. Public for tests.
pub fn load_single(
    path: &Path,
    source: OutputStyleSource,
) -> Result<OutputStyleConfig, OutputStylesError> {
    let raw = std::fs::read_to_string(path).map_err(|source| OutputStylesError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed = coco_frontmatter::parse(&raw);
    Ok(build_config_from_parsed(path, &parsed, source))
}

/// Build an [`OutputStyleConfig`] from already-parsed frontmatter +
/// content. Public so the plugin loader can reuse the same field
/// extraction without re-reading the file.
pub fn build_config_from_parsed(
    path: &Path,
    parsed: &Frontmatter,
    source: OutputStyleSource,
) -> OutputStyleConfig {
    let stem = filename_stem(path);

    // `name` fallback: filename stem.
    let name = parsed
        .data
        .get("name")
        .and_then(FrontmatterValue::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| stem.clone());

    let description = parsed
        .data
        .get("description")
        .and_then(coerce_description_to_string)
        .unwrap_or_else(|| {
            description_from_markdown(&parsed.content, &format!("Custom {stem} output style"))
        });

    // `keep-coding-instructions` accepts both bool and stringly-typed bool.
    let keep_coding_instructions = parsed
        .data
        .get("keep-coding-instructions")
        .and_then(parse_ts_boolean_frontmatter);

    // `force-for-plugin` warning when present on non-plugin style.
    // Plugin loader handles the real read.
    if !matches!(source, OutputStyleSource::Plugin) && parsed.data.contains_key("force-for-plugin")
    {
        tracing::warn!(
            target: "coco_output_styles::dir_loader",
            path = %path.display(),
            style = %name,
            "`force-for-plugin` only applies to plugin output styles; ignoring"
        );
    }

    OutputStyleConfig {
        name,
        description,
        prompt: parsed.content.trim().to_string(),
        source,
        keep_coding_instructions,
        force_for_plugin: None,
    }
}

fn filename_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// First non-empty line of the markdown content, with leading `#`s
/// stripped, capped at 100 chars.
pub(crate) fn coerce_description_to_string(value: &FrontmatterValue) -> Option<String> {
    match value {
        FrontmatterValue::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        FrontmatterValue::Bool(b) => Some(b.to_string()),
        FrontmatterValue::Int(n) => Some(n.to_string()),
        FrontmatterValue::Float(n) => Some(n.to_string()),
        FrontmatterValue::Sequence(_) | FrontmatterValue::Mapping(_) | FrontmatterValue::Null => {
            None
        }
    }
}

pub(crate) fn description_from_markdown(content: &str, fallback: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let text = if let Some(stripped) = trimmed.strip_prefix('#') {
            // Strip any number of leading '#' plus the space after.
            let mut s = stripped;
            while let Some(rest) = s.strip_prefix('#') {
                s = rest;
            }
            s.trim_start().to_string()
        } else {
            trimmed.to_string()
        };
        return if text.chars().count() > 100 {
            let mut truncated: String = text.chars().take(97).collect();
            truncated.push_str("...");
            truncated
        } else {
            text
        };
    }
    fallback.to_string()
}

pub(crate) fn parse_ts_boolean_frontmatter(value: &FrontmatterValue) -> Option<bool> {
    match value {
        FrontmatterValue::Bool(value) => Some(*value),
        FrontmatterValue::String(value) if value == "true" => Some(true),
        FrontmatterValue::String(value) if value == "false" => Some(false),
        _ => None,
    }
}

fn file_identity(path: &Path) -> Option<String> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    file_identity_from_metadata(path, &metadata)
}

#[cfg(unix)]
fn file_identity_from_metadata(_path: &Path, metadata: &std::fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let dev = metadata.dev();
    let ino = metadata.ino();
    if dev == 0 && ino == 0 {
        None
    } else {
        Some(format!("{dev}:{ino}"))
    }
}

#[cfg(not(unix))]
fn file_identity_from_metadata(path: &Path, _metadata: &std::fs::Metadata) -> Option<String> {
    path.canonicalize()
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

#[cfg(test)]
#[path = "dir_loader.test.rs"]
mod tests;
