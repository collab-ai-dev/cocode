//! Output style catalog types.

use serde::Deserialize;
use serde::Serialize;

/// A single output style definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputStyleConfig {
    /// Display name. Used both as the catalog key and shown in pickers
    /// and the per-turn reminder. For plugin styles this is namespaced
    /// as `pluginName:baseName`.
    pub name: String,

    /// Short description shown in `/config` and the picker.
    pub description: String,

    /// The system-prompt body injected when this style is active. Empty
    /// for the sentinel `default` style (so the prompt section stays
    /// absent when no style is active — see [`builtin::builtin_styles`]).
    pub prompt: String,

    /// Where the style was loaded from.
    pub source: OutputStyleSource,

    /// When `Some(true)`, the standard "Doing tasks" coding instructions
    /// stay on top of this style. When `Some(false)` or `None`, the
    /// style fully replaces them. Frontmatter key: `keep-coding-instructions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_coding_instructions: Option<bool>,

    /// Plugin-only: when `Some(true)`, the style is force-applied as
    /// long as the plugin is enabled, overriding `settings.output_style`.
    /// Frontmatter key: `force-for-plugin`. Parsed-but-ignored on
    /// non-plugin sources (with a warning in the dir loader).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_for_plugin: Option<bool>,
}

/// Where this output style came from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum OutputStyleSource {
    /// Shipped in the binary.
    BuiltIn,
    /// Loaded from a plugin.
    Plugin,
    /// Managed/policy directory (`/etc/coco/...`, organization-pushed).
    PolicySettings,
    /// User home `~/.coco/output-styles/`.
    UserSettings,
    /// Project `.coco/output-styles/`.
    ProjectSettings,
}

impl OutputStyleSource {
    /// Aggregation priority — higher value wins when names collide.
    /// Built-ins are lowest priority; groups are layered `plugin → user →
    /// project → managed` with managed being highest.
    pub const fn priority(self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            Self::Plugin => 1,
            Self::UserSettings => 2,
            Self::ProjectSettings => 3,
            Self::PolicySettings => 4,
        }
    }
}
