//! Configuration for ignore behavior.

/// Configuration for file ignore behavior.
///
/// Controls which ignore files are respected and how files are filtered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreConfig {
    /// Whether to respect .gitignore files.
    ///
    /// When `true`, applies rules from:
    /// - `.gitignore` files in the directory tree
    /// - Global gitignore (`~/.config/git/ignore`)
    /// - `.git/info/exclude`
    ///
    /// Default: `true`
    pub respect_gitignore: bool,

    /// Whether to respect `.ignore` files (ripgrep native support).
    ///
    /// Uses the same syntax as `.gitignore`.
    ///
    /// Default: `true`
    pub respect_ignore: bool,

    /// Whether to respect `.agentignore` files (agent-level exclusions).
    ///
    /// `.agentignore` uses gitignore syntax but is honored **independently**
    /// of [`Self::respect_gitignore`] / [`Self::respect_ignore`] — it stays
    /// in force even in the Glob tool's `--no-ignore` discovery mode. This is
    /// the "hide from the AI agent" mechanism for checked-in files (secrets,
    /// fixtures, generated artefacts) that the user wants kept out of the
    /// model's view without affecting git tooling.
    ///
    /// Default: `true`
    pub respect_agentignore: bool,

    /// Whether to include hidden files (dotfiles).
    ///
    /// When `false`, files and directories starting with `.` are excluded
    /// (unless explicitly un-ignored).
    ///
    /// Default: `false`
    pub include_hidden: bool,

    /// Whether to follow symbolic links.
    ///
    /// When `true`, symbolic links are followed and their targets are included.
    /// Be careful of cycles when enabling this.
    ///
    /// Default: `false`
    pub follow_links: bool,

    /// Additional custom exclude patterns.
    ///
    /// Uses gitignore syntax (e.g., `*.log`, `**/temp/**`).
    /// These patterns are applied in addition to ignore files.
    pub custom_excludes: Vec<String>,
}

impl Default for IgnoreConfig {
    fn default() -> Self {
        Self {
            respect_gitignore: true,
            respect_ignore: true,
            respect_agentignore: true,
            include_hidden: false,
            follow_links: false,
            custom_excludes: Vec::new(),
        }
    }
}

impl IgnoreConfig {
    /// Create a config that respects all ignore files.
    pub fn respecting_all() -> Self {
        Self::default()
    }

    /// Create a config that ignores all ignore files (show everything),
    /// including `.agentignore`.
    pub fn ignoring_none() -> Self {
        Self {
            respect_gitignore: false,
            respect_ignore: false,
            respect_agentignore: false,
            include_hidden: true,
            follow_links: false,
            custom_excludes: Vec::new(),
        }
    }

    /// Glob-tool discovery config: matches the TS reference's
    /// `--no-ignore --hidden` (ignore `.gitignore` / `.ignore`, show hidden)
    /// while keeping `.agentignore` in force so agent-hidden files stay hidden
    /// even during broad file discovery.
    pub fn for_glob_discovery() -> Self {
        Self {
            respect_gitignore: false,
            respect_ignore: false,
            respect_agentignore: true,
            include_hidden: true,
            follow_links: false,
            custom_excludes: Vec::new(),
        }
    }

    /// Builder method: set whether to respect gitignore.
    pub fn with_gitignore(mut self, respect: bool) -> Self {
        self.respect_gitignore = respect;
        self
    }

    /// Builder method: set whether to respect `.ignore` files.
    pub fn with_ignore(mut self, respect: bool) -> Self {
        self.respect_ignore = respect;
        self
    }

    /// Builder method: set whether to respect `.agentignore` files.
    pub fn with_agentignore(mut self, respect: bool) -> Self {
        self.respect_agentignore = respect;
        self
    }

    /// Builder method: set whether to include hidden files.
    pub fn with_hidden(mut self, include: bool) -> Self {
        self.include_hidden = include;
        self
    }

    /// Builder method: set whether to follow symlinks.
    pub fn with_follow_links(mut self, follow: bool) -> Self {
        self.follow_links = follow;
        self
    }

    /// Builder method: add custom exclude patterns.
    pub fn with_excludes(mut self, excludes: Vec<String>) -> Self {
        self.custom_excludes = excludes;
        self
    }
}

#[cfg(test)]
#[path = "config.test.rs"]
mod tests;
