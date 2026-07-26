//! How this binary got onto the machine, and therefore how to replace it.
//!
//! Detection is by executable path, because that is the one signal that is
//! true regardless of which shell, which package manager wrapper, or which
//! version manager the user has in front of it. Every rule below is a substring
//! of the *canonical* path, so a symlink from `~/.local/bin` does not hide an
//! npm install.
//!
//! An unrecognized layout returns [`InstallMethod::Unknown`], and an unknown
//! method offers no command. Printing a plausible-looking upgrade line that
//! fails is worse than printing none: the user runs it, it errors, and now they
//! distrust the notice as well.

use std::path::Path;
use std::path::PathBuf;

/// The distribution channel this binary came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Npm,
    Pnpm,
    Bun,
    Homebrew,
    Cargo,
    Unknown,
}

impl InstallMethod {
    /// Detect from the running executable's path.
    pub fn detect() -> Self {
        std::env::current_exe()
            .ok()
            .and_then(|path| std::fs::canonicalize(&path).ok().or(Some(path)))
            .map_or(Self::Unknown, |path| Self::from_path(&path))
    }

    /// Classify an executable path. Split out from [`Self::detect`] so the
    /// rules are testable without installing coco six ways.
    pub fn from_path(path: &Path) -> Self {
        let normalized = normalize(path);
        // Order matters: pnpm and bun keep their stores under paths that also
        // contain "node_modules", so they must be checked before npm.
        if normalized.contains("/.bun/") || normalized.contains("/bun/install/") {
            return Self::Bun;
        }
        if normalized.contains("/pnpm/") || normalized.contains("/.pnpm/") {
            return Self::Pnpm;
        }
        if normalized.contains("/node_modules/") || normalized.contains("/npm/") {
            return Self::Npm;
        }
        if normalized.contains("/homebrew/") || normalized.contains("/cellar/") {
            return Self::Homebrew;
        }
        if normalized.contains("/.cargo/bin/") {
            return Self::Cargo;
        }
        Self::Unknown
    }

    /// The command that upgrades an installation of this kind.
    pub fn upgrade_command(self) -> Option<String> {
        let command = match self {
            Self::Npm => "npm install -g @cocode-cli/cocode-cli@latest",
            Self::Pnpm => "pnpm add -g @cocode-cli/cocode-cli@latest",
            Self::Bun => "bun install -g @cocode-cli/cocode-cli@latest",
            Self::Homebrew => "brew upgrade cocode",
            Self::Cargo => "cargo install coco-cli --force",
            Self::Unknown => return None,
        };
        Some(command.to_string())
    }
}

/// Lowercased, forward-slashed path with a trailing separator, so every rule
/// can match on `/segment/` and cannot half-match a longer name.
fn normalize(path: &Path) -> String {
    let mut normalized = PathBuf::from(path)
        .to_string_lossy()
        .replace('\\', "/")
        .to_lowercase();
    normalized.push('/');
    normalized
}

#[cfg(test)]
#[path = "install.test.rs"]
mod tests;
