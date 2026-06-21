//! EnterWorktree + ExitWorktree tools — git worktree isolation.
//!
//! Worktree tools let the model (usually an agent) work in an isolated
//! git worktree — create a branch, modify files there, and tear it
//! down. The process CWD (and `session_cwd`) restoration is handled here.
//!
//! coco-rs does not memoize: `app/query::build_prompt` re-runs
//! `coco_context::discover_memory_files(cwd)` every turn from the live
//! process cwd, and the plans dir / system prompt are recomputed per turn.
//! So changing the cwd here is sufficient — there is no cache to invalidate.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::path::PathBuf;

/// Model-facing tool prompt. The path / brand details are adapted to coco-rs
/// (worktrees live under `../worktrees/`, isolation is git-only) — see this
/// module's header for why cache-invalidation is a no-op here.
const ENTER_WORKTREE_PROMPT: &str = r#"Use this tool ONLY when the user explicitly asks to work in a worktree. This tool creates an isolated git worktree and switches the current session into it.

## When to Use

- The user explicitly says "worktree" (e.g., "start a worktree", "work in a worktree", "create a worktree", "use a worktree")

## When NOT to Use

- The user asks to create a branch, switch branches, or work on a different branch — use git commands instead
- The user asks to fix a bug or work on a feature — use normal git workflow unless they specifically mention worktrees
- Never use this tool unless the user explicitly mentions "worktree"

## Requirements

- Must be in a git repository
- Must not already be in a worktree

## Behavior

- Creates a new git worktree inside `../worktrees/` with a new branch based on the repository's default branch
- Switches the session's working directory to the new worktree
- Use ExitWorktree to leave the worktree mid-session (keep or remove). On session exit, if still in the worktree, the user will be prompted to keep or remove it

## Parameters

- `name` (optional): A name for the worktree. If not provided, a random name is generated.
"#;

/// Model-facing tool prompt. The tmux note is dropped (coco-rs worktrees have
/// no tmux session) and the cache-clearing detail is adapted (no memoized
/// caches to clear here).
const EXIT_WORKTREE_PROMPT: &str = r#"Exit a worktree session created by EnterWorktree and return the session to the original working directory.

## Scope

This tool ONLY operates on worktrees created by EnterWorktree in this session. It will NOT touch:
- Worktrees you created manually with `git worktree add`
- Worktrees from a previous session (even if created by EnterWorktree then)
- The directory you're in if EnterWorktree was never called

If called outside an EnterWorktree session, the tool is a **no-op**: it reports that no worktree session is active and takes no action. Filesystem state is unchanged.

## When to Use

- The user explicitly asks to "exit the worktree", "leave the worktree", "go back", or otherwise end the worktree session
- Do NOT call this proactively — only when the user asks

## Parameters

- `action` (required): `"keep"` or `"remove"`
  - `"keep"` — leave the worktree directory and branch intact on disk. Use this if the user wants to come back to the work later, or if there are changes to preserve.
  - `"remove"` — delete the worktree directory and its branch. Use this for a clean exit when the work is done or abandoned.
- `discard_changes` (optional, default false): only meaningful with `action: "remove"`. If the worktree has uncommitted files or commits not on the original branch, the tool will REFUSE to remove it unless this is set to `true`. If the tool returns an error listing changes, confirm with the user before re-invoking with `discard_changes: true`.

## Behavior

- Restores the session's working directory to where it was before EnterWorktree
- Once exited, EnterWorktree can be called again to create a fresh worktree
"#;

// ── EnterWorktreeTool ──

/// Typed input for [`EnterWorktreeTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct EnterWorktreeInput {
    /// Optional name for the worktree. Each "/"-separated segment may contain
    /// only letters, digits, dots, underscores, and dashes; max 64 chars
    /// total. A random name is generated if not provided.
    #[serde(default)]
    pub name: Option<String>,
}

/// Typed output for [`EnterWorktreeTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnterWorktreeOutput {
    pub worktree_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    pub message: String,
}

pub struct EnterWorktreeTool;

#[async_trait::async_trait]
impl Tool for EnterWorktreeTool {
    type Input = EnterWorktreeInput;
    coco_tool_runtime::impl_runtime_schema!(EnterWorktreeInput);
    type Output = EnterWorktreeOutput;

    fn to_auto_classifier_input(&self, input: &EnterWorktreeInput) -> Option<String> {
        Some(input.name.clone().unwrap_or_default())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::EnterWorktree)
    }
    fn name(&self) -> &str {
        ToolName::EnterWorktree.as_str()
    }
    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::Worktree)
    }
    fn description(&self, _input: &EnterWorktreeInput, _options: &DescriptionOptions) -> String {
        "Creates an isolated worktree (via git or configured hooks) and switches the session into it"
            .into()
    }
    /// The model's tool description is sourced from `prompt()`, not the
    /// short `description()` UI label.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        ENTER_WORKTREE_PROMPT.to_string()
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("create an isolated git worktree and switch into it")
    }

    /// Emit the prebuilt `message` field as plain text — the model
    /// doesn't need the path/branch fields separately, they're already
    /// in the message. Skips JSON envelope overhead.
    fn render_for_model(&self, out: &EnterWorktreeOutput) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.message.clone(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: EnterWorktreeInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<EnterWorktreeOutput>, ToolError> {
        let app_state = ctx
            .app_state
            .as_ref()
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Cannot enter a worktree because app state is unavailable".into(),
                error_code: None,
            })?;

        // Refuse to nest worktree sessions — doing so would lose the
        // original cwd needed to restore on exit.
        if app_state.read().await.active_worktree.is_some() {
            return Err(ToolError::InvalidInput {
                message: "Already in a worktree session. Exit the current worktree before \
                          entering another."
                    .into(),
                error_code: None,
            });
        }

        // Reject path-traversal in the provided name BEFORE slugifying or
        // touching git. (A `None` name falls through to the generated slug,
        // always valid.)
        if let Some(name) = input
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            validate_worktree_slug(name)?;
        }

        let slug = worktree_slug(input.name.as_deref());
        if slug.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "name must include at least one ASCII letter or digit".into(),
                error_code: None,
            });
        }

        let branch = format!("{}{slug}", coco_types::AGENT_WORKTREE_BRANCH_PREFIX);
        let current_cwd = if let Some(session_cwd) = &ctx.session_cwd {
            session_cwd.read().await.clone()
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };
        let worktree_path = current_cwd.join("..").join("worktrees").join(&slug);
        let worktree_path_string = worktree_path.to_string_lossy().to_string();

        // Resolve the base branch the worktree branches FROM: resolve the repo's
        // default branch, prefer the local `origin/<default>` ref, else fetch it
        // (no-prompt), else fall back to the current `HEAD`. `original_head_commit`
        // is the resolved base SHA, so ExitWorktree's `discardedCommits` counts
        // the worktree's commits ahead of the default-branch baseline.
        let default_branch = coco_git::get_default_branch(&current_cwd);
        let origin_ref = format!("origin/{default_branch}");
        let base_branch = {
            let local_ok = tokio::process::Command::new("git")
                .current_dir(&current_cwd)
                .args(["rev-parse", "--verify", "--quiet", &origin_ref])
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false);
            if local_ok {
                origin_ref.clone()
            } else {
                // Fetch the default branch (no credential prompt). On failure the
                // base falls back to the current HEAD (e.g. a repo with no remote).
                let fetched = tokio::process::Command::new("git")
                    .current_dir(&current_cwd)
                    .env("GIT_TERMINAL_PROMPT", "0")
                    .env("GIT_ASKPASS", "")
                    .args(["fetch", "origin", &default_branch])
                    .output()
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);
                if fetched {
                    origin_ref.clone()
                } else {
                    "HEAD".to_string()
                }
            }
        };
        // Base SHA = `original_head_commit` (the discardedCommits baseline).
        let original_head_commit = tokio::process::Command::new("git")
            .current_dir(&current_cwd)
            .args(["rev-parse", base_branch.as_str()])
            .output()
            .await
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        // Create from the resolved base with `-B` (reset any orphan branch left
        // by a prior removed worktree dir).
        let output = tokio::process::Command::new("git")
            .current_dir(&current_cwd)
            .args([
                "worktree",
                "add",
                "-B",
                &branch,
                &worktree_path_string,
                base_branch.as_str(),
            ])
            .output()
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to run git worktree add: {e}"),
                display_data: None,
                source: None,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ToolError::ExecutionFailed {
                message: format!("git worktree add failed: {stderr}"),
                display_data: None,
                source: None,
            });
        }

        let abs_worktree = std::fs::canonicalize(&worktree_path).unwrap_or(worktree_path);
        let state = coco_types::ActiveWorktreeState {
            original_cwd: current_cwd,
            worktree_path: abs_worktree.clone(),
            worktree_branch: Some(branch.clone()),
            original_head_commit,
        };
        {
            let mut guard = app_state.write().await;
            guard.active_worktree = Some(state.clone());
        }

        if let Err(e) = std::env::set_current_dir(&abs_worktree) {
            {
                let mut guard = app_state.write().await;
                guard.active_worktree = None;
            }
            let _ = tokio::process::Command::new("git")
                .current_dir(&state.original_cwd)
                .args(["worktree", "remove", "--force", &worktree_path_string])
                .output()
                .await;
            return Err(ToolError::ExecutionFailed {
                message: format!("Created worktree but failed to enter it: {e}"),
                display_data: None,
                source: None,
            });
        }
        if let Some(session_cwd) = &ctx.session_cwd {
            *session_cwd.write().await = abs_worktree.clone();
        }

        Ok(ToolResult {
            data: EnterWorktreeOutput {
                worktree_path: abs_worktree.display().to_string(),
                worktree_branch: Some(branch.clone()),
                message: format!(
                    "Created and entered worktree at '{}' on branch '{}' (based on {})",
                    abs_worktree.display(),
                    branch,
                    base_branch
                ),
            },
            new_messages: vec![],
            app_state_patch: Some(Box::new(move |app_state| {
                app_state.active_worktree = Some(state);
            })),
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

// ── ExitWorktreeTool ──
//
// The active worktree target lives in session app state, so the model only
// chooses whether to keep or remove it. This tool restores the process/session
// cwd before optional removal so later shell commands do not inherit a removed
// directory.

/// Typed input for [`ExitWorktreeTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ExitWorktreeInput {
    /// "keep" leaves the worktree and branch on disk; "remove" deletes both.
    pub action: ExitWorktreeAction,
    /// Required true when action is "remove" and the worktree has uncommitted
    /// files or unmerged commits. The tool will refuse and list them otherwise.
    #[serde(default)]
    pub discard_changes: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExitWorktreeAction {
    #[default]
    Keep,
    Remove,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExitWorktreeOutput {
    /// Whether the worktree was kept or removed.
    #[serde(default)]
    pub action: ExitWorktreeAction,
    /// The cwd the session was restored to.
    #[serde(default)]
    pub original_cwd: String,
    pub worktree_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// Uncommitted files discarded by `action: "remove"`. `None` when the
    /// worktree was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discarded_files: Option<i64>,
    /// Commits ahead of base discarded by `action: "remove"`. `None` when
    /// the worktree was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discarded_commits: Option<i64>,
    pub message: String,
}

pub struct ExitWorktreeTool;

#[async_trait::async_trait]
impl Tool for ExitWorktreeTool {
    type Input = ExitWorktreeInput;
    coco_tool_runtime::impl_runtime_schema!(ExitWorktreeInput);
    type Output = ExitWorktreeOutput;

    fn to_auto_classifier_input(&self, input: &ExitWorktreeInput) -> Option<String> {
        // `discard_changes` permits removing a dirty worktree — that's the
        // security-relevant signal, so surface it alongside the action.
        Some(if input.discard_changes {
            format!("{:?} (discard_changes)", input.action)
        } else {
            format!("{:?}", input.action)
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::ExitWorktree)
    }
    fn name(&self) -> &str {
        ToolName::ExitWorktree.as_str()
    }
    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::Worktree)
    }
    fn description(&self, _input: &ExitWorktreeInput, _options: &DescriptionOptions) -> String {
        "Exits a worktree session created by EnterWorktree and restores the original working directory"
            .into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        EXIT_WORKTREE_PROMPT.to_string()
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("exit a worktree session and return to the original directory")
    }

    /// Emit the prebuilt `message` field; restoration metadata is for
    /// the query-engine cleanup hook, not the model.
    fn render_for_model(&self, out: &ExitWorktreeOutput) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.message.clone(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: ExitWorktreeInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<ExitWorktreeOutput>, ToolError> {
        let state = ctx
            .app_state
            .as_ref()
            .ok_or_else(|| ToolError::InvalidInput {
                message: "No active worktree state is available in this session".into(),
                error_code: None,
            })?
            .read()
            .await
            .active_worktree
            .clone()
            .ok_or_else(|| ToolError::InvalidInput {
                message: "No active worktree to exit".into(),
                error_code: None,
            })?;

        let worktree_path = state.worktree_path.clone();
        let restore_target = state.original_cwd.clone();
        let path_display = worktree_path.display().to_string();

        // For removals, count what would be discarded (uncommitted files +
        // commits ahead of the base HEAD) BEFORE the worktree is torn down.
        // The same count drives the safety gate AND the `discardedFiles` /
        // `discardedCommits` output fields. Re-runs `countWorktreeChanges` at
        // execution time for accurate output.
        //
        // Safety gate: unless the caller passed `discard_changes`, refuse to
        // remove a worktree that has uncommitted work — `git worktree remove
        // --force` would destroy it permanently. Fail-closed: if git state
        // can't be verified, also refuse.
        let (discarded_files, discarded_commits) = if matches!(
            input.action,
            ExitWorktreeAction::Remove
        ) {
            let gate_path = worktree_path.clone();
            let base = state.original_head_commit.clone();
            let summary = tokio::task::spawn_blocking(move || {
                coco_git::count_worktree_changes(&gate_path, base.as_deref())
            })
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("worktree status check panicked: {e}"),
                display_data: None,
                source: None,
            })?;

            if !input.discard_changes {
                match &summary {
                    None => {
                        return Err(ToolError::InvalidInput {
                            message: format!(
                                "Could not verify the git state of the worktree at {path_display:?}. \
                                     Refusing to remove without explicit confirmation. Re-invoke with \
                                     discard_changes: true to proceed — or use action: \"keep\" to \
                                     preserve the worktree."
                            ),
                            error_code: None,
                        });
                    }
                    Some(summary) if summary.has_pending_work() => {
                        let files = summary.changed_files;
                        let noun = if files == 1 { "file" } else { "files" };
                        return Err(ToolError::InvalidInput {
                            message: format!(
                                "Worktree at {path_display:?} has {files} uncommitted {noun}. Removing will \
                                     discard this work permanently. Confirm with the user, then re-invoke \
                                     with discard_changes: true — or use action: \"keep\" to preserve the \
                                     worktree."
                            ),
                            error_code: None,
                        });
                    }
                    Some(_) => {} // clean worktree — safe to remove
                }
            }

            // Falls back to 0/0 when the git count fails on a force-remove.
            match summary {
                Some(s) => (Some(s.changed_files as i64), Some(s.commits_ahead as i64)),
                None => (Some(0), Some(0)),
            }
        } else {
            (None, None)
        };

        if let Err(e) = std::env::set_current_dir(&restore_target) {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "Failed to restore cwd to '{}': {e}",
                    restore_target.display()
                ),
                display_data: None,
                source: None,
            });
        }
        if let Some(session_cwd) = &ctx.session_cwd {
            *session_cwd.write().await = restore_target.clone();
        }

        if matches!(input.action, ExitWorktreeAction::Remove) {
            let mut args = vec!["worktree", "remove"];
            if input.discard_changes {
                args.push("--force");
            }
            args.push(&path_display);

            let output = tokio::process::Command::new("git")
                .current_dir(&restore_target)
                .args(&args)
                .output()
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to run git worktree remove: {e}"),
                    display_data: None,
                    source: None,
                })?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(ToolError::ExecutionFailed {
                    message: format!("git worktree remove failed: {stderr}"),
                    display_data: None,
                    source: None,
                });
            }
        }

        // Proactive LSP cleanup: shutdown the worktree-rooted servers
        // BEFORE process-cwd restoration. The `git worktree remove`
        // succeeded above, but stale `(server_id, worktree_root)` cache
        // entries would otherwise linger until session end. The lazy
        // path in `LspServerManager::get_client` (server.rs:206-229)
        // catches this when the next request happens to touch a file
        // under the removed root, but that next request may never come
        // in the session. Best-effort — adapter swallows errors.
        if matches!(input.action, ExitWorktreeAction::Remove) {
            ctx.lsp.shutdown_for_root(&worktree_path).await;
        }
        // Synthesize the structured fields into a user-facing narrative:
        // branch (keep) / discard summary (remove) + the restored session cwd.
        // (coco-rs worktrees have no tmux session, so that detail is omitted.)
        let restored_cwd = restore_target.display().to_string();
        let message = match input.action {
            ExitWorktreeAction::Keep => {
                let branch = state
                    .worktree_branch
                    .as_deref()
                    .map(|b| format!(" on branch '{b}'"))
                    .unwrap_or_default();
                format!(
                    "Exited worktree. Your work is preserved at '{path_display}'{branch}. \
                     Session is now back in '{restored_cwd}'."
                )
            }
            ExitWorktreeAction::Remove => {
                let mut discarded = Vec::new();
                if let Some(f) = discarded_files.filter(|n| *n > 0) {
                    let noun = if f == 1 { "file" } else { "files" };
                    discarded.push(format!("{f} uncommitted {noun}"));
                }
                if let Some(c) = discarded_commits.filter(|n| *n > 0) {
                    let noun = if c == 1 { "commit" } else { "commits" };
                    discarded.push(format!("{c} {noun}"));
                }
                let discard = if discarded.is_empty() {
                    String::new()
                } else {
                    format!(" Discarded {}.", discarded.join(" and "))
                };
                format!(
                    "Exited and removed worktree at '{path_display}'.{discard} \
                     Session is now back in '{restored_cwd}'."
                )
            }
        };

        Ok(ToolResult {
            data: ExitWorktreeOutput {
                action: input.action,
                original_cwd: restored_cwd,
                worktree_path: path_display,
                worktree_branch: state.worktree_branch,
                discarded_files,
                discarded_commits,
                message,
            },
            new_messages: vec![],
            app_state_patch: Some(Box::new(|app_state| {
                app_state.active_worktree = None;
            })),
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Reject a worktree name that would escape the worktrees dir via path
/// traversal. Runs before any git/chdir side effect. Each `/`-separated
/// segment must be non-empty and match `[a-zA-Z0-9._-]+`; `.`/`..` segments
/// and names over 64 chars are rejected.
fn validate_worktree_slug(slug: &str) -> Result<(), ToolError> {
    const MAX_WORKTREE_SLUG_LENGTH: usize = 64;
    let len = slug.chars().count();
    if len > MAX_WORKTREE_SLUG_LENGTH {
        return Err(ToolError::InvalidInput {
            message: format!(
                "Invalid worktree name: must be {MAX_WORKTREE_SLUG_LENGTH} characters or fewer (got {len})"
            ),
            error_code: None,
        });
    }
    for segment in slug.split('/') {
        if segment == "." || segment == ".." {
            return Err(ToolError::InvalidInput {
                message: format!(
                    "Invalid worktree name \"{slug}\": must not contain \".\" or \"..\" path segments"
                ),
                error_code: None,
            });
        }
        if segment.is_empty()
            || !segment
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(ToolError::InvalidInput {
                message: format!(
                    "Invalid worktree name \"{slug}\": each \"/\"-separated segment must be \
                     non-empty and contain only letters, digits, dots, underscores, and dashes"
                ),
                error_code: None,
            });
        }
    }
    Ok(())
}

fn worktree_slug(name: Option<&str>) -> String {
    let source = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()[..8].to_string());
    let mut out = String::new();
    let mut last_dash = false;
    for ch in source.chars() {
        let next = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if ch == '-' || ch == '_' || ch.is_whitespace() {
            Some('-')
        } else {
            None
        };
        let Some(next) = next else {
            continue;
        };
        if next == '-' {
            if !last_dash && !out.is_empty() {
                out.push(next);
                last_dash = true;
            }
        } else {
            out.push(next);
            last_dash = false;
        }
        if out.len() >= 48 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
#[path = "worktree.test.rs"]
mod tests;
