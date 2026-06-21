//! Git-instruction gating.

use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;

/// Whether the git-status block should be included in the system prompt.
///
/// Tri-state, mirroring TS `shouldIncludeGitInstructions()`:
/// 1. `COCO_DISABLE_GIT_INSTRUCTIONS` truthy  → `false` (force off)
/// 2. `COCO_DISABLE_GIT_INSTRUCTIONS` defined-falsy → `true` (force on)
/// 3. otherwise → `settings.include_git_instructions` (default `true`)
///
/// (TS's env var is `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS`; coco uses the
/// `COCO_`-prefixed equivalent.)
pub fn should_include_git_instructions(settings: &Settings, env: &EnvSnapshot) -> bool {
    if env.is_truthy(EnvKey::CocoDisableGitInstructions) {
        return false;
    }
    if env.is_falsy(EnvKey::CocoDisableGitInstructions) {
        return true;
    }
    settings.include_git_instructions.unwrap_or(true)
}

#[cfg(test)]
#[path = "gitsettings.test.rs"]
mod tests;
