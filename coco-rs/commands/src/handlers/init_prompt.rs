//! `/init` command — return the codebase-init prompt body.
//!
//! Two prompt bodies gated by feature flag:
//! - Old: single instruction to generate CLAUDE.md.
//! - New: 8-phase guided setup (Q&A, codebase survey, CLAUDE.md, skills, hooks).

use async_trait::async_trait;
use coco_types::Feature;
use coco_types::Features;
use coco_types::UserType;
use std::path::PathBuf;

use crate::CommandHandler;
use crate::CommandResult;
use crate::PromptPart;

const NEW_INIT_PROMPT: &str = include_str!("../prompts/init_new.txt");
const OLD_INIT_PROMPT: &str = include_str!("../prompts/init_old.txt");

pub struct InitPromptHandler {
    pub user_type: UserType,
    pub features: Features,
    /// Project root, used by `maybe_mark_project_onboarding_complete` to
    /// flip the onboarding-completed flag in `~/.coco.json` when the
    /// project already has a `CLAUDE.md`. `None` falls back to the
    /// process cwd at invocation time.
    pub project_root: Option<PathBuf>,
}

impl InitPromptHandler {
    pub fn select_prompt(&self) -> &'static str {
        let new_init_env = std::env::var("COCO_NEW_INIT")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        if self.features.enabled(Feature::NewInit) && (self.user_type.is_ant() || new_init_env) {
            NEW_INIT_PROMPT
        } else {
            OLD_INIT_PROMPT
        }
    }
}

#[async_trait]
impl CommandHandler for InitPromptHandler {
    async fn execute_command(&self, _args: &str) -> crate::Result<CommandResult> {
        // The flag short-circuits future invocations; in coco-rs
        // the call is opportunistic — failures are swallowed by the
        // helper itself.
        let cwd = self
            .project_root
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        coco_config::global_config::maybe_mark_project_onboarding_complete(&cwd);

        let body = self.select_prompt();
        Ok(CommandResult::Prompt {
            progress_message: "analyzing your codebase".into(),
            parts: vec![PromptPart::Text {
                text: body.to_string(),
            }],
        })
    }

    fn handler_name(&self) -> &str {
        "init"
    }
}

#[cfg(test)]
#[path = "init_prompt.test.rs"]
mod tests;
