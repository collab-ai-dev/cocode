//! The skill-review write fence.
//!
//! [`SkillWriteHandle`] is the `CanUseToolHandle` threaded onto the review
//! fork's `AgentSpawnRequest.can_use_tool`. It confines the fork to writing
//! **under the agent skills root** — the spatial provenance boundary. Because
//! agent skills live in their own directory, the fence is pure containment: it
//! does not read target frontmatter (no per-write I/O, no TOCTOU), it just
//! checks the path is inside `agent_root` via the shared symlink-aware L0
//! primitive.
//!
//! Policy:
//! - `Read` / `Glob` / `Grep` ⇒ Allow (the fork must read existing skills to
//!   patch them).
//! - `Bash` ⇒ Allow iff the command is a read-only pipeline
//!   ([`coco_shell_parser::is_read_only_pipeline`]); no mutation, ever.
//! - `Edit` / `Write` / `apply_patch` ⇒ Allow iff every affected path is
//!   contained by `agent_root` ([`coco_utils_absolute_path::contains_symlink_aware`],
//!   fail-closed on traversal / symlink escape) AND passes the filename
//!   policy: no hidden (dot-prefixed) components, documentation-class
//!   extensions only. Loop metadata (curator lock, promotions store) lives
//!   outside the root, but the dotfile deny keeps the fork from planting
//!   anything invisible to discovery; the extension allowlist keeps scripts
//!   and binaries out of a library that loads inert anyway.
//! - Everything else ⇒ Deny.
//!
//! Composes as the inner ring with `AgentSpawnConstraints.allowed_write_roots`
//! (outer ring). The fork writes skill files (SKILL.md + support files); the
//! *contents* are made inert on load by `coco-skills`' location-keyed
//! agent-scope enforcement.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use coco_tool_runtime::{
    CanUseToolCallContext, CanUseToolDecision, CanUseToolHandle, CanUseToolHandleRef,
    DecisionReason,
};
use coco_types::ToolName;
use serde_json::Value;

const TOOL_READ: &str = ToolName::Read.as_str();
const TOOL_GLOB: &str = ToolName::Glob.as_str();
const TOOL_GREP: &str = ToolName::Grep.as_str();
const TOOL_BASH: &str = ToolName::Bash.as_str();
const TOOL_EDIT: &str = ToolName::Edit.as_str();
const TOOL_WRITE: &str = ToolName::Write.as_str();
const TOOL_APPLY_PATCH: &str = ToolName::ApplyPatch.as_str();

/// Extensions a review fork may write. Agent skills load inert, so scripts
/// would never execute — rejecting them anyway keeps junk out of the library.
const ALLOWED_WRITE_EXTENSIONS: &[&str] = &["md", "txt", "json", "yaml", "yml", "toml"];

/// Build the skill-review write fence rooted at `agent_root`
/// (`<config_home>/skills/.agent`).
pub fn create_skill_write_handle(agent_root: PathBuf) -> CanUseToolHandleRef {
    Arc::new(SkillWriteHandle { agent_root })
}

/// `CanUseToolHandle` confining a skill-review fork to the agent skills root.
pub struct SkillWriteHandle {
    agent_root: PathBuf,
}

impl std::fmt::Debug for SkillWriteHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillWriteHandle")
            .field("agent_root", &self.agent_root)
            .finish()
    }
}

#[async_trait]
impl CanUseToolHandle for SkillWriteHandle {
    async fn check(
        &self,
        tool_name: &str,
        input: &Value,
        ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        match tool_name {
            TOOL_READ | TOOL_GLOB | TOOL_GREP => allow(format!("skill_review: {tool_name} read")),
            TOOL_BASH => {
                if bash_is_read_only(input) {
                    allow("skill_review: read-only bash".into())
                } else {
                    deny(
                        "skill_review: bash command not read-only".into(),
                        "skill_review_bash_mutating",
                    )
                }
            }
            TOOL_EDIT | TOOL_WRITE => {
                if input_path_under_root(input, &self.agent_root, &ctx.cwd) {
                    allow("skill_review: write within agent skills dir".into())
                } else {
                    deny(
                        format!(
                            "skill_review: {tool_name} only allowed under {}",
                            self.agent_root.display()
                        ),
                        "skill_review_write_outside_dir",
                    )
                }
            }
            TOOL_APPLY_PATCH => {
                if apply_patch_paths_under_root(input, &self.agent_root, &ctx.cwd) {
                    allow("skill_review: patch within agent skills dir".into())
                } else {
                    deny(
                        format!(
                            "skill_review: apply_patch only allowed under {}",
                            self.agent_root.display()
                        ),
                        "skill_review_write_outside_dir",
                    )
                }
            }
            other => deny(
                format!("skill_review: tool '{other}' not in policy"),
                "skill_review_unknown_tool",
            ),
        }
    }
}

fn allow(reason: String) -> CanUseToolDecision {
    CanUseToolDecision::Allow {
        updated_input: None,
        decision_reason: DecisionReason::Other { reason },
    }
}

fn deny(message: String, reason_label: &str) -> CanUseToolDecision {
    CanUseToolDecision::Deny {
        message,
        decision_reason: DecisionReason::Other {
            reason: reason_label.to_string(),
        },
    }
}

fn bash_is_read_only(input: &Value) -> bool {
    let Some(cmd) = input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    coco_shell_parser::is_read_only_pipeline(cmd)
}

fn input_path_under_root(input: &Value, root: &Path, cwd: &Path) -> bool {
    coco_background_review::input_write_target(input, cwd)
        .is_some_and(|absolute| write_target_allowed(root, &absolute))
}

/// Containment + filename policy for a single write target. Fail-closed on
/// every ambiguity (non-UTF-8 components, un-strippable prefix, no/unknown
/// extension).
fn write_target_allowed(root: &Path, absolute: &Path) -> bool {
    if !coco_utils_absolute_path::contains_symlink_aware(root, absolute) {
        return false;
    }
    // Containment normalizes `..`/symlinks; a raw path that doesn't strip
    // lexically is suspicious — deny rather than reason about it.
    let Ok(rel) = absolute.strip_prefix(root) else {
        return false;
    };
    let no_hidden = rel.components().all(|c| {
        c.as_os_str()
            .to_str()
            .is_some_and(|s| !s.is_empty() && !s.starts_with('.'))
    });
    if !no_hidden {
        return false;
    }
    absolute
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ALLOWED_WRITE_EXTENSIONS
                .iter()
                .any(|a| a.eq_ignore_ascii_case(e))
        })
}

fn apply_patch_paths_under_root(input: &Value, root: &Path, cwd: &Path) -> bool {
    coco_background_review::apply_patch_write_targets(input, cwd)
        .is_some_and(|paths| paths.iter().all(|path| write_target_allowed(root, path)))
}

#[cfg(test)]
#[path = "fence.test.rs"]
mod tests;
