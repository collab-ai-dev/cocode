//! Plugin output-style loader.
//!
//! Reads `.md` files from each enabled plugin's `output-styles/` directory
//! plus any manifest-declared extras. Style names are namespaced as
//! `pluginName:baseName`, mirroring how plugin commands and agents are
//! exposed.
//!
//! Plugin styles set [`OutputStyleSource::Plugin`]. The optional
//! `force-for-plugin` frontmatter key is parsed here (only valid on
//! plugin styles); when `Some(true)` the resolver picks this style over
//! `settings.output_style`.

use std::path::Path;
use std::path::PathBuf;

use crate::catalog::OutputStyleConfig;
use crate::catalog::OutputStyleSource;
use crate::dir_loader::build_config_from_parsed;
use crate::dir_loader::coerce_description_to_string;
use crate::dir_loader::description_from_markdown;
use crate::dir_loader::is_markdown_file;
use crate::dir_loader::parse_ts_boolean_frontmatter;
use crate::error::OutputStylesError;

/// Minimal description of an enabled plugin needed to load its output
/// styles. Built by callers from `coco_plugins::loader::LoadedPluginV2`
/// via [`PluginOutputStyleSource::from_loaded_plugin`].
#[derive(Debug, Clone)]
pub struct PluginOutputStyleSource {
    /// Plugin name. Used for the `<name>:<style>` namespacing.
    pub plugin_name: String,
    /// Default `<plugin_root>/output-styles/` directory, if it exists.
    pub default_dir: Option<PathBuf>,
    /// Extra paths from `manifest.outputStyles` (file or directory).
    pub extra_paths: Vec<PathBuf>,
}

impl PluginOutputStyleSource {
    /// Convert a [`coco_plugins::loader::LoadedPluginV2`] to the
    /// loader-facing source description. Pulls:
    /// - The default `<plugin_root>/output-styles/` directory when
    ///   present on disk.
    /// - Every extra path from `manifest.output_styles` (single string
    ///   or array), resolved relative to the plugin root.
    ///
    /// Builds the source description from the loaded plugin
    /// (default dir + manifest `outputStyles` extras).
    pub fn from_loaded_plugin(plugin: &coco_plugins::loader::LoadedPluginV2) -> Self {
        use coco_plugins::schemas::ManifestPaths;
        let default_candidate = plugin.path.join("output-styles");
        let default_dir = if default_candidate.is_dir() {
            Some(default_candidate)
        } else {
            None
        };
        let extra_paths = match &plugin.manifest.output_styles {
            Some(ManifestPaths::Single(p)) => vec![plugin.path.join(p)],
            Some(ManifestPaths::Multiple(paths)) => {
                paths.iter().map(|p| plugin.path.join(p)).collect()
            }
            None => Vec::new(),
        };
        Self {
            plugin_name: plugin.id.name.clone(),
            default_dir,
            extra_paths,
        }
    }
}

/// Load output styles from every plugin in `plugins`. Errors per plugin
/// are logged and that plugin is skipped — the rest still load.
///
/// Names are namespaced as `<plugin_name>:<base_name>`. Duplicate
/// physical file paths within the same plugin are skipped after
/// resolving symlinks.
pub fn load_plugin_output_styles(plugins: &[PluginOutputStyleSource]) -> Vec<OutputStyleConfig> {
    let mut all = Vec::new();
    for plugin in plugins {
        let mut loaded_paths = std::collections::HashSet::new();
        for path in candidate_paths(plugin) {
            all.extend(load_plugin_path(
                &plugin.plugin_name,
                &path,
                &mut loaded_paths,
            ));
        }
    }
    all
}

fn candidate_paths(plugin: &PluginOutputStyleSource) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(dir) = &plugin.default_dir {
        paths.push(dir.clone());
    }
    paths.extend(plugin.extra_paths.iter().cloned());
    paths
}

fn load_plugin_path(
    plugin_name: &str,
    path: &Path,
    loaded_paths: &mut std::collections::HashSet<PathBuf>,
) -> Vec<OutputStyleConfig> {
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };

    if metadata.is_dir() {
        let mut out = Vec::new();
        for entry in walkdir::WalkDir::new(path).into_iter() {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            if !is_markdown_file(p) {
                continue;
            }
            if is_duplicate_path(p, loaded_paths) {
                continue;
            }
            match load_single_plugin_file(plugin_name, p) {
                Ok(style) => out.push(style),
                Err(e) => tracing::warn!(
                    target: "coco_output_styles::plugin_loader",
                    plugin = plugin_name,
                    path = %p.display(),
                    error = %e,
                    "skipping malformed plugin output-style file"
                ),
            }
        }
        out
    } else if metadata.is_file() && is_markdown_file(path) {
        if is_duplicate_path(path, loaded_paths) {
            return Vec::new();
        }
        match load_single_plugin_file(plugin_name, path) {
            Ok(style) => vec![style],
            Err(e) => {
                tracing::warn!(
                    target: "coco_output_styles::plugin_loader",
                    plugin = plugin_name,
                    path = %path.display(),
                    error = %e,
                    "skipping malformed plugin output-style file"
                );
                Vec::new()
            }
        }
    } else {
        Vec::new()
    }
}

fn is_duplicate_path(path: &Path, loaded_paths: &mut std::collections::HashSet<PathBuf>) -> bool {
    let resolved = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    !loaded_paths.insert(resolved)
}

fn load_single_plugin_file(
    plugin_name: &str,
    path: &Path,
) -> Result<OutputStyleConfig, OutputStylesError> {
    let raw = std::fs::read_to_string(path).map_err(|source| OutputStylesError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let parsed = coco_frontmatter::parse(&raw);
    let mut style = build_config_from_parsed(path, &parsed, OutputStyleSource::Plugin);

    // Plugin namespace prefix on the name. Always prefixes the
    // frontmatter/file base name; does not strip an existing prefix.
    style.name = format!("{plugin_name}:{}", style.name);
    style.description = parsed
        .data
        .get("description")
        .and_then(coerce_description_to_string)
        .unwrap_or_else(|| {
            description_from_markdown(
                &parsed.content,
                &format!("Output style from {plugin_name} plugin"),
            )
        });

    // `force-for-plugin` only valid here.
    style.force_for_plugin = parsed
        .data
        .get("force-for-plugin")
        .and_then(parse_ts_boolean_frontmatter);

    // `keep-coding-instructions` is dir-style-only — clear if accidentally set.
    style.keep_coding_instructions = None;

    Ok(style)
}

#[cfg(test)]
#[path = "plugin_loader.test.rs"]
mod tests;
