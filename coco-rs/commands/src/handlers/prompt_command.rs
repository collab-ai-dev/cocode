//! Prompt-type slash commands.
//!
//!
//! These commands don't run code locally — they push a prompt back into the
//! agent loop, which then drives subsequent tool calls.

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;
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

/// Handler for `/review [pr] [instructions...]`, matching Claude Code's
/// PR-scoped review command instead of reviewing the local worktree.
pub struct ReviewPromptHandler {
    pub name: String,
    pub progress_message: String,
    pub body: String,
}

impl ReviewPromptHandler {
    fn build_prompt(&self, args: &str) -> String {
        let mut segments = args.split_whitespace();
        let raw_pr = segments.next().unwrap_or_default();
        let pr = raw_pr.replace('`', "").trim_start_matches('#').to_string();
        let extra_instructions = segments.collect::<Vec<_>>().join(" ");

        if pr.is_empty() {
            return "Run `gh pr list` to show open pull requests, then ask the user which one to review. \
                    After they choose one, review it with `/review <number>`."
                .to_string();
        }

        let mut text = format!(
            "Review target: GitHub pull request `{pr}`.\n\n\
             Gather PR metadata with:\n\
             `gh pr view {pr} --json title,body,author,baseRefName,headRefName,state,additions,deletions,changedFiles,labels`\n\n\
             Gather the PR diff with:\n\
             `gh pr diff {pr}`\n\n\
             Review only the PR diff. Local working-tree changes are out of scope."
        );
        if !extra_instructions.trim().is_empty() {
            text.push_str("\n\nAdditional instructions from the user: ");
            text.push_str(extra_instructions.trim());
        }
        text.push_str("\n\n");
        text.push_str(self.body.trim());
        text
    }
}

#[async_trait]
impl CommandHandler for ReviewPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        Ok(CommandResult::Prompt {
            progress_message: self.progress_message.clone(),
            parts: vec![PromptPart::Text {
                text: self.build_prompt(args),
            }],
        })
    }

    fn handler_name(&self) -> &str {
        &self.name
    }
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

/// Handler for a slash command projected from a workflow definition
/// (`/deep-research`, or any script in the workflow lookup dirs).
///
/// The command **executes nothing**. It expands to text telling the model the
/// workflow's name, description, `whenToUse` and phase list, then hands it the
/// exact `Workflow(...)` call to make. That indirection is deliberate: the human
/// path (`/deep-research <question>`) and the model path converge on the same
/// tool call, which is also the one place permission rules see it.
///
/// Advertising the phases is the workflow's own cost disclosure — a reader sees
/// "3-vote adversarial verification per claim" *before* ~100 subagents run.
pub struct WorkflowLaunchPromptHandler {
    /// The workflow's `meta.name` — both the command name and the `Workflow`
    /// tool's `name` argument.
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    /// `meta.phases`, rendered as `- title: detail` lines.
    pub phases: Vec<(String, Option<String>)>,
    pub progress_message: String,
}

impl WorkflowLaunchPromptHandler {
    fn build_prompt(&self, args: &str) -> String {
        let mut text = format!(
            "Run the \"{}\" workflow.\n\n{}",
            self.name, self.description
        );
        if let Some(when_to_use) = self
            .when_to_use
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            text.push_str("\n\n");
            text.push_str(when_to_use);
        }
        if !self.phases.is_empty() {
            text.push_str("\n\nPhases:");
            for (title, detail) in &self.phases {
                match detail.as_deref().filter(|detail| !detail.is_empty()) {
                    Some(detail) => text.push_str(&format!("\n- {title}: {detail}")),
                    None => text.push_str(&format!("\n- {title}")),
                }
            }
        }
        // JSON-encode both fields: the model has to reproduce a syntactically
        // valid call, and a question containing quotes or newlines otherwise
        // would not round-trip.
        let name = json_string(&self.name);
        let args = args.trim();
        let invocation = if args.is_empty() {
            format!("{{ name: {name} }}")
        } else {
            format!("{{ name: {name}, args: {} }}", json_string(args))
        };
        text.push_str(&format!("\n\nInvoke: Workflow({invocation})"));
        text
    }
}

/// Render `value` as a JSON string literal (`serde_json` never fails on a
/// `&str`, so the fallback is unreachable rather than lossy).
fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("\"{value}\""))
}

#[async_trait]
impl CommandHandler for WorkflowLaunchPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        Ok(CommandResult::Prompt {
            progress_message: self.progress_message.clone(),
            parts: vec![PromptPart::Text {
                text: self.build_prompt(args),
            }],
        })
    }

    fn handler_name(&self) -> &str {
        &self.name
    }
}

/// Prompt handler that opens the workflow picker on bare `/workflow`, while
/// preserving prompt-command behavior when the user supplies a target/task.
pub struct WorkflowPromptHandler {
    pub inner: StaticPromptHandler,
}

#[async_trait]
impl CommandHandler for WorkflowPromptHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        if args.trim().is_empty() {
            return Ok(CommandResult::OpenDialog(DialogSpec::WorkflowPicker));
        }
        self.inner.execute_command(args).await
    }

    fn handler_name(&self) -> &str {
        self.inner.handler_name()
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
