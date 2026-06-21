use super::background_task::BackgroundKind;
use super::background_task::background_output_path;
use super::background_task::format_background_notice;
use super::shell_render::strip_leading_blank_lines;
use coco_messages::ToolResult;
use coco_permissions::InternalPathContext;
use coco_permissions::has_shell_expansion;
use coco_permissions::is_editable_internal_path;
use coco_permissions::is_path_within_allowed_dirs;
use coco_sandbox::SandboxBypass;
use coco_sandbox::SandboxState;
use coco_shell::read_only::is_read_only_command;
use coco_shell::security::SecuritySeverity;
use coco_shell::security::check_security;
use coco_tool_runtime::BackgroundShellRequest;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolProgress;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// Typed input for [`BashTool`].
///
/// The model-facing schema is built by the manual [`BashTool::input_schema`]
/// override — four user-visible fields, intentionally omitting
/// `_simulatedSedEdit` (internal; populated by the
/// `SedEditPermissionRequest` TUI dialog).
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct BashInput {
    /// The shell command to execute. Required — the runtime validation
    /// schema declares `"required": ["command"]`, so a missing command is
    /// rejected before deserialize.
    pub command: String,
    /// Optional timeout (ms). Defaults to `ToolConfig::bash.default_timeout_ms`
    /// when absent or zero; the model's value is otherwise honored.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Short description of what the command does. Falls back to the
    /// command string when omitted.
    #[serde(default)]
    pub description: Option<String>,
    /// Run in the background. Returns a `task_id` immediately and emits
    /// a `<task-notification>` on completion.
    #[serde(default)]
    pub run_in_background: bool,
    /// Bypass sandbox wrapping for this command. Requires the killswitch
    /// to be unlocked; falls open at the sandbox layer otherwise.
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    pub dangerously_disable_sandbox: bool,
    /// (Internal) Previewed sed-edit payload from the
    /// `SedEditPermissionRequest` TUI dialog. NOT exposed in
    /// `input_schema()` — the model can't synthesise this; it's only
    /// populated by upstream rewrite when the user converts a
    /// `sed -i ...` command into an Edit-style write.
    #[serde(default, rename = "_simulatedSedEdit")]
    pub simulated_sed_edit: Option<SimulatedSedEdit>,
}

/// Internal sed-edit payload — see [`BashInput::simulated_sed_edit`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct SimulatedSedEdit {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "newContent")]
    pub new_content: String,
}

fn default_timeout_ms(config: &coco_config::ToolConfig) -> u64 {
    config.bash.default_timeout_ms.max(1) as u64
}

/// Long-form tool description shown to the model.
///
/// Conditional sections based on runtime config (sandbox config dump,
/// undercover guidance, per-user git skill references,
/// embedded-search-tool variants) are intentionally omitted because:
///
///   1. They depend on runtime feature flags coco-rs doesn't currently
///      model (isUndercover, hasEmbeddedSearchTools).
///   2. The sandbox config dump leaks /private/tmp paths into the
///      prompt cache key — coco-rs doesn't have a sandbox manager that
///      emits config to the prompt.
///   3. The git commit/PR section is ~80 lines of skill-specific
///      guidance that's only relevant when /commit, /commit-push-pr
///      skills are loaded — coco-rs has its own skill discovery
///      pipeline.
///
/// Included: the avoid-native-commands list, tool-preference items,
/// multi-command parallelism guidance, git safety bullets,
/// timeout/run_in_background notes, sleep-avoidance guidance, and the
/// commit safety/PR creation instructions.
const BASH_TOOL_DESCRIPTION: &str = "Executes a given bash command and returns its output.

The working directory persists between commands, but shell state does not. The shell environment is initialized from the user's profile (bash or zsh).

IMPORTANT: Avoid using this tool to run `find`, `grep`, `cat`, `head`, `tail`, `sed`, `awk`, or `echo` commands, unless explicitly instructed or after you have verified that a dedicated tool cannot accomplish your task. Instead, use the appropriate dedicated tool as this will provide a much better experience for the user:

 - File search: Use Glob (NOT find or ls)
 - Content search: Use Grep (NOT grep or rg)
 - Read files: Use Read (NOT cat/head/tail)
 - Edit files: Use Edit (NOT sed/awk)
 - Write files: Use Write (NOT echo >/cat <<EOF)
 - Communication: Output text directly (NOT echo/printf)
While the Bash tool can do similar things, it's better to use the built-in tools as they provide a better user experience and make it easier to review tool calls and give permission.

# Instructions
 - If your command will create new directories or files, first use this tool to run `ls` to verify the parent directory exists and is the correct location.
 - Always quote file paths that contain spaces with double quotes in your command (e.g., cd \"path with spaces/file.txt\")
 - Try to maintain your current working directory throughout the session by using absolute paths and avoiding usage of `cd`. You may use `cd` if the User explicitly requests it.
 - You may specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). By default, your command will timeout after 120000ms (2 minutes).
 - You can use the `run_in_background` parameter to run the command in the background. Only use this if you don't need the result immediately and are OK being notified when the command completes later. You do not need to check the output right away - you'll be notified when it finishes. You do not need to use '&' at the end of the command when using this parameter.
 - When issuing multiple commands:
  - If the commands are independent and can run in parallel, make multiple Bash tool calls in a single message. Example: if you need to run \"git status\" and \"git diff\", send a single message with two Bash tool calls in parallel.
  - If the commands depend on each other and must run sequentially, use a single Bash call with '&&' to chain them together.
  - Use ';' only when you need to run commands sequentially but don't care if earlier commands fail.
  - DO NOT use newlines to separate commands (newlines are ok in quoted strings).
 - For git commands:
  - Prefer to create a new commit rather than amending an existing commit.
  - Before running destructive operations (e.g., git reset --hard, git push --force, git checkout --), consider whether there is a safer alternative that achieves the same goal. Only use destructive operations when they are truly the best approach.
  - Never skip hooks (--no-verify) or bypass signing (--no-gpg-sign, -c commit.gpgsign=false) unless the user has explicitly asked for it. If a hook fails, investigate and fix the underlying issue.
 - Avoid unnecessary `sleep` commands:
  - Do not sleep between commands that can run immediately — just run them.
  - If your command is long running and you would like to be notified when it finishes — use `run_in_background`. No sleep needed.
  - Do not retry failing commands in a sleep loop — diagnose the root cause.
  - If waiting for a background task you started with `run_in_background`, you will be notified when it completes — do not poll.
  - If you must poll an external process, use a check command (e.g. `gh run view`) rather than sleeping first.
  - If you must sleep, keep the duration short (1-5 seconds) to avoid blocking the user.

# Committing changes with git

Only create commits when requested by the user. If unclear, ask first. When the user asks you to create a new git commit, follow these steps carefully:

You can call multiple tools in a single response. When multiple independent pieces of information are requested and all commands are likely to succeed, run multiple tool calls in parallel for optimal performance. The numbered steps below indicate which commands should be batched in parallel.

Git Safety Protocol:
- NEVER update the git config
- NEVER run destructive git commands (push --force, reset --hard, checkout ., restore ., clean -f, branch -D) unless the user explicitly requests these actions. Taking unauthorized destructive actions is unhelpful and can result in lost work, so it's best to ONLY run these commands when given direct instructions
- NEVER skip hooks (--no-verify, --no-gpg-sign, etc) unless the user explicitly requests it
- NEVER run force push to main/master, warn the user if they request it
- CRITICAL: Always create NEW commits rather than amending, unless the user explicitly requests a git amend. When a pre-commit hook fails, the commit did NOT happen — so --amend would modify the PREVIOUS commit, which may result in destroying work or losing previous changes. Instead, after hook failure, fix the issue, re-stage, and create a NEW commit
- When staging files, prefer adding specific files by name rather than using \"git add -A\" or \"git add .\", which can accidentally include sensitive files (.env, credentials) or large binaries
- NEVER commit changes unless the user explicitly asks you to. It is VERY IMPORTANT to only commit when explicitly asked, otherwise the user will feel that you are being too proactive

1. Run the following bash commands in parallel, each using the Bash tool:
  - Run a git status command to see all untracked files. IMPORTANT: Never use the -uall flag as it can cause memory issues on large repos.
  - Run a git diff command to see both staged and unstaged changes that will be committed.
  - Run a git log command to see recent commit messages, so that you can follow this repository's commit message style.
2. Analyze all staged changes (both previously staged and newly added) and draft a commit message:
  - Summarize the nature of the changes (eg. new feature, enhancement to an existing feature, bug fix, refactoring, test, docs, etc.). Ensure the message accurately reflects the changes and their purpose (i.e. \"add\" means a wholly new feature, \"update\" means an enhancement to an existing feature, \"fix\" means a bug fix, etc.).
  - Do not commit files that likely contain secrets (.env, credentials.json, etc). Warn the user if they specifically request to commit those files
  - Draft a concise (1-2 sentences) commit message that focuses on the \"why\" rather than the \"what\"
  - Ensure it accurately reflects the changes and their purpose
3. Run the following commands in parallel:
   - Add relevant untracked files to the staging area.
   - Create the commit.
   - Run git status after the commit completes to verify success.
   Note: git status depends on the commit completing, so run it sequentially after the commit.
4. If the commit fails due to pre-commit hook: fix the issue and create a NEW commit

Important notes:
- NEVER run additional commands to read or explore code, besides git bash commands
- DO NOT push to the remote repository unless the user explicitly asks you to do so
- IMPORTANT: Never use git commands with the -i flag (like git rebase -i or git add -i) since they require interactive input which is not supported.
- IMPORTANT: Do not use --no-edit with git rebase commands, as the --no-edit flag is not a valid option for git rebase.
- If there are no changes to commit (i.e., no untracked files and no modifications), do not create an empty commit

# Creating pull requests
Use the gh command via the Bash tool for ALL GitHub-related tasks including working with issues, pull requests, checks, and releases. If given a Github URL use the gh command to get the information needed.

IMPORTANT: When the user asks you to create a pull request, follow these steps carefully:

1. Run the following bash commands in parallel using the Bash tool, in order to understand the current state of the branch since it diverged from the main branch:
   - Run a git status command to see all untracked files (never use -uall flag)
   - Run a git diff command to see both staged and unstaged changes that will be committed
   - Check if the current branch tracks a remote branch and is up to date with the remote, so you know if you need to push to the remote
   - Run a git log command and `git diff [base-branch]...HEAD` to understand the full commit history for the current branch (from the time it diverged from the base branch)
2. Analyze all changes that will be included in the pull request, making sure to look at all relevant commits (NOT just the latest commit, but ALL commits that will be included in the pull request!!!), and draft a pull request title and summary:
   - Keep the PR title short (under 70 characters)
   - Use the description/body for details, not the title
3. Run the following commands in parallel:
   - Create new branch if needed
   - Push to remote with -u flag if needed
   - Create PR using gh pr create

Important:
- DO NOT use the TodoWrite or Agent tools
- Return the PR URL when you're done, so the user can see it

# Other common operations
- View comments on a Github PR: gh api repos/foo/bar/pulls/123/comments";

/// Resolve the active [`SandboxState`] for a tool invocation.
///
/// Returns the state when (a) the `Sandbox` feature is enabled and
/// (b) the bootstrap layer installed an `Arc<SandboxState>` on the
/// context. Otherwise returns `None`, leaving the executor to spawn
/// commands without sandbox wrapping. Enable check → bypass + exclusion
/// are evaluated downstream by `SandboxState::command_snapshot`.
fn active_sandbox_state(ctx: &ToolUseContext) -> Option<std::sync::Arc<SandboxState>> {
    if !ctx.features.enabled(coco_types::Feature::Sandbox) {
        return None;
    }
    ctx.sandbox_state.clone()
}

/// Working directory for Bash force-ask path gates.
///
/// Mirrors the executor spawn cwd so permission analysis and execution observe
/// the same current directory.
async fn bash_gate_cwd(ctx: &ToolUseContext) -> String {
    ctx.effective_shell_cwd()
        .await
        .to_string_lossy()
        .into_owned()
}

/// Effective max Bash output byte budget.
///
/// Clamp to `[0, BASH_MAX_OUTPUT_BYTES_UPPER]` is enforced by
/// `BashConfig::finalize()` at config-resolution time — this helper
/// just normalizes the (already non-negative) value to `usize`.
fn max_output_bytes(config: &coco_config::ToolConfig) -> usize {
    config.bash.max_output_bytes.max(0) as usize
}

/// Suggestion for an out-of-tree write/redirect Ask: add the target's parent
/// directory to the allowed working dirs. This is the ONLY update that unblocks
/// the path gate — it runs inside the tool at evaluator step-1 and returns
/// before any allow/Edit rule, so a `Bash(cmd:*)` or `Edit(dir/**)` rule can't
/// help; only widening `additional_dirs` lets the same command pass next time.
/// Mirrors TS `pathValidation.ts` `getDirectoryForPath(blockedPath)` →
/// `addDirectories`. Target is resolved exactly like the gate's own
/// `is_path_within_allowed_dirs` (tilde + relative + lexical-normalize).
fn out_of_dir_suggestion(target: &str, cwd: &str) -> Vec<coco_types::PermissionUpdate> {
    let Some(resolved) = coco_permissions::filesystem::get_paths_for_permission_check(target, cwd)
        .into_iter()
        .next()
    else {
        return Vec::new();
    };
    let Some(dir) = std::path::Path::new(&resolved)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
    else {
        return Vec::new();
    };
    vec![coco_types::PermissionUpdate::AddDirectories {
        directories: vec![dir],
        destination: coco_types::PermissionUpdateDestination::Session,
    }]
}

/// Bash tool -- executes shell commands via bash -c.
/// Captures stdout, stderr, and exit code.
///
/// Supports `run_in_background: true` to spawn the command as a background
/// task. The task ID is returned immediately and the model is notified
/// asynchronously when the command completes via task-notification XML.
pub struct BashTool;

#[async_trait::async_trait]
impl Tool for BashTool {
    type Input = BashInput;
    /// Multiple wire shapes (fg text, fg image with `structuredContent`,
    /// bg `{task_id, status}`) make a typed Output more friction than
    /// it's worth — keep `Value` as the deliberate escape hatch (same
    /// pattern as `ReadTool` / `PowerShellTool`).
    type Output = serde_json::Value;

    fn to_auto_classifier_input(&self, input: &BashInput) -> Option<String> {
        Some(input.command.clone())
    }

    // Static schema from a literal `json!`; a parse failure means the literal
    // is malformed (programmer error), so panicking on first build is correct.
    #[allow(clippy::expect_used)]
    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        static SCHEMA: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
            std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            // Runtime schema additionally declares `_simulatedSedEdit` — the TUI
            // sed-edit dialog injects it before re-validation. The model schema
            // omits it (see `tool_spec`).
            coco_tool_runtime::ToolInputSchema::from_static_value(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The command to execute"
                    },
                    "timeout": {
                        "type": "number",
                        "description": "Optional timeout in milliseconds (max 600000)"
                    },
                    "description": {
                        "type": "string",
                        "description": "Clear, concise description of what this command does in active voice. Never use words like \"complex\" or \"risk\" in the description - just describe what it does.\n\nFor simple commands (git, npm, standard CLI tools), keep it brief (5-10 words):\n- ls → \"List files in current directory\"\n- git status → \"Show working tree status\"\n- npm install → \"Install package dependencies\"\n\nFor commands that are harder to parse at a glance (piped commands, obscure flags, etc.), add enough context to clarify what it does:\n- find . -name \"*.tmp\" -exec rm {} \\; → \"Find and delete all .tmp files recursively\"\n- git reset --hard origin/main → \"Discard all local changes and match remote main\"\n- curl -s url | jq '.data[]' → \"Fetch JSON from URL and extract data array elements\""
                    },
                    "run_in_background": {
                        "type": "boolean",
                        "description": "Set to true to run this command in the background. Use Read to read the output later."
                    },
                    "dangerouslyDisableSandbox": {
                        "type": "boolean",
                        "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."
                    },
                    "_simulatedSedEdit": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "filePath": { "type": "string" },
                            "newContent": { "type": "string" }
                        },
                        "required": ["filePath", "newContent"],
                        "description": "(internal) TUI-injected sed-edit payload"
                    }
                },
                "required": ["command"]
            }))
        })
    }

    /// Model-facing spec. Always strips the internal `_simulatedSedEdit`
    /// field. When background tasks are disabled
    /// (`COCO_BACKGROUND_TASKS_DISABLE`), also drops `run_in_background`.
    async fn tool_spec(
        &self,
        ctx: &coco_tool_runtime::SchemaContext,
        prompt_opts: &coco_tool_runtime::PromptOptions,
    ) -> coco_tool_runtime::ToolSpec {
        let omit: &[&str] = if ctx.background_tasks_disabled {
            &["_simulatedSedEdit", "run_in_background"]
        } else {
            &["_simulatedSedEdit"]
        };
        coco_tool_runtime::ToolSpec::Function(coco_tool_runtime::FunctionToolSpec {
            name: self.name().to_string(),
            description: self.prompt(prompt_opts).await,
            parameters: coco_tool_runtime::schema_omit_properties(
                self.runtime_validation_schema().as_value(),
                omit,
            ),
            strict: self.strict(),
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Bash)
    }

    fn name(&self) -> &str {
        ToolName::Bash.as_str()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.active_shell_tool == coco_types::ActiveShellTool::Bash
    }

    /// Short per-call UI label: `description || 'Run shell command'`.
    fn search_hint(&self) -> Option<&str> {
        Some("execute shell commands")
    }
    fn description(&self, input: &BashInput, _options: &DescriptionOptions) -> String {
        input
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "Run shell command".to_string())
    }

    /// Model-facing tool description (schema-listing time). Text held in
    /// [`BASH_TOOL_DESCRIPTION`].
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        BASH_TOOL_DESCRIPTION.into()
    }

    /// Read-only fast path. Commands on the allowlist (`cat`, `ls`, `grep`,
    /// `git log`, `docker ps`, etc.) get auto-approved upstream and batched as
    /// concurrency-safe, avoiding the permission UI for routine inspection.
    ///
    /// Delegates to `coco_shell::read_only::is_read_only_command` which wraps the
    /// 40+ safe-command allowlist + conditional safety rules for git/sed/find/rg/etc.
    fn is_read_only(&self, input: &BashInput) -> bool {
        if input.command.is_empty() {
            return false;
        }
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "/".to_string());
        if coco_shell::check_multiple_cwd_changes(&input.command, &cwd).is_some() {
            return false;
        }
        // Git sandbox-escape commands (cd+git, git-internal writes) are NOT
        // read-only — they reach the permission prompt instead of auto-allowing.
        if coco_shell::has_git_escape_pattern_in_cwd(&input.command, &cwd) {
            return false;
        }
        is_read_only_command(&input.command)
    }

    /// Concurrency-safe iff read-only, matching TS Bash. Foreground Bash still
    /// writes a cwd checkpoint after completion; concurrent read-only cwd
    /// changes use final-completer-wins session cwd updates, as in TS.
    fn is_concurrency_safe(&self, input: &BashInput) -> bool {
        Tool::is_read_only(self, input)
    }

    fn get_activity_description(&self, input: &BashInput) -> Option<String> {
        if input.command.is_empty() {
            return None;
        }
        let command = input.command.as_str();
        let truncated: String = command.chars().take(57).collect();
        let display = if truncated.len() < command.len() {
            format!("Running {truncated}...")
        } else {
            format!("Running {command}")
        };
        Some(display)
    }

    /// Route shell security analysis through the permission pipeline instead of
    /// only acting on `Deny` at execute time. `SecuritySeverity::Ask` results
    /// (eval / IFS / jq-danger / dangerous-vars / …, demoted to Ask in
    /// shell#163) now reach the user as a prompt; `Deny` blocks pre-execution.
    /// In acceptEdits mode any filesystem subcommand (mkdir/rm/mv/…) auto-allows.
    /// Read-only commands defer to the rule pipeline (no false positives on
    /// quoted metacharacters).
    async fn check_permissions(
        &self,
        input: &BashInput,
        ctx: &ToolUseContext,
    ) -> coco_types::ToolCheckResult {
        let command = input.command.as_str();
        if command.is_empty() {
            return coco_types::ToolCheckResult::Passthrough;
        }

        // Force-ask gates: run BEFORE the acceptEdits auto-allow and the
        // allow-rule pipeline so a dangerous removal, a code-executing/file-writing
        // `sed`, or a git sandbox-escape cannot be auto-allowed. A returned `Ask`
        // short-circuits at the evaluator's step-1b — no allow rule or mode
        // overrides it (only `DontAsk` converts it to deny).
        let cwd = bash_gate_cwd(ctx).await;
        let command_analysis = coco_shell::analyze_compound_command(command, &cwd);
        if let Some(reason) = coco_shell::check_dangerous_removal(command, &cwd) {
            return coco_types::ToolCheckResult::Ask {
                message: reason,
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        if let Some(reason) = coco_shell::check_multiple_cwd_changes(command, &cwd) {
            return coco_types::ToolCheckResult::Ask {
                message: reason,
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        let allow_file_writes =
            ctx.permission_context.mode == coco_types::PermissionMode::AcceptEdits;
        if coco_shell::has_dangerous_sed(command, allow_file_writes) {
            return coco_types::ToolCheckResult::Ask {
                message: "This `sed` command performs a shell-execute or file-write \
                          operation and requires approval."
                    .into(),
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        if let Some(reason) = coco_shell::check_git_escape(command, &cwd) {
            return coco_types::ToolCheckResult::Ask {
                message: reason,
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        // Path-constraint gate: an output redirection or process substitution that
        // writes OUTSIDE the allowed working dirs — or via a shell-expanded /
        // unresolvable target — forces Ask and cannot be auto-allowed.
        // `> /etc/passwd`, `echo x > $TARGET`, `… > >(tee .git/config)`.
        if coco_shell::has_process_substitution(command) {
            return coco_types::ToolCheckResult::Ask {
                message: "Process substitution (`>(...)` or `<(...)`) can execute arbitrary \
                          commands and requires manual approval."
                    .into(),
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        let additional_dirs: Vec<String> = ctx
            .permission_context
            .additional_dirs
            .keys()
            .cloned()
            .collect();
        let redirect_targets = coco_shell::extract_output_redirect_targets(command);
        if command_analysis.has_non_noop_cwd_change() && !redirect_targets.is_empty() {
            return coco_types::ToolCheckResult::Ask {
                message: "Output redirection after changing directories requires approval.".into(),
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        for target in redirect_targets {
            // /dev/null is always safe — it discards output.
            if target == "/dev/null" {
                continue;
            }
            if has_shell_expansion(&target) {
                return coco_types::ToolCheckResult::Ask {
                    message: "Shell expansion syntax in a redirection target requires \
                              manual approval."
                        .into(),
                    suggestions: Vec::new(),
                    choices: None,
                    detail: None,
                };
            }
            if !is_path_within_allowed_dirs(&target, &cwd, &additional_dirs) {
                return coco_types::ToolCheckResult::Ask {
                    message: format!(
                        "Output redirection to '{target}' is outside the allowed working \
                         directories and requires manual approval."
                    ),
                    suggestions: out_of_dir_suggestion(&target, &cwd),
                    choices: None,
                    detail: None,
                };
            }
        }
        // Per-subcommand write-path gate: a filesystem write (rm/rmdir/mv/cp/touch/mkdir)
        // targeting a path OUTSIDE the allowed working dirs forces Ask. Extends
        // the dangerous-removal gate from the catastrophic-system-path list to
        // any out-of-tree write (e.g. `cp secret.txt /opt/x`, `mv x ~/.ssh/`).
        // Reads are intentionally not fenced here — see `extract_write_path_targets`.
        let internal_ctx = InternalPathContext {
            cwd: &cwd,
            session_plan_file: ctx.permission_context.session_plan_file.as_deref(),
        };
        let write_targets = coco_shell::extract_write_path_targets(command);
        if command_analysis.has_non_noop_cwd_change() && !write_targets.is_empty() {
            return coco_types::ToolCheckResult::Ask {
                message: "Filesystem writes after changing directories require approval.".into(),
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        for target in write_targets {
            if has_shell_expansion(&target) {
                return coco_types::ToolCheckResult::Ask {
                    message: "Shell expansion syntax in a write path requires manual approval."
                        .into(),
                    suggestions: Vec::new(),
                    choices: None,
                    detail: None,
                };
            }
            let allowed = is_path_within_allowed_dirs(&target, &cwd, &additional_dirs)
                || is_editable_internal_path(&target, &internal_ctx);
            if !allowed {
                return coco_types::ToolCheckResult::Ask {
                    message: format!(
                        "Writing to '{target}' is outside the allowed working directories \
                         and requires manual approval."
                    ),
                    suggestions: out_of_dir_suggestion(&target, &cwd),
                    choices: None,
                    detail: None,
                };
            }
        }

        // acceptEdits: a filesystem subcommand auto-allows (#164).
        if ctx.permission_context.mode == coco_types::PermissionMode::AcceptEdits
            && coco_shell::is_auto_allowed_in_accept_edits(command)
        {
            return coco_types::ToolCheckResult::Allow {
                updated_input: None,
                feedback: None,
            };
        }

        // Read-only commands skip security analysis and defer to rules.
        if is_read_only_command(command) {
            return coco_types::ToolCheckResult::Passthrough;
        }

        let checks = check_security(command);
        if let Some(deny) = checks.iter().find(|c| c.severity == SecuritySeverity::Deny) {
            return coco_types::ToolCheckResult::Deny {
                message: format!(
                    "Command blocked by coco-rs security check (stricter than Claude Code): {}. \
                     If you believe this is a false positive, use a sandbox or restructure the \
                     command.",
                    deny.message
                ),
            };
        }
        // Only a CURATED set of narrow, high-confidence risks routes to a prompt
        // — the ones that map 1:1 to the specific `behavior:'ask'` validators.
        // The broader analyzer suite (command substitution, metacharacters,
        // code-exec, …) stays computed-but-informational: coco-rs's analyzers
        // lack the safe-substitution carve-outs, so routing them all would
        // prompt on common commands like `for f in $(ls)` or `tar …$(date)…`.
        const ASK_SECURITY_CHECK_IDS: &[coco_shell::SecurityCheckId] = &[
            coco_shell::SecurityCheckId::JQ_SYSTEM_FUNCTION, // jq system()/file flags (#162)
            coco_shell::SecurityCheckId::DANGEROUS_VARIABLES, // $VAR adjacent to pipe/redirect (#167)
            coco_shell::SecurityCheckId::IFS_INJECTION,       // IFS= reassignment
        ];
        if let Some(ask) = checks
            .iter()
            .find(|c| c.severity == SecuritySeverity::Ask && ASK_SECURITY_CHECK_IDS.contains(&c.id))
        {
            return coco_types::ToolCheckResult::Ask {
                message: format!(
                    "A security check flagged this command ({}). Run it?",
                    ask.message
                ),
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        }
        coco_types::ToolCheckResult::Passthrough
    }

    /// Destructive iff NOT read-only. The upstream permission evaluator uses this
    /// flag to decide whether the command needs user approval. The read-only fast
    /// path auto-allows before reaching the Ask phase.
    fn is_destructive(&self, input: &BashInput) -> bool {
        !Tool::is_read_only(self, input)
    }

    /// Tool-result persistence threshold: `maxResultSizeChars: 30_000`.
    /// When Bash output exceeds this budget, the executor persists the full
    /// output to a tool-results file and only keeps a truncated snippet inline.
    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        coco_tool_runtime::ResultSizeBound::Chars(30_000)
    }

    /// Render the structured `data` envelope into model-visible content parts.
    ///
    /// Branches:
    /// 1. **structuredContent present** (image stdout): decode each block
    ///    into a `FileData` (image) part. This is what enables Anthropic /
    ///    Gemini 3+ to actually see image bytes captured by `cat foo.png`.
    /// 2. **Text path**: build a single Text part by joining
    ///    `[processedStdout, errorMessage, exitTail, backgroundInfo]` with
    ///    `\n`, skipping empty pieces. `processedStdout` strips leading
    ///    blank-only lines + trims trailing whitespace. A backgrounded
    ///    command carries `backgroundTaskId` + `outputPath`, so the
    ///    `backgroundInfo` line names both — matching TS so the model can
    ///    `Read` the output file. Oversized text output is persisted by the
    ///    query-level generic Level 1 tool-result pipeline, not by Bash.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        // Branch 1: image stdout — decode the structuredContent envelope
        // into FileData parts so multimodal-capable providers see the
        // raw image bytes.
        if let Some(arr) = data.get("structuredContent").and_then(Value::as_array) {
            let parts: Vec<ToolResultContentPart> = arr
                .iter()
                .filter_map(|block| {
                    let kind = block.get("type")?.as_str()?;
                    match kind {
                        "image" => {
                            let source = block.get("source")?;
                            let media_type = source.get("media_type")?.as_str()?.to_string();
                            let b64 = source.get("data")?.as_str()?.to_string();
                            Some(ToolResultContentPart::FileData {
                                data: b64,
                                media_type,
                                filename: None,
                                provider_options: None,
                            })
                        }
                        "text" => {
                            let text = block.get("text")?.as_str()?.to_string();
                            Some(ToolResultContentPart::Text {
                                text,
                                provider_options: None,
                            })
                        }
                        _ => None,
                    }
                })
                .collect();
            if !parts.is_empty() {
                return parts;
            }
        }

        // Branch 2: text path.
        let stdout = data.get("stdout").and_then(Value::as_str).unwrap_or("");
        let stderr = data.get("stderr").and_then(Value::as_str).unwrap_or("");
        let interrupted = data
            .get("interrupted")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let processed = strip_leading_blank_lines(stdout).trim_end().to_string();

        let mut error_message = stderr.trim().to_string();
        if interrupted {
            if !error_message.is_empty() {
                error_message.push('\n');
            }
            error_message.push_str("<error>Command was aborted before completion</error>");
        }

        // Background-info text: a backgrounded command carries
        // `backgroundTaskId` + `outputPath`. The notice names both so the
        // model can `Read` the output file directly. The `assistantAuto` /
        // `user` / `explicit` variant is decided by the flags in `data`.
        let background_info = data
            .get("backgroundTaskId")
            .and_then(Value::as_str)
            .map(|task_id| {
                let output_path = data.get("outputPath").and_then(Value::as_str).unwrap_or("");
                format_background_notice(BackgroundKind::from_result(data), task_id, output_path)
            })
            .unwrap_or_default();

        // Surface non-zero exit codes. A command that fails only via its exit
        // code with no stdout/stderr (`false`, a script that `exit 1`s silently)
        // would otherwise render as empty — the model could not tell it failed.
        // Append the bare `Exit code N` ONLY when the command-aware interpreter
        // classifies it as a genuine error. Expected non-zero codes (grep
        // no-match, diff differs, test false, find inaccessible) are
        // `is_error: false` → nothing is appended; their friendly explanation
        // is TUI-only.
        let exit_tail = {
            let exit_code = data.get("exitCode").and_then(Value::as_i64).unwrap_or(0);
            let command = data.get("command").and_then(Value::as_str).unwrap_or("");
            if exit_code != 0
                && !interrupted
                && coco_shell::semantics::interpret_command_result(command, exit_code as i32)
                    .is_error
            {
                format!("Exit code {exit_code}")
            } else {
                String::new()
            }
        };

        let combined = [
            processed.as_str(),
            error_message.as_str(),
            exit_tail.as_str(),
            background_info.as_str(),
        ]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<&str>>()
        .join("\n");

        vec![ToolResultContentPart::Text {
            text: combined,
            provider_options: None,
        }]
    }

    fn validate_input(&self, input: &BashInput, _ctx: &ToolUseContext) -> ValidationResult {
        if input.command.is_empty() {
            return ValidationResult::invalid("missing required field: command");
        }
        // No max timeout is enforced — the configured max is only an advisory
        // hint in the schema description. The model's raw timeout is honored.
        ValidationResult::Valid
    }

    async fn execute(
        &self,
        input: BashInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        // ── R7-T11: _simulatedSedEdit short-circuit ──
        //
        // The BashTool input schema accepts an internal `_simulatedSedEdit:
        // { filePath, newContent }` field that the SedEditPermissionRequest
        // TUI dialog populates when the user reviews a `sed -i ...` command
        // and chooses to convert it to a previewed Edit-style write. The
        // dialog does the actual sed-against-original computation and hands
        // BashTool the precomputed `newContent` so what the user previewed
        // is exactly what gets written.
        //
        // The field is **deliberately omitted** from `input_schema()` so
        // the model can never see it as a valid input. The upstream executor
        // SHOULD also strip incoming `_simulatedSedEdit` payloads before
        // invoking this method as a defense-in-depth measure. We emit a
        // debug log when the field is present so anomalous traffic is
        // visible even if the executor strip is missing.
        if let Some(sed_input) = input.simulated_sed_edit.as_ref() {
            tracing::debug!(
                "BashTool received _simulatedSedEdit input — applying as Edit-style write"
            );
            return apply_sed_edit(sed_input, ctx).await;
        }

        let command = input.command.as_str();

        // A falsy (0) or absent timeout falls back to the default;
        // no max clamp (the raw value is honored).
        let timeout_ms = input
            .timeout
            .filter(|&t| t > 0)
            .unwrap_or_else(|| default_timeout_ms(&ctx.tool_config));

        let run_in_background = input.run_in_background;

        // R6-T18: sandbox decision:
        //   1. If sandbox not globally enabled → unsandboxed
        //   2. If `dangerouslyDisableSandbox` and bypass allowed → unsandboxed
        //   3. If command matches an excluded pattern → unsandboxed
        //   4. Otherwise → sandboxed
        //
        // Decision evaluation lives on `SandboxState::command_snapshot`;
        // this site only resolves whether sandboxing is reachable at all
        // (feature gate + bootstrap supplied the state) and forwards the
        // bypass flag.
        let sandbox_state = active_sandbox_state(ctx);
        let sandbox_bypass = SandboxBypass::from_flag(input.dangerously_disable_sandbox);
        if let Some(state) = &sandbox_state {
            let snapshot = state.command_snapshot(command, sandbox_bypass);
            if snapshot.should_wrap {
                tracing::debug!(
                    enforcement = ?snapshot.enforcement,
                    command = %command,
                    "bash command will run wrapped by SandboxState platform"
                );
            }
        }

        // Permission pipeline.
        //
        // Stage 1 — read-only fast path. Already handled by `is_read_only()`
        // at the trait level; the upstream permission evaluator auto-allows
        // read-only commands and batches them with other concurrency-safe tools.
        //
        // Stage 2 — security analysis. `check_security` runs the full
        // `coco_shell_parser` analyzer suite (29 quote/heredoc-aware
        // validators). ALL analyzer-caught risks (eval, IFS=, backtick
        // substitution, brace expansion, comment/quote desync, …) map to
        // `SecuritySeverity::Ask` and pass through to the normal permission
        // prompt below.
        //
        // `check_security` retains only TWO coco-rs-specific `Deny` checks for
        // genuinely-catastrophic constructs with no legitimate use here: raw
        // control / zero-width characters and `/proc/*/environ` access. Those
        // hard-fail without prompting (a DELIBERATE divergence we keep because
        // they are near-always obfuscation/secret-exfiltration attempts).
        //
        // User-configured deny rules and path validation are a different
        // concern from the pattern checks here.
        //
        // Read-only commands skip the security check to avoid false positives
        // on harmless `grep 'foo`bar'` patterns with metacharacters inside
        // quoted strings.
        //
        // NOTE: destructive-command detection is intentionally NOT a block.
        // It is a purely informational advisory — it never affects permission
        // logic. The `coco_shell::destructive::get_destructive_warning`
        // advisory is available for the permission-request UI but does not
        // deny here.
        if !is_read_only_command(command) {
            for check in check_security(command) {
                if check.severity == SecuritySeverity::Deny {
                    return Err(ToolError::PermissionDenied {
                        message: format!(
                            "Command blocked by coco-rs security check (stricter than \
                             Claude Code): {}. If you believe this is a false positive, \
                             use a sandbox or restructure the command.",
                            check.message
                        ),
                    });
                }
            }
        }

        // `description: description || command` — the model's input.description
        // takes precedence, falling back to the command string when omitted.
        let resolved_description = input
            .description
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| command.to_string());

        // W3: unified fg/bg execution via TaskRuntime when available.
        // - Always spawn through `spawn_shell_task`.
        // - `run_in_background: true` → return `{backgroundTaskId, outputPath}`.
        // - Otherwise → race terminal/detach/cancel/auto-detach inside
        //   `tool.execute`, compose either fg-shape `{stdout, exitCode,
        //   interrupted}` or bg-shape `{backgroundTaskId, outputPath, ...}`.
        //
        // Tests / minimal embeddings without a TaskRuntime fall back
        // to the legacy ShellExecutor path (`execute_foreground`).
        // W6: TaskRuntime now supports sandbox wrap (sandbox params
        // threaded into `BackgroundShellRequest`).
        if ctx.task_handle.is_some() {
            return execute_via_task_runtime(
                command,
                &resolved_description,
                timeout_ms,
                run_in_background,
                ctx,
                sandbox_state.clone(),
                sandbox_bypass,
            )
            .await;
        }

        // Fallback: no TaskRuntime in this context.
        if run_in_background {
            return Err(ToolError::ExecutionFailed {
                message:
                    "Background task execution is not available in this context (no TaskRuntime)."
                        .into(),
                display_data: None,
                source: None,
            });
        }
        execute_foreground(command, timeout_ms, ctx, sandbox_state, sandbox_bypass).await
    }
}

/// W3: unified fg/bg execution path via TaskRuntime.
///
/// Always spawns through `spawn_shell_task` (the same primitive bg
/// used). The fg/bg distinction is purely about which `tool.execute`
/// arm wins the `select!`:
///
/// - `run_in_background: true` → return `{backgroundTaskId, outputPath}`
///   immediately (one `output_file_path` await, no command await).
/// - `run_in_background: false` → race four signals:
///   1. `ctx.abort.cancelled()` (Ctrl+C / explicit kill) → return
///      `ToolError::Cancelled`.
///   2. `terminal_signal.await_terminal()` → read disk via
///      `read_terminal_outputs` and return fg-shape result.
///   3. `detach.notified()` → external `signal_detach` (TUI Ctrl+B or
///      another co-routine) → return bg-shape result; task keeps
///      running.
///   4. Auto-detach timer (when `auto_background_on_timeout` config is set;
///      see `ASSISTANT_BLOCKING_BUDGET_MS = 15_000`) → same as (3) but the
///      timer itself fires `signal_detach`. The
///      detach arm in (3) observes the notification.
///
/// This replaces both the old `execute_background` and the D5 re-run
/// path on foreground timeout — the previous code re-spawned the
/// command after timeout, duplicating side effects of `npm publish` /
/// `git push` / etc. The unified path **never** re-spawns: the same
/// child keeps running, the fg awaiter just stops blocking.
#[allow(clippy::too_many_arguments)]
async fn execute_via_task_runtime(
    command: &str,
    description: &str,
    timeout_ms: u64,
    run_in_background: bool,
    ctx: &ToolUseContext,
    sandbox_state: Option<std::sync::Arc<SandboxState>>,
    sandbox_bypass: SandboxBypass,
) -> Result<ToolResult<Value>, ToolError> {
    let task_handle = ctx.task_handle.as_ref().ok_or_else(|| {
        // Caller (`execute`) guards with `task_handle.is_some()`.
        // Reaching here is a programmer error, not user input.
        ToolError::ExecutionFailed {
            message: "execute_via_task_runtime invoked without task_handle".into(),
            display_data: None,
            source: None,
        }
    })?;

    let tool_use_id = ctx.tool_use_id.clone();
    let agent_id = ctx.agent_id.as_ref().map(|a| a.as_str().to_string());

    // Auto-background-on-timeout: for a foreground, main-thread command that's
    // eligible (anything but `sleep`), the command's own `timeout_ms` becomes
    // the auto-detach budget — when it elapses the fg awaiter is released with
    // a bg-shape result and the child KEEPS RUNNING (the driver does not kill
    // it). Subagents and bg-spawned commands don't auto-detach (no fg awaiter
    // to release).
    let auto_background = !run_in_background
        && ctx.agent_id.is_none()
        && ctx.tool_config.bash.auto_background_on_timeout
        && super::bash_advanced::is_autobackgrounding_allowed(command);
    let auto_detach_ms = if auto_background {
        Some(timeout_ms)
    } else {
        None
    };
    // When auto-backgrounding, the timeout must NOT kill the child — it only
    // releases the fg awaiter (via the auto-detach timer above). Otherwise
    // (sleep, subagents, explicit bg) the driver hard-kills on timeout.
    let kill_on_timeout = !auto_background;

    // Progress emission: fg mode only. Bg-spawned commands return
    // immediately so the model has no live receiver. Bg progress flows
    // through `<task-notification>` envelopes later.
    let progress_tx = if run_in_background {
        None
    } else {
        ctx.progress_tx.clone()
    };

    let req = BackgroundShellRequest {
        command: command.to_string(),
        shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
        description: description.to_string(),
        timeout_ms: Some(timeout_ms as i64),
        tool_use_id,
        issuing_agent: agent_id,
        progress_tx,
        progress_throttle_ms: 1000,
        auto_detach_ms,
        kill_on_timeout,
        // W6: sandbox params from `execute` (single resolution site
        // for `dangerouslyDisableSandbox` parsing). Both fg and bg
        // paths now apply the same wrap as the legacy ShellExecutor
        // foreground path did.
        sandbox_state,
        sandbox_bypass,
    };

    let task_id =
        task_handle
            .spawn_shell_task(req)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to spawn shell task: {e}"),
                display_data: None,
                source: None,
            })?;

    // Bg path: return now. The task runs detached, will push a
    // `<task-notification>` envelope on terminal. The result carries the
    // task id + the on-disk output path so the model can `Read` the file
    // directly (mirrors TS BashTool).
    if run_in_background {
        let output_path = background_output_path(task_handle, &task_id).await;
        return Ok(ToolResult {
            data: serde_json::json!({
                "backgroundTaskId": task_id,
                "outputPath": output_path,
            }),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        });
    }

    // Fg path: subscribe to terminal + detach handles, race.
    let terminal = task_handle
        .subscribe_terminal(&task_id)
        .await
        .ok_or_else(|| ToolError::ExecutionFailed {
            message: "task vanished after spawn (no terminal handle)".into(),
            display_data: None,
            source: None,
        })?;
    let detach =
        task_handle
            .detach_handle(&task_id)
            .await
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "task vanished after spawn (no detach handle)".into(),
                display_data: None,
                source: None,
            })?;

    let kill_arm = ctx.cancel_token();

    let outcome: BashOutcome = tokio::select! {
        biased;
        () = kill_arm.cancelled() => {
            // Cancel propagates into the task driver via its own cancel
            // token (BashTool ctx.cancel_token() is shared with the driver). The
            // driver will fire `apply_shell_terminal_state(Killed)` and
            // push the notification — we don't need to do anything
            // beyond returning the cancellation error.
            BashOutcome::Cancelled
        }
        _ = terminal.await_terminal() => {
            BashOutcome::Terminal
        }
        () = detach.notified() => {
            BashOutcome::Detached { by_user: true }
        }
    };

    match outcome {
        BashOutcome::Cancelled => Err(ToolError::ExecutionFailed {
            message: "Bash command was interrupted by the user.".into(),
            display_data: None,
            source: None,
        }),
        BashOutcome::Terminal => {
            // Compose fg-shape result from disk + persisted exit_code.
            let outputs = task_handle
                .read_terminal_outputs(&task_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("Failed to read terminal outputs: {e}"),
                    display_data: None,
                    source: None,
                })?;
            let max_bytes = max_output_bytes(&ctx.tool_config);
            let stdout = truncate_output(outputs.stdout.as_bytes(), max_bytes);
            let stderr = truncate_output(outputs.stderr.as_bytes(), max_bytes);
            // Strip + record Claude Code hints so the model never sees the tag.
            let stdout = maybe_strip_and_record_hints(stdout, command);
            let mut result_obj = serde_json::json!({
                "stdout": stdout,
                "stderr": stderr,
                "exitCode": outputs.exit_code,
                "interrupted": outputs.interrupted,
                // Carried so `render_for_model` can interpret the exit code with
                // command-aware semantics. This is the PRODUCTION path (a
                // TaskRuntime is wired); without it the interpreter saw an empty
                // command and labelled every non-zero exit a generic error.
                "command": command,
            });
            // Image detection on bytes from disk. The unified path
            // reads the file via `read_terminal_outputs` which returns
            // a `String` (UTF-8 lossy) — magic-byte detection still
            // works since the leading bytes survive lossy conversion
            // for raster formats.
            let raw_bytes = stdout.as_bytes();
            if is_likely_image_bytes(raw_bytes) {
                result_obj["isImage"] = serde_json::Value::Bool(true);
                if let Some(content) = Some(build_image_block(raw_bytes)) {
                    result_obj["structuredContent"] = content;
                }
            }
            Ok(ToolResult {
                data: result_obj,
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            })
        }
        BashOutcome::Detached { by_user } => {
            // Auto-detach timer or external `signal_detach` fired.
            // Differentiate the two with `by_user`: when the
            // auto-detach timer is the originator, the task's own
            // `BgAgentExtras.is_backgrounded()` flip is observable —
            // but for now we just stamp the differentiator in the
            // result shape so the model sees the right signals
            // (`backgroundedByUser` vs `assistantAutoBackgrounded`).
            //
            // `by_user` is `true` whenever the detach arm wins; we
            // can't distinguish auto-detach-timer from external-TUI
            // sources here without an extra atomic. Default to
            // `backgroundedByUser=true` matching the most-common
            // interactive case; auto-detach will surface a follow-up
            // path in a later refactor.
            let _ = by_user;
            let output_path = background_output_path(task_handle, &task_id).await;
            Ok(ToolResult {
                data: serde_json::json!({
                    "backgroundTaskId": task_id,
                    "outputPath": output_path,
                    "backgroundedByUser": true,
                }),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            })
        }
    }
}

/// Outcome of the W3 fg `tokio::select!` race.
enum BashOutcome {
    Cancelled,
    Terminal,
    Detached { by_user: bool },
}

/// Execute a command in the foreground with continuous progress reporting.
async fn execute_foreground(
    command: &str,
    timeout_ms: u64,
    ctx: &ToolUseContext,
    sandbox_state: Option<std::sync::Arc<SandboxState>>,
    sandbox_bypass: SandboxBypass,
) -> Result<ToolResult<Value>, ToolError> {
    // 4-tier cwd resolution. Spawn at the live session cwd; the
    // out-of-project guard runs AFTER exec so the offending command runs
    // in /tmp / the drifted dir and the annotation lands on its stderr.
    let cwd = crate::tools::shell_cwd::resolve_spawn_cwd(ctx).await;

    // Prefer the session-scoped provider (snapshot + session-env + `/env`
    // + shell-prefix all live there). Fall back to per-call construction
    // for legacy / test paths that haven't wired the provider yet.
    let mut executor = match ctx.shell_provider.clone() {
        Some(provider) => coco_shell::ShellExecutor::with_provider(&cwd, provider),
        None => coco_shell::ShellExecutor::new_with_config(&cwd, &ctx.shell_config),
    };

    // R6-T17 + R6-T18: thread the ctx cancel token and the sandbox state
    // through to the shell executor. When `sandbox_state` is `Some` and
    // the per-command snapshot says `should_wrap`, the executor calls
    // `SandboxPlatform::wrap_command` before spawning the child.
    //
    // Snapshot the violation count *before* spawning so we can splice
    // anything that landed during this command into stderr.
    let violations_baseline = if let Some(state) = &sandbox_state {
        Some(state.violations_total_snapshot().await)
    } else {
        None
    };

    let opts = coco_shell::ExecOptions {
        timeout_ms: Some(timeout_ms as i64),
        cancel: Some(ctx.cancel_token()),
        sandbox: sandbox_state.clone(),
        sandbox_bypass,
        ..Default::default()
    };

    // Use streaming execution with progress if progress channel is available
    let cmd_result = if let Some(progress_tx) = ctx.progress_tx.clone() {
        let tool_use_id: std::sync::Arc<str> = ctx.tool_use_id.clone().unwrap_or_default().into();

        // Initial "running" progress
        let _ = progress_tx.send(ToolProgress {
            tool_use_id: tool_use_id.to_string(),
            parent_tool_use_id: None,
            data: serde_json::json!({
                "type": "bash_progress",
                "status": "running",
                "command": command,
            }),
        });

        // Continuous progress via streaming executor (~1s interval)
        let progress_id = tool_use_id.clone();
        executor
            .execute_with_progress(command, &opts, move |progress| {
                let _ = progress_tx.send(ToolProgress {
                    tool_use_id: progress_id.to_string(),
                    parent_tool_use_id: None,
                    data: serde_json::json!({
                        "type": "bash_progress",
                        "status": "running",
                        "elapsedTimeSeconds": progress.elapsed_seconds,
                        "totalBytes": progress.total_bytes,
                    }),
                });
            })
            .await
    } else {
        executor.execute(command, &opts).await
    };

    let mut cmd_result = cmd_result.map_err(|e| ToolError::ExecutionFailed {
        message: format!("shell execution failed: {e}"),
        display_data: None,
        source: None,
    })?;

    // setCwd(new_cwd) → resetCwdIfOutsideProject. If reset fires, the
    // annotation lands on THIS command's stderr. No-op for worktree subagents.
    let reset_message =
        crate::tools::shell_cwd::finalize_cwd_post_exec(ctx, cmd_result.new_cwd.clone()).await;
    crate::tools::shell_cwd::annotate_stderr_with_reset(&mut cmd_result.stderr, reset_message);

    // Annotate stderr with any sandbox violations recorded during this
    // command — violations are informational, not blocking.
    if let (Some(state), Some(prev)) = (&sandbox_state, violations_baseline)
        && let Some(annotation) = state.format_violations_since(prev).await
    {
        if cmd_result.stderr.is_empty() {
            cmd_result.stderr = annotation;
        } else {
            cmd_result.stderr.push('\n');
            cmd_result.stderr.push_str(&annotation);
        }
    }

    // ── B4.2: auto-background-on-timeout ──
    //
    // On foreground timeout, the intended behavior is to convert the running
    // process into a background task (process keeps running, detached from
    // the fg await). This requires the shell executor to support process
    // handle transfer, which coco-rs's current `ShellExecutor` does not.
    //
    // Until that architectural change lands, we fall back to a
    // safer-but-weaker behavior: on timeout, return an error that
    // **explicitly recommends** retrying with `run_in_background=true`.
    // The structured payload tells the model:
    //   - the command did time out (not a different error)
    //   - a task_handle is available so bg retry is possible
    //   - the original command + timeout so the retry is trivial
    //
    // This is strictly an error-message improvement — we don't risk
    // double-execution of side-effectful commands by re-spawning on
    // the tool's behalf. Re-spawn would happen only if the model
    // explicitly issued a new Bash call with run_in_background=true,
    // which is the user's explicit opt-in.
    if cmd_result.timed_out {
        let bg_available = ctx.task_handle.is_some();
        let suggestion = if bg_available {
            format!(
                " The command is a candidate for background execution. \
                 Retry with `run_in_background: true` if you want it to \
                 keep running past the {timeout_ms}ms limit."
            )
        } else {
            String::new()
        };
        return Err(ToolError::ExecutionFailed {
            message: format!("Command timed out after {timeout_ms}ms.{suggestion}"),
            display_data: None,
            source: None,
        });
    }

    let max_bytes = max_output_bytes(&ctx.tool_config);
    // The `stdout` String field comes from `cmd_result.stdout`, which
    // the executor has already cleaned up (CWD-marker stripped via
    // `extract_cwd_from_output`). Truncate that for the inline view.
    let stdout = truncate_output(cmd_result.stdout.as_bytes(), max_bytes);

    // Claude Code hints protocol: CLIs/SDKs emit a `<claude-code-hint />`
    // tag to stderr (merged into stdout here). Scan, record for the TUI's
    // pending-hint dialog to surface, then strip so the model never sees
    // the tag — a zero-token side channel. Stripping runs unconditionally
    // (subagent output must stay clean too); recording is best-effort and
    // never affects the tool result.
    let stdout = maybe_strip_and_record_hints(stdout, command);
    let stderr = truncate_output(cmd_result.stderr.as_bytes(), max_bytes);
    let exit_code = cmd_result.exit_code;
    // R7-T18: image detection inspects the executor's raw stdout bytes
    // (pre-UTF-8-lossy) when available so the magic-byte signature
    // isn't mangled by replacement characters. Note this DOES include
    // the CWD marker for shells that emit one — the detector is
    // resilient to trailing bytes (PNG / JPEG / GIF / WebP all match
    // on the leading bytes only). The fallback to `cmd_result.stdout`
    // covers test stubs that don't populate `stdout_bytes`.
    let raw_stdout_bytes_for_detection: &[u8] = cmd_result
        .stdout_bytes
        .as_deref()
        .unwrap_or(cmd_result.stdout.as_bytes());

    // R5-T14 + R6-T17 + R7-T12: structured output envelope:
    //
    //   { stdout, stderr, exitCode, interrupted, isImage?,
    //     backgroundTaskId?, structuredContent?, persistedOutputPath?,
    //     persistedOutputSize? }
    //
    // `interrupted` is sourced from the shell executor's own `interrupted`
    // flag, which is set when the ctx cancel token fires and the child
    // process is killed. That's distinct from a timeout (the executor sets
    // `timed_out` for the watchdog path). Either condition surfaces as
    // `interrupted=true` to the model.
    let interrupted = cmd_result.interrupted || cmd_result.timed_out;
    if cmd_result.interrupted && ctx.abort.is_aborted() {
        return Err(ToolError::Cancelled);
    }

    // R7-T12 fields:
    //
    // 1. `isImage`: detect from stdout magic bytes when e.g. `cat image.png`
    //    returns binary image data, so the UI can render it inline.
    // 2. `structuredContent`: when stdout IS an image, attach a base64
    //    multimodal block so the model receives the actual pixels.
    //    For text output, structuredContent is omitted (the plain
    //    `stdout` field is enough).
    // 3. Oversized text output is handled by the generic query-level
    //    Tool Result Budget pipeline. Bash keeps the structured
    //    envelope focused on stdout/stderr/exit status and does not
    //    write model-visible temp files.
    let is_image = is_likely_image_bytes(raw_stdout_bytes_for_detection);
    let structured_content = if is_image {
        Some(build_image_block(raw_stdout_bytes_for_detection))
    } else {
        None
    };
    let mut result_obj = serde_json::json!({
        "stdout": stdout,
        "stderr": stderr,
        "exitCode": exit_code,
        "interrupted": interrupted,
        // Carried so `render_for_model` can interpret the exit code with
        // command-aware semantics (grep no-match vs genuine error).
        "command": command,
    });
    if is_image {
        result_obj["isImage"] = serde_json::Value::Bool(true);
    }
    if let Some(content) = structured_content {
        result_obj["structuredContent"] = content;
    }
    Ok(ToolResult {
        data: result_obj,
        new_messages: vec![],
        app_state_patch: None,
        permission_updates: Vec::new(),
        display_data: None,
    })
}

/// Strip `<claude-code-hint />` tags from bash stdout and best-effort
/// record any plugin hints for the TUI's pending-hint dialog. Returns the
/// stripped stdout (what the model sees). Recording failures (disk I/O,
/// policy lookups) are swallowed — they must never affect the tool result.
pub(crate) fn maybe_strip_and_record_hints(stdout: String, command: &str) -> String {
    let (hints, stripped) = coco_plugins::extract_claude_code_hints(&stdout, command);
    for hint in &hints {
        // `None` installed-manager: skip the per-call installed-check (the
        // async resolve step still gates on marketplace-cache membership).
        coco_plugins::maybe_record_plugin_hint(hint, None);
    }
    stripped
}

/// Detect whether a byte buffer is a known image format from its magic
/// bytes. Used by the UI to render the result as an inline image
/// rather than attempting to display raw bytes as text.
///
/// Recognized formats (order = check priority):
///  - PNG: `89 50 4E 47 0D 0A 1A 0A`
///  - JPEG: `FF D8 FF`
///  - GIF: `47 49 46 38` (`GIF8`, both 87a and 89a)
///  - WebP: `52 49 46 46` ... `57 45 42 50` (`RIFF`...`WEBP`)
///  - BMP: `42 4D` (`BM`)
pub(crate) fn is_likely_image_bytes(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        return true;
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return true;
    }
    if bytes.starts_with(b"GIF8") {
        return true;
    }
    // WebP: RIFF....WEBP — needs at least 12 bytes and the magic at
    // positions 0-3 + 8-11.
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return true;
    }
    if bytes.starts_with(b"BM") && bytes.len() > 14 {
        return true;
    }
    false
}

/// Build a multimodal content-block array containing a single base64
/// image entry from raw image bytes. The MIME type is inferred from
/// the magic bytes detected in `is_likely_image_bytes` so the model
/// receives the correct content type. Used to populate the
/// `structuredContent` field of the BashTool result envelope when
/// stdout is an image.
///
/// Each magic-byte check mirrors `is_likely_image_bytes` exactly,
/// including the BMP `len() > 14` length gate, so direct callers
/// can't accidentally tag a 2-byte `b"BM"` payload as a BMP image.
fn build_image_block(bytes: &[u8]) -> Value {
    use base64::Engine;
    let mime = if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.len() > 14 && bytes.starts_with(b"BM") {
        "image/bmp"
    } else {
        "application/octet-stream"
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    serde_json::json!([
        {
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": mime,
                "data": b64,
            }
        }
    ])
}

/// Apply a previewed sed edit by writing the precomputed `newContent`
/// to `filePath`, preserving the file's original encoding and line
/// endings. Used by the `_simulatedSedEdit` short-circuit in
/// `BashTool::execute` so that what the user previewed in the
/// SedEditPermissionRequest dialog is exactly what hits disk.
///
/// Behavior:
///   1. Resolve the absolute path (`canonicalize` with a fallback to
///      the input path).
///   2. Read the original file metadata to detect encoding + line
///      endings — sed preserves both.
///   3. ENOENT → return a sed-formatted error message via stderr,
///      `exitCode: 1`, never throw. The model sees a normal Bash
///      result with the sed CLI's "No such file or directory" text.
///   4. Track file history before mutating (matches `track_file_edit`
///      called from FileEditTool/FileWriteTool).
///   5. Write the new content with the detected encoding + line
///      endings (NOT the LF-always policy used by FileWriteTool —
///      sed is an in-place edit and must round-trip the format).
///   6. Update `FileReadState` so subsequent edits/writes don't trip
///      the read-before-write check, and so the file_unchanged dedup
///      cache is still consistent.
///   7. Return `{ stdout: "", stderr: "", exitCode: 0, interrupted: false }`.
///
/// `sed_input` must be a JSON object with string `filePath` and string
/// `newContent` fields. Missing/wrong-type fields surface as
/// `InvalidInput` errors.
async fn apply_sed_edit(
    sed_input: &SimulatedSedEdit,
    ctx: &ToolUseContext,
) -> Result<ToolResult<Value>, ToolError> {
    let file_path = sed_input.file_path.as_str();
    let new_content = sed_input.new_content.as_str();

    let path = std::path::Path::new(file_path);

    // ENOENT → sed-shaped error envelope, NOT a tool error, so the model
    // can pattern-match `sed: ... No such file ...` and recover.
    if !path.exists() {
        return Ok(ToolResult {
            data: serde_json::json!({
                "stdout": "",
                "stderr": format!("sed: {file_path}: No such file or directory"),
                "exitCode": 1,
                "interrupted": false,
            }),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        });
    }

    // Detect encoding + line endings from the existing file so the
    // write round-trips the format. Read failure (rare — file existed
    // a microsecond ago) → sed-style error.
    let (_old_content, encoding, line_ending) = match coco_file_encoding::read_with_format(path) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolResult {
                data: serde_json::json!({
                    "stdout": "",
                    "stderr": format!("sed: {file_path}: {e}"),
                    "exitCode": 1,
                    "interrupted": false,
                }),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            });
        }
    };

    // File-history snapshot before mutating. Matches FileEditTool /
    // FileWriteTool / NotebookEditTool ordering.
    crate::track_file_edit(ctx, path).await;

    // Write the previewed content, preserving original encoding +
    // line endings (the key difference from FileWriteTool which
    // always normalizes to LF).
    if let Err(e) = coco_file_encoding::write_with_format(path, new_content, encoding, line_ending)
    {
        return Err(ToolError::ExecutionFailed {
            message: format!("failed to write sed-edit result to {file_path}: {e}"),
            display_data: None,
            source: None,
        });
    }

    // Refresh the cache so the next Edit/Write doesn't fail its mtime
    // check against a stale entry left by the earlier Read.
    crate::record_file_edit(ctx, path, new_content.to_string()).await;
    // Fire skill auto-discovery + conditional-skill activation when a sed
    // pipeline touches a path.
    crate::track_skill_triggers(ctx, path).await;

    Ok(ToolResult {
        data: serde_json::json!({
            "stdout": "",
            "stderr": "",
            "exitCode": 0,
            "interrupted": false,
        }),
        new_messages: vec![],
        app_state_patch: None,
        permission_updates: Vec::new(),
        display_data: None,
    })
}

/// Truncate output head-only: keep the first `max_bytes` chars and append
/// `\n\n... [N lines truncated] ...` where N counts the lines dropped from
/// the tail (newlines after the cut, +1).
///
/// Char boundaries are respected so truncation never yields invalid UTF-8.
fn truncate_output(bytes: &[u8], max_bytes: usize) -> String {
    let s = String::from_utf8_lossy(bytes);
    if s.len() <= max_bytes {
        return s.to_string();
    }

    // Snap the cut to the nearest preceding char boundary.
    let mut cut = max_bytes.min(s.len());
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &s[..cut];
    // Count newlines after cut, +1 for the final (possibly unterminated) line.
    let remaining_lines = s[cut..].matches('\n').count() + 1;
    format!("{head}\n\n... [{remaining_lines} lines truncated] ...")
}

#[cfg(test)]
#[path = "bash.test.rs"]
mod tests;
