//! `/commit-push-pr` — orchestrated commit + push + PR prompt.
//!
//! Builds a prompt that interpolates several shell substitutions
//! (`git status`, `git diff HEAD`, `git branch --show-current`,
//! `git diff <default>...HEAD`, `gh pr view --json number`) and resolves
//! the repository's default branch. The agent then runs the orchestrated
//! commit → push → PR flow with `ALLOWED_TOOLS` pre-granted.
//!
//! This handler mirrors that shape: detect the default branch, run each
//! of the shell substitutions inline, render the prompt template, append
//! optional user guidance, and emit a `CommandResult::Prompt`. Tests can
//! pin the cwd via [`CommitPushPrHandler::with_cwd`] to stay isolated
//! from the rest of the workspace.

use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::PromptPart;

const PROMPT_TEMPLATE: &str = include_str!("../prompts/commit_push_pr.txt");

pub struct CommitPushPrHandler {
    /// Override cwd for tests; production uses `std::env::current_dir`.
    cwd: Option<PathBuf>,
}

impl CommitPushPrHandler {
    pub const fn new() -> Self {
        Self { cwd: None }
    }

    #[cfg(test)]
    pub fn with_cwd(path: PathBuf) -> Self {
        Self { cwd: Some(path) }
    }

    fn resolved_cwd(&self) -> PathBuf {
        self.cwd
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

impl Default for CommitPushPrHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for CommitPushPrHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let cwd = self.resolved_cwd();
        if run_git(&cwd, &["rev-parse", "--is-inside-work-tree"])
            .await
            .is_err()
        {
            return Ok(CommandResult::Text(
                "Not a git repository. Run /commit-push-pr from inside a git project.".to_string(),
            ));
        }

        let default_branch = detect_default_branch(&cwd).await;
        let whoami = current_user();

        let status = run_git(&cwd, &["status"]).await.unwrap_or_default();
        let diff_head = run_git(&cwd, &["diff", "HEAD"]).await.unwrap_or_default();
        let branch = run_git(&cwd, &["branch", "--show-current"])
            .await
            .unwrap_or_default();
        let diff_vs_default = run_git(&cwd, &["diff", &format!("{default_branch}...HEAD")])
            .await
            .unwrap_or_default();
        let pr_view = run_gh_pr_view(&cwd).await;

        let mut body = PROMPT_TEMPLATE
            .replace("{{WHOAMI}}", whoami.trim())
            .replace("{{DEFAULT_BRANCH}}", default_branch.trim())
            .replace("!`git status`", status.trim())
            .replace("!`git diff HEAD`", diff_head.trim())
            .replace("!`git branch --show-current`", branch.trim())
            .replace(
                &format!("!`git diff {}...HEAD`", default_branch.trim()),
                diff_vs_default.trim(),
            )
            .replace(
                "!`gh pr view --json number 2>/dev/null || true`",
                pr_view.trim(),
            );

        let extra = args.trim();
        if !extra.is_empty() {
            body.push_str("\n\n## Additional instructions from user\n\n");
            body.push_str(extra);
        }

        Ok(CommandResult::Prompt {
            progress_message: "creating commit and PR".to_string(),
            parts: vec![PromptPart::Text { text: body }],
        })
    }

    fn handler_name(&self) -> &str {
        "commit-push-pr"
    }
}

/// Detect the repository's default branch:
/// 1. Prefer `git symbolic-ref refs/remotes/origin/HEAD` (resolves to
///    `refs/remotes/origin/<branch>`); take the trailing segment.
/// 2. Fall back to `git config --get init.defaultBranch`.
/// 3. Final fallback: `main`.
async fn detect_default_branch(cwd: &Path) -> String {
    if let Ok(out) = run_git(cwd, &["symbolic-ref", "refs/remotes/origin/HEAD"]).await
        && let Some(branch) = out.trim().rsplit('/').next()
        && !branch.is_empty()
    {
        return branch.to_string();
    }
    if let Ok(out) = run_git(cwd, &["config", "--get", "init.defaultBranch"]).await {
        let trimmed = out.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    "main".to_string()
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

async fn run_git(cwd: &Path, args: &[&str]) -> crate::Result<String> {
    let output = tokio::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .await?;
    if !output.status.success() {
        return Err(crate::CommandsError::git_failed(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `gh pr view` returns non-zero when no PR exists for the branch — that's
/// the *normal* path before the first push. Treat any non-zero status as
/// "no PR yet" and emit empty output (equivalent to `2>/dev/null || true`).
async fn run_gh_pr_view(cwd: &Path) -> String {
    match tokio::process::Command::new("gh")
        .current_dir(cwd)
        .args(["pr", "view", "--json", "number"])
        .output()
        .await
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "commit_push_pr.test.rs"]
mod tests;
