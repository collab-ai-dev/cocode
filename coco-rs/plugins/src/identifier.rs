//! Plugin identifiers and parsing.
//!
//! Format: `name@marketplace`. The `@` separator distinguishes plugin name
//! from its marketplace; both segments can contain alphanumerics, dashes,
//! underscores. Bare names (no `@`) are valid in dependency declarations
//! and inherit the declarer's marketplace at qualification time.

use serde::Deserialize;
use serde::Serialize;
use std::fmt;

/// Synthetic marketplace sentinel for `--plugin-dir` plugins.
pub const INLINE_MARKETPLACE: &str = "inline";

/// Builtin marketplace sentinel for compiled-in plugins.
pub const BUILTIN_MARKETPLACE: &str = "builtin";

/// Strongly-typed plugin identifier.
///
/// Wire form: `"name@marketplace"`. Serializes via Display/FromStr.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginId {
    pub name: String,
    pub marketplace: Option<String>,
}

impl PluginId {
    pub fn new(name: impl Into<String>, marketplace: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            marketplace: Some(marketplace.into()),
        }
    }

    pub fn bare(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            marketplace: None,
        }
    }

    /// Parse `name@marketplace` form. Bare names map to `marketplace: None`.
    pub fn parse(input: &str) -> Self {
        if let Some((name, marketplace)) = input.rsplit_once('@') {
            Self {
                name: name.to_string(),
                marketplace: if marketplace.is_empty() {
                    None
                } else {
                    Some(marketplace.to_string())
                },
            }
        } else {
            Self::bare(input)
        }
    }

    /// Whether this id refers to a builtin plugin (`<name>@builtin`).
    pub fn is_builtin(&self) -> bool {
        matches!(&self.marketplace, Some(m) if m == BUILTIN_MARKETPLACE)
    }

    /// Whether this id is from a `--plugin-dir` source.
    pub fn is_inline(&self) -> bool {
        matches!(&self.marketplace, Some(m) if m == INLINE_MARKETPLACE)
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.marketplace {
            Some(m) => write!(f, "{}@{}", self.name, m),
            None => write!(f, "{}", self.name),
        }
    }
}

impl Serialize for PluginId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Ok(Self::parse(&raw))
    }
}

/// Plugin-scope ordering: priority high-to-low (Managed wins).
///
/// Higher scope wins on name collisions; `Managed` cannot be disabled by users.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginScope {
    /// Local development via `--plugin-dir`.
    Local = 0,
    /// `<cwd>/.coco/plugins/`.
    Project = 1,
    /// `~/.coco/plugins/`.
    User = 2,
    /// Enterprise/policy-managed plugins. Cannot be disabled by users.
    Managed = 3,
}

impl PluginScope {
    pub fn as_str(self) -> &'static str {
        match self {
            PluginScope::Local => "local",
            PluginScope::Project => "project",
            PluginScope::User => "user",
            PluginScope::Managed => "managed",
        }
    }
}

#[cfg(test)]
#[path = "identifier.test.rs"]
mod tests;
