//! Prompt-type slash commands.
//!
//!
//! These commands don't run code locally — they push a prompt back into the
//! agent loop, which then drives subsequent tool calls.

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::PromptPart;

/// How a Prompt-type command should incorporate the user-supplied
/// `args` into the static body.
///
/// Replaces the prior `bool append_task` flag — CLAUDE.md style guide
/// flags `bool` parameters when callsites would read as opaque
/// literals (`register_static_prompt(..., true)`). The enum makes the
/// behaviour explicit at every callsite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgsHandling {
    /// No args manipulation — body is emitted verbatim regardless of
    /// `args`. Used by static prompts that never expect args
    /// (`/statusline`).
    Static,
    /// Append `\n\n## Task\n\n<args>` when args are non-empty.
    /// Used by `/security-review`, `/insights`, `/pr-comments`.
    AppendUnderTask,
    /// Always emit `\n<prefix><args>` at the body's end. `args` may be
    /// empty — as in `/review`: ``PR number: ${args}`` is
    /// included even when no PR number was given, so the model sees
    /// an explicit empty value rather than the line being absent.
    AppendInline { prefix: &'static str },
}

/// Handler that returns a static prompt text wrapped in
/// `CommandResult::Prompt`. The supplied [`ArgsHandling`] decides how
/// `args` are folded into the body.
pub struct StaticPromptHandler {
    pub name: String,
    pub progress_message: String,
    pub body: String,
    pub args_handling: ArgsHandling,
}

impl StaticPromptHandler {
    pub fn new(
        name: impl Into<String>,
        progress_message: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            progress_message: progress_message.into(),
            body: body.into(),
            args_handling: ArgsHandling::Static,
        }
    }

    pub fn with_task_append(
        name: impl Into<String>,
        progress_message: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            progress_message: progress_message.into(),
            body: body.into(),
            args_handling: ArgsHandling::AppendUnderTask,
        }
    }

    pub fn with_inline_append(
        name: impl Into<String>,
        progress_message: impl Into<String>,
        body: impl Into<String>,
        prefix: &'static str,
    ) -> Self {
        Self {
            name: name.into(),
            progress_message: progress_message.into(),
            body: body.into(),
            args_handling: ArgsHandling::AppendInline { prefix },
        }
    }
}

#[async_trait]
impl CommandHandler for StaticPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let mut text = self.body.clone();
        match self.args_handling {
            ArgsHandling::Static => {}
            ArgsHandling::AppendUnderTask => {
                if !args.trim().is_empty() {
                    text.push_str("\n\n## Task\n\n");
                    text.push_str(args);
                }
            }
            ArgsHandling::AppendInline { prefix } => {
                // Emit the prefix line unconditionally — even when args is
                // empty — so the model gets an explicit blank value rather
                // than an absent line.
                text.push('\n');
                text.push_str(prefix);
                text.push_str(args);
            }
        }
        Ok(CommandResult::Prompt {
            progress_message: self.progress_message.clone(),
            parts: vec![PromptPart::Text { text }],
        })
    }

    fn handler_name(&self) -> &str {
        &self.name
    }
}

/// Handler that pre-resolves `` !`<shell-cmd>` `` (and block `` ```! ``)
/// markers in the prompt body before sending to the model.
///
/// Each command is routed through the injected [`BashToolHandle`], which
/// performs the real per-command permission check + Bash execution. A
/// denied or failing command ABORTS the whole expansion. `allowed_tools`
/// is empty for slash commands
/// — only configured permission rules apply (unlike skills, which inject
/// their frontmatter `allowed-tools`).
///
/// When no handle is wired (tests / pre-bootstrap) the body is emitted
/// verbatim — no unguarded `bash -c` runs from a slash command.
///
/// Used by `/security-review` and any other Prompt command that expands
/// shell substitutions before pushing to the agent.
pub struct ShellExpandingPromptHandler {
    pub name: String,
    pub progress_message: String,
    pub body: String,
    /// How `args` are folded into the body. See [`ArgsHandling`].
    pub args_handling: ArgsHandling,
    /// Shared, late-bound Bash handle (cloned from the registry cell).
    pub bash_tool_handle: crate::SharedBashToolHandle,
}

impl ShellExpandingPromptHandler {
    pub fn new(
        name: impl Into<String>,
        progress_message: impl Into<String>,
        body: impl Into<String>,
        bash_tool_handle: crate::SharedBashToolHandle,
    ) -> Self {
        Self {
            name: name.into(),
            progress_message: progress_message.into(),
            body: body.into(),
            args_handling: ArgsHandling::Static,
            bash_tool_handle,
        }
    }
}

#[async_trait]
impl CommandHandler for ShellExpandingPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        // Slash commands carry no frontmatter `allowed-tools` — only
        // configured permission rules apply (empty slice).
        let mut text = match crate::snapshot_bash_handle(&self.bash_tool_handle) {
            Some(handle) => coco_skills::shell_exec::execute_shell_in_prompt_with_tool(
                &self.body,
                &*handle,
                &[],
            )
            .await
            .map_err(|message| crate::CommandsError::ShellCommandError { message })?,
            None => self.body.clone(),
        };
        match self.args_handling {
            ArgsHandling::Static => {}
            ArgsHandling::AppendUnderTask => {
                if !args.trim().is_empty() {
                    text.push_str("\n\n## Task\n\n");
                    text.push_str(args);
                }
            }
            ArgsHandling::AppendInline { prefix } => {
                text.push('\n');
                text.push_str(prefix);
                text.push_str(args);
            }
        }
        Ok(CommandResult::Prompt {
            progress_message: self.progress_message.clone(),
            parts: vec![PromptPart::Text { text }],
        })
    }

    fn handler_name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
#[path = "prompt_command.test.rs"]
mod tests;
