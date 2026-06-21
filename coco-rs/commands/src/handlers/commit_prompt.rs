//! `/commit` — prompt that bundles git context for the agent.
//!
//! Builds a prompt that interpolates `!\`git status\`` / `!\`git diff HEAD\`` /
//! `!\`git log --oneline -10\`` / `!\`git branch --show-current\`` so the
//! resulting `CommandResult::Prompt` carries concrete diff/status text.
//!
//! Args after `/commit` are appended as additional guidance for the
//! commit message.

use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::PromptPart;

const PROMPT_TEMPLATE: &str = include_str!("../prompts/commit.txt");

pub struct CommitPromptHandler {
    /// Override cwd for tests; production uses `std::env::current_dir`.
    cwd: Option<PathBuf>,
}

impl CommitPromptHandler {
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

impl Default for CommitPromptHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CommandHandler for CommitPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let cwd = self.resolved_cwd();
        let in_repo = run_git(&cwd, &["rev-parse", "--is-inside-work-tree"]).await;
        if in_repo.is_err() {
            return Ok(CommandResult::Text(
                "Not a git repository. Run /commit from inside a git project.".to_string(),
            ));
        }

        let status = run_git(&cwd, &["status"]).await.unwrap_or_default();
        let diff = run_git(&cwd, &["diff", "HEAD"]).await.unwrap_or_default();
        let branch = run_git(&cwd, &["branch", "--show-current"])
            .await
            .unwrap_or_default();
        let log = run_git(&cwd, &["log", "--oneline", "-10"])
            .await
            .unwrap_or_default();

        // Inline-substitute the !`...` markers. Keep the rest of the
        // prompt verbatim so commits/instructions stay aligned.
        let mut body = PROMPT_TEMPLATE
            .replace("!`git status`", status.trim())
            .replace("!`git diff HEAD`", diff.trim())
            .replace("!`git branch --show-current`", branch.trim())
            .replace("!`git log --oneline -10`", log.trim());

        let extra = args.trim();
        if !extra.is_empty() {
            body.push_str("\n\n## Additional guidance\n\n");
            body.push_str(extra);
        }

        Ok(CommandResult::Prompt {
            progress_message: "preparing commit".to_string(),
            parts: vec![PromptPart::Text { text: body }],
        })
    }

    fn handler_name(&self) -> &str {
        "commit"
    }
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

#[cfg(test)]
#[path = "commit_prompt.test.rs"]
mod tests;
