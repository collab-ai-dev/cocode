use serde::Deserialize;
use serde::Serialize;

/// Where a setting came from. Used for conflict resolution and security.
/// Ordered by priority (Plugin lowest, Policy highest).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingSource {
    /// Plugin-contributed base settings (lowest priority).
    Plugin,
    /// ~/.coco/settings.json (global per machine).
    User,
    /// .coco/settings.json (shared, checked in).
    Project,
    /// .coco/settings.local.json (gitignored).
    Local,
    /// --settings CLI file or SDK inline.
    Flag,
    /// Enterprise/MDM managed (highest priority).
    Policy,
}

impl SettingSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Plugin => "plugin",
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Flag => "flag",
            Self::Policy => "policy",
        }
    }
}

impl std::fmt::Display for SettingSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
