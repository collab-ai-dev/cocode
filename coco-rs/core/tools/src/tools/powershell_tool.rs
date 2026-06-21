//! PowerShellTool — security-gated pwsh execution.
//!
//! R5-T9: rounds 1-4 shipped a bare `PowerShellTool` that just spawned
//! `pwsh` with the raw command; the full security pipeline living in
//! `core/tools/src/tools/powershell.rs` (CLM type gate, git-internal-path
//! guard, UNC-path guard, UTF-16 output decode) was dead code. This
//! module re-houses the tool and wires every helper into the execute
//! path so PowerShell behaves like Bash: a permission gate, a read-only
//! fast path, and a destructive warning phase before the child is ever
//! spawned.

use coco_messages::ToolResult;
use coco_sandbox::SandboxBypass;
use coco_sandbox::SandboxState;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use super::background_task::BackgroundKind;
use super::background_task::background_output_path;
use super::background_task::format_background_notice;
use super::powershell::analyze_ps_security;
use super::powershell::classify_ps_command;
use super::powershell::decode_ps_output;
use super::powershell::is_vulnerable_unc_path;

/// Default pwsh command timeout (2 minutes) — matches Bash default.
const DEFAULT_TIMEOUT_MS: u64 = 120_000;

/// Max pwsh command timeout (10 minutes) — matches Bash max.
const MAX_TIMEOUT_MS: u64 = 600_000;

/// Model-facing PowerShell tool description. Runtime interpolations
/// are resolved to coco-rs constants:
/// - edition: detection-unresolved default — coco-rs does not resolve the
///   pwsh edition at prompt-build time, so conservative 5.1-safe guidance
///   is emitted;
/// - max timeout = 600000ms / 10 min ([`MAX_TIMEOUT_MS`]);
/// - default timeout = 120000ms / 2 min ([`DEFAULT_TIMEOUT_MS`]);
/// - max output = 30000 chars ([`PowerShellTool::max_result_size_bound`]);
/// - background-usage note and sleep guidance are included.
const POWERSHELL_PROMPT: &str = "Executes a given PowerShell command with optional timeout. Working directory persists between commands; shell state (variables, functions) does not.

IMPORTANT: This tool is for terminal operations via PowerShell: git, npm, docker, and PS cmdlets. DO NOT use it for file operations (reading, writing, editing, searching, finding files) - use the specialized tools for this instead.

PowerShell edition: unknown — assume Windows PowerShell 5.1 for compatibility
   - Do NOT use `&&`, `||`, ternary `?:`, null-coalescing `??`, or null-conditional `?.`. These are PowerShell 7+ only and parser-error on 5.1.
   - To chain commands conditionally: `A; if ($?) { B }`. Unconditionally: `A; B`.

Before executing the command, please follow these steps:

1. Directory Verification:
   - If the command will create new directories or files, first use `Get-ChildItem` (or `ls`) to verify the parent directory exists and is the correct location

2. Command Execution:
   - Always quote file paths that contain spaces with double quotes
   - Capture the output of the command.

PowerShell Syntax Notes:
   - Variables use $ prefix: $myVar = \"value\"
   - Escape character is backtick (`), not backslash
   - Use Verb-Noun cmdlet naming: Get-ChildItem, Set-Location, New-Item, Remove-Item
   - Common aliases: ls (Get-ChildItem), cd (Set-Location), cat (Get-Content), rm (Remove-Item)
   - Pipe operator | works similarly to bash but passes objects, not text
   - Use Select-Object, Where-Object, ForEach-Object for filtering and transformation
   - String interpolation: \"Hello $name\" or \"Hello $($obj.Property)\"
   - Registry access uses PSDrive prefixes: `HKLM:\\SOFTWARE\\...`, `HKCU:\\...` — NOT raw `HKEY_LOCAL_MACHINE\\...`
   - Environment variables: read with `$env:NAME`, set with `$env:NAME = \"value\"` (NOT `Set-Variable` or bash `export`)
   - Call native exe with spaces in path via call operator: `& \"C:\\Program Files\\App\\app.exe\" arg1 arg2`

Interactive and blocking commands (will hang — this tool runs with -NonInteractive):
   - NEVER use `Read-Host`, `Get-Credential`, `Out-GridView`, `$Host.UI.PromptForChoice`, or `pause`
   - Destructive cmdlets (`Remove-Item`, `Stop-Process`, `Clear-Content`, etc.) may prompt for confirmation. Add `-Confirm:$false` when you intend the action to proceed. Use `-Force` for read-only/hidden items.
   - Never use `git rebase -i`, `git add -i`, or other commands that open an interactive editor

Passing multiline strings (commit messages, file content) to native executables:
   - Use a single-quoted here-string so PowerShell does not expand `$` or backticks inside. The closing `'@` MUST be at column 0 (no leading whitespace) on its own line — indenting it is a parse error:
<example>
git commit -m @'
Commit message here.
Second line with $literal dollar signs.
'@
</example>
   - Use `@'...'@` (single-quoted, literal) not `@\"...\"@` (double-quoted, interpolated) unless you need variable expansion
   - For arguments containing `-`, `@`, or other characters PowerShell parses as operators, use the stop-parsing token: `git log --% --format=%H`

Usage notes:
  - The command argument is required.
  - You can specify an optional timeout in milliseconds (up to 600000ms / 10 minutes). If not specified, commands will timeout after 120000ms (2 minutes).
  - It is very helpful if you write a clear, concise description of what this command does.
  - If the output exceeds 30000 characters, output will be truncated before being returned to you.
  - You can use the `run_in_background` parameter to run the command in the background. Only use this if you don't need the result immediately and are OK being notified when the command completes later. You do not need to check the output right away - you'll be notified when it finishes.
  - Avoid using PowerShell to run commands that have dedicated tools, unless explicitly instructed:
    - File search: Use Glob (NOT Get-ChildItem -Recurse)
    - Content search: Use Grep (NOT Select-String)
    - Read files: Use Read (NOT Get-Content)
    - Edit files: Use Edit
    - Write files: Use Write (NOT Set-Content/Out-File)
    - Communication: Output text directly (NOT Write-Output/Write-Host)
  - When issuing multiple commands:
    - If the commands are independent and can run in parallel, make multiple PowerShell tool calls in a single message.
    - If the commands depend on each other and must run sequentially, chain them in a single PowerShell call (see edition-specific chaining syntax above).
    - Use `;` only when you need to run commands sequentially but don't care if earlier commands fail.
    - DO NOT use newlines to separate commands (newlines are ok in quoted strings and here-strings)
  - Do NOT prefix commands with `cd` or `Set-Location` -- the working directory is already set to the correct project directory automatically.
  - Avoid unnecessary `Start-Sleep` commands:
    - Do not sleep between commands that can run immediately — just run them.
    - If your command is long running and you would like to be notified when it finishes — simply run your command using `run_in_background`. There is no need to sleep in this case.
    - Do not retry failing commands in a sleep loop — diagnose the root cause or consider an alternative approach.
    - If waiting for a background task you started with `run_in_background`, you will be notified when it completes — do not poll.
    - If you must poll an external process, use a check command rather than sleeping first.
    - If you must sleep, keep the duration short (1-5 seconds) to avoid blocking the user.
  - For git commands:
    - Prefer to create a new commit rather than amending an existing commit.
    - Before running destructive operations (e.g., git reset --hard, git push --force, git checkout --), consider whether there is a safer alternative that achieves the same goal. Only use destructive operations when they are truly the best approach.
    - Never skip hooks (--no-verify) or bypass signing (--no-gpg-sign, -c commit.gpgsign=false) unless the user has explicitly asked for it. If a hook fails, investigate and fix the underlying issue.";

/// Typed input for [`PowerShellTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct PowerShellInput {
    /// The PowerShell command to execute
    ///
    /// `command` is the only non-optional field, so it must be plain (no
    /// `#[serde(default)]`) to stay in the derived `required` array.
    pub command: String,
    /// Optional timeout in milliseconds. Defaults to 120000 (2 min)
    /// and cannot exceed 600000 (10 min).
    #[serde(default)]
    pub timeout: Option<u64>,
    /// Set to true to run this command in the background. Returns
    /// immediately with a task_id.
    #[serde(default)]
    pub run_in_background: bool,
    /// Set this to true to dangerously override sandbox mode and run
    /// commands without sandboxing.
    ///
    /// Wire-format name is `dangerouslyDisableSandbox` (camelCase) to match
    /// BashTool's opt-out field.
    #[serde(default, rename = "dangerouslyDisableSandbox")]
    pub dangerously_disable_sandbox: bool,
    /// Optional human-readable description for the background-task UI.
    #[serde(default)]
    pub description: Option<String>,
}

pub struct PowerShellTool;

fn resolve_powershell_provider(
    ctx: &ToolUseContext,
) -> Result<Arc<dyn coco_shell::ShellProvider>, ToolError> {
    if let Some(provider) = ctx.shell_provider.clone() {
        if matches!(provider.shell_type(), coco_shell::ShellType::PowerShell) {
            return Ok(provider);
        }
        return Err(ToolError::ExecutionFailed {
            message: format!(
                "PowerShell tool selected, but the session shell provider is {}.",
                provider.shell_type().name()
            ),
            display_data: None,
            source: None,
        });
    }

    let shell = coco_shell::get_shell(coco_shell::ShellType::PowerShell, None).ok_or(
        ToolError::ExecutionFailed {
            message:
                "PowerShell not found on PATH. Install `pwsh` or `powershell` to use this tool."
                    .into(),
            display_data: None,
            source: None,
        },
    )?;
    Ok(Arc::new(coco_shell::PowerShellProvider::from_shell(shell)))
}

#[async_trait::async_trait]
impl Tool for PowerShellTool {
    type Input = PowerShellInput;
    coco_tool_runtime::impl_runtime_schema!(PowerShellInput);
    /// Output is `Value` because the wire shape is a tagged union of
    /// fg / bg envelopes (`{stdout, stderr, exitCode, interrupted}` vs
    /// `{backgroundTaskId, outputPath}` plus optional
    /// `assistantAutoBackgrounded` / `backgroundedByUser` discriminants).
    /// Modeling as a tagged enum would require a `Bash`-style refactor of
    /// the renderer; deferred to a follow-up pass.
    type Output = Value;

    fn to_auto_classifier_input(&self, input: &PowerShellInput) -> Option<String> {
        Some(input.command.clone())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::PowerShell)
    }

    fn name(&self) -> &str {
        ToolName::PowerShell.as_str()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.active_shell_tool == coco_types::ActiveShellTool::PowerShell
    }

    /// Short UI label: the caller-supplied `description` field, falling back
    /// to `'Run PowerShell command'`. The long model-facing guidance lives in
    /// [`Self::prompt`].
    fn description(&self, input: &PowerShellInput, _options: &DescriptionOptions) -> String {
        match input.description.as_deref() {
            Some(d) if !d.is_empty() => d.to_string(),
            _ => "Run PowerShell command".into(),
        }
    }
    /// Model-facing tool description.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        POWERSHELL_PROMPT.into()
    }

    /// Model-facing spec. Mirrors [`BashTool`]: when background tasks are
    /// disabled (`COCO_BACKGROUND_TASKS_DISABLE`), drop `run_in_background`
    /// from the schema so the model never sets a field whose background
    /// execution path (`execute_background`) would then fail for lack of a
    /// task handle. (PowerShell has no internal-only field to always omit, so
    /// the enabled case keeps the full schema.)
    async fn tool_spec(
        &self,
        ctx: &coco_tool_runtime::SchemaContext,
        prompt_opts: &coco_tool_runtime::PromptOptions,
    ) -> coco_tool_runtime::ToolSpec {
        let omit: &[&str] = if ctx.background_tasks_disabled {
            &["run_in_background"]
        } else {
            &[]
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

    /// Mirror Bash's read-only fast path. A command classified as search/read
    /// is concurrency-safe and skips the user-approval flow upstream.
    fn is_read_only(&self, input: &PowerShellInput) -> bool {
        let (is_search, is_read) = classify_ps_command(&input.command);
        is_search || is_read
    }

    fn is_concurrency_safe(&self, input: &PowerShellInput) -> bool {
        Tool::is_read_only(self, input)
    }

    fn is_destructive(&self, input: &PowerShellInput) -> bool {
        !Tool::is_read_only(self, input)
    }

    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("execute Windows PowerShell commands")
    }

    fn get_activity_description(&self, input: &PowerShellInput) -> Option<String> {
        if input.command.is_empty() {
            return None;
        }
        let truncated: String = input.command.chars().take(57).collect();
        Some(if truncated.len() < input.command.len() {
            format!("Running pwsh {truncated}...")
        } else {
            format!("Running pwsh {command}", command = input.command)
        })
    }

    /// `maxResultSizeChars: 30_000`.
    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        coco_tool_runtime::ResultSizeBound::Chars(30_000)
    }

    fn validate_input(&self, input: &PowerShellInput, _ctx: &ToolUseContext) -> ValidationResult {
        if input.command.is_empty() {
            return ValidationResult::invalid("missing required field: command");
        }
        if let Some(timeout) = input.timeout
            && timeout > MAX_TIMEOUT_MS
        {
            return ValidationResult::invalid(format!(
                "timeout must not exceed {MAX_TIMEOUT_MS}ms"
            ));
        }
        ValidationResult::Valid
    }

    /// Render the PowerShell envelope. Mirrors Bash's text-path render:
    /// build `[processedStdout, errorMessage, backgroundInfo]` joined with
    /// `\n`, skipping empties. `processedStdout` strips leading blank lines +
    /// trims trailing whitespace. Oversized text output is persisted by the
    /// query-level generic Level 1 tool-result pipeline. A backgrounded
    /// command carries `backgroundTaskId` + `outputPath`, so the
    /// `backgroundInfo` line names both — the model `Read`s that path.
    ///
    /// The `isImage` branch is intentionally unimplemented because
    /// `execute_foreground` decodes UTF-16 stdout into UTF-8 before the
    /// data envelope is built — image bytes would be mangled by that decode.
    /// Wire image detection into the execute path (emit `structuredContent`)
    /// before adding the render branch.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let stdout = data.get("stdout").and_then(Value::as_str).unwrap_or("");
        let stderr = data.get("stderr").and_then(Value::as_str).unwrap_or("");
        let interrupted = data
            .get("interrupted")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let processed = super::shell_render::strip_leading_blank_lines(stdout)
            .trim_end()
            .to_string();
        let mut error_message = stderr.trim().to_string();
        if interrupted {
            if !error_message.is_empty() {
                error_message.push('\n');
            }
            error_message.push_str("<error>Command was aborted before completion</error>");
        }

        let background_info = data
            .get("backgroundTaskId")
            .and_then(Value::as_str)
            .map(|task_id| {
                let output_path = data.get("outputPath").and_then(Value::as_str).unwrap_or("");
                format_background_notice(BackgroundKind::from_result(data), task_id, output_path)
            })
            .unwrap_or_default();

        let combined = [
            processed.as_str(),
            error_message.as_str(),
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

    async fn execute(
        &self,
        input: PowerShellInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        if input.command.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "missing command".into(),
                error_code: None,
            });
        }

        // ── Stage 1: CLM type + git-internal guard ──
        //
        // Runs before the child is spawned; commands using .NET types outside
        // the CLM allowlist or writing to `.git/{hooks,refs,objects,HEAD,config}`
        // are rejected hard. Read-only commands skip this gate because a harmless
        // `Get-Content ./x` must not be blocked because it mentions
        // `[IO.File]::ReadAllText(...)` in a string literal.
        let (is_search, is_read) = classify_ps_command(&input.command);
        if !(is_search || is_read) {
            let result = analyze_ps_security(&input.command);
            if !result.is_safe {
                let reason = result
                    .reason
                    .unwrap_or_else(|| "PowerShell security check failed".into());
                return Err(ToolError::PermissionDenied {
                    message: format!(
                        "Command blocked by coco-rs PowerShell security gate: {reason}. \
                         If you believe this is a false positive, restructure the command."
                    ),
                });
            }
        }

        // ── Stage 2: UNC path guard ──
        //
        // UNC paths (`\\server\share\...`) in command arguments can be used
        // for NTLM credential leakage. Rejects any non-whitelisted UNC path
        // before execution.
        for token in input.command.split_ascii_whitespace() {
            if is_vulnerable_unc_path(token) {
                return Err(ToolError::PermissionDenied {
                    message: format!(
                        "Command contains UNC path `{token}` which can leak NTLM \
                         credentials. Reject per PowerShell path validation."
                    ),
                });
            }
        }

        // Sandbox decision parity with Bash. Resolve the active state +
        // bypass flag here — BEFORE the background branch — so the foreground
        // AND background paths apply the same platform wrap. #38: backgrounded
        // pwsh is sandboxed identically to foreground.
        let sandbox_state = if ctx.features.enabled(coco_types::Feature::Sandbox) {
            ctx.sandbox_state.clone()
        } else {
            None
        };
        let sandbox_bypass = SandboxBypass::from_flag(input.dangerously_disable_sandbox);

        if input.run_in_background {
            return execute_background(&input, ctx, sandbox_state, sandbox_bypass).await;
        }

        execute_foreground(&input, ctx, sandbox_state, sandbox_bypass).await
    }
}

/// Spawn the command as a background task via `task_handle`.
async fn execute_background(
    input: &PowerShellInput,
    ctx: &ToolUseContext,
    sandbox_state: Option<std::sync::Arc<coco_sandbox::SandboxState>>,
    sandbox_bypass: SandboxBypass,
) -> Result<ToolResult<Value>, ToolError> {
    let task_handle = ctx
        .task_handle
        .as_ref()
        .ok_or_else(|| ToolError::ExecutionFailed {
            message: "Background task execution is not available in this context.".into(),
            display_data: None,
            source: None,
        })?;

    let provider = resolve_powershell_provider(ctx)?;
    let description = input
        .description
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| "PowerShell background task".into());
    let task_id = task_handle
        .spawn_shell_task(coco_tool_runtime::BackgroundShellRequest {
            command: input.command.clone(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::Provider(provider),
            timeout_ms: input.timeout.map(|t| t as i64),
            description,
            tool_use_id: ctx.tool_use_id.clone(),
            issuing_agent: ctx.agent_id.as_ref().map(ToString::to_string),
            // W3: bg-only spawn — no progress emission, no auto-detach.
            // The fg PowerShell path still goes through ShellExecutor;
            // unifying it would mirror the BashTool W3 refactor and is
            // a follow-up.
            progress_tx: None,
            progress_throttle_ms: 1000,
            auto_detach_ms: None,
            // Explicit bg spawn keeps the hard-kill-on-timeout behaviour.
            kill_on_timeout: true,
            // #38: thread the resolved sandbox state/bypass so backgrounded
            // pwsh is sandboxed identically to the foreground path.
            sandbox_state,
            sandbox_bypass,
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to spawn background task: {e}"),
            display_data: None,
            source: None,
        })?;

    let output_path = background_output_path(task_handle, &task_id).await;
    Ok(ToolResult {
        data: serde_json::json!({
            "backgroundTaskId": task_id,
            "outputPath": output_path,
        }),
        new_messages: vec![],
        app_state_patch: None,
        permission_updates: Vec::new(),
        display_data: None,
    })
}

/// Foreground execution with UTF-16 output decode and optional sandbox wrap.
///
/// Goes through [`coco_shell::ShellExecutor`] + [`coco_shell::PowerShellProvider`]
/// so cancel-token, timeout, sandbox-wrap, and CWD tracking all behave
/// the same way they do for `BashTool`. The provider's `build_exec_command`
/// emits `-EncodedCommand <base64-utf16le>` when sandboxed (to dodge the
/// sandbox-runtime's shellquote layer corrupting `!`/`$`/`?`); we get
/// `stdout_bytes` / `stderr_bytes` back so [`decode_ps_output`] can
/// recover the original UTF-16 BOM-prefixed text without going through
/// the lossy String conversion the executor applies for display.
async fn execute_foreground(
    input: &PowerShellInput,
    ctx: &ToolUseContext,
    sandbox_state: Option<Arc<SandboxState>>,
    sandbox_bypass: SandboxBypass,
) -> Result<ToolResult<Value>, ToolError> {
    let command = input.command.as_str();
    let timeout_ms = input
        .timeout
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .min(MAX_TIMEOUT_MS);

    // 4-tier cwd resolution. Spawn at live session cwd; reset guard
    // runs AFTER exec — annotation lands on the offending command's stderr.
    let cwd = crate::tools::shell_cwd::resolve_spawn_cwd(ctx).await;

    let provider = resolve_powershell_provider(ctx)?;
    let mut executor = coco_shell::ShellExecutor::with_provider(&cwd, provider);

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

    let cmd_result =
        executor
            .execute(command, &opts)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("PowerShell execution failed: {e}"),
                display_data: None,
                source: None,
            })?;

    if cmd_result.timed_out {
        return Err(ToolError::Timeout {
            timeout_ms: timeout_ms as i64,
        });
    }

    // setCwd(new_cwd) → resetCwdIfOutsideProject. `Set-Location C:\foo` in
    // turn N persists into turn N+1; if it drifted outside the allowed set,
    // session_cwd snaps back to original and we annotate stderr.
    let reset_message =
        crate::tools::shell_cwd::finalize_cwd_post_exec(ctx, cmd_result.new_cwd.clone()).await;

    // Decode UTF-16 BOM-prefixed output. Falls through to UTF-8 lossy
    // when the byte buffer is plain UTF-8.
    let stdout = cmd_result
        .stdout_bytes
        .as_deref()
        .map(decode_ps_output)
        .unwrap_or_else(|| cmd_result.stdout.clone());
    // Strip + record any `<claude-code-hint />` tags (both foreground and
    // background, same as Bash).
    let stdout = crate::tools::bash::maybe_strip_and_record_hints(stdout, command);
    let mut stderr = cmd_result
        .stderr_bytes
        .as_deref()
        .map(decode_ps_output)
        .unwrap_or_else(|| cmd_result.stderr.clone());

    crate::tools::shell_cwd::annotate_stderr_with_reset(&mut stderr, reset_message);

    if let (Some(state), Some(prev)) = (&sandbox_state, violations_baseline)
        && let Some(annotation) = state.format_violations_since(prev).await
    {
        if stderr.is_empty() {
            stderr = annotation;
        } else {
            stderr.push('\n');
            stderr.push_str(&annotation);
        }
    }

    Ok(ToolResult {
        data: serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exitCode": cmd_result.exit_code,
            "interrupted": cmd_result.interrupted,
        }),
        new_messages: vec![],
        app_state_patch: None,
        permission_updates: Vec::new(),
        display_data: None,
    })
}

#[cfg(test)]
#[path = "powershell_tool.test.rs"]
mod tests;
