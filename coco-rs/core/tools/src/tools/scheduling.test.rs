use super::CronCreateTool;
use super::CronListTool;
use super::ScheduleWakeupTool;
use super::is_valid_cron_expression;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::InMemoryScheduleStore;
use coco_tool_runtime::ScheduleStore;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use serde_json::json;
use std::sync::Arc;

// ── R7-T22: cron expression validation tests ──
//
// `validateInput` validates the cron expression so the model gets a clear
// error before the schedule store rejects. coco-rs implements a lightweight
// 5-field validator inline; these tests cover the grammar.

#[test]
fn test_cron_validator_accepts_basic_expressions() {
    assert!(is_valid_cron_expression("* * * * *"));
    assert!(is_valid_cron_expression("0 9 * * 1-5"));
    assert!(is_valid_cron_expression("*/5 * * * *"));
    assert!(is_valid_cron_expression("30 14 28 2 *"));
    assert!(is_valid_cron_expression("0 0,6,12,18 * * *"));
    assert!(is_valid_cron_expression("0-30 * * * *"));
}

#[test]
fn test_cron_validator_rejects_wrong_field_count() {
    assert!(!is_valid_cron_expression("* * * *")); // 4 fields
    assert!(!is_valid_cron_expression("* * * * * *")); // 6 fields
    assert!(!is_valid_cron_expression("")); // 0 fields
    assert!(!is_valid_cron_expression("hello world"));
}

#[test]
fn test_cron_validator_rejects_invalid_atoms() {
    assert!(!is_valid_cron_expression("* * * * abc"));
    assert!(!is_valid_cron_expression("*/abc * * * *"));
    assert!(!is_valid_cron_expression("5-2 * * * *")); // descending range
    assert!(!is_valid_cron_expression("* * * * /5")); // step with no base
}

#[test]
fn test_cron_create_validate_input_rejects_invalid_cron() {
    let ctx = ToolUseContext::test_default();
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"cron": "not a cron", "prompt": "do thing"}),
        &ctx,
    );
    match result {
        ValidationResult::Invalid { message, .. } => {
            assert!(
                message.contains("Invalid cron expression"),
                "expected invalid-cron error, got: {message}"
            );
        }
        _ => panic!("expected Invalid result for malformed cron"),
    }
}

#[test]
fn test_cron_create_validate_input_requires_cron_and_prompt() {
    let ctx = ToolUseContext::test_default();
    // Empty cron.
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"prompt": "do thing"}),
        &ctx,
    );
    assert!(matches!(result, ValidationResult::Invalid { .. }));
    // Empty prompt.
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"cron": "* * * * *"}),
        &ctx,
    );
    assert!(matches!(result, ValidationResult::Invalid { .. }));
}

#[test]
fn test_cron_create_validate_input_accepts_valid() {
    let ctx = ToolUseContext::test_default();
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"cron": "*/15 * * * *", "prompt": "ping"}),
        &ctx,
    );
    assert!(matches!(result, ValidationResult::Valid));
}

#[test]
fn test_cron_create_validate_input_rejects_unreachable() {
    // tools-web-mcp#54: syntactically valid but never fires (Feb 30) — the
    // coco-cron reachability check (errorCode 2) must reject before execute.
    let ctx = ToolUseContext::test_default();
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"cron": "30 14 30 2 *", "prompt": "ping"}),
        &ctx,
    );
    match result {
        ValidationResult::Invalid { message, .. } => {
            assert!(message.contains("no scheduled run"), "got: {message}");
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

// ── tools-web-mcp#60: durable-cron teammate guard ──

#[test]
fn test_cron_create_rejects_durable_for_in_process_teammate() {
    let mut ctx = ToolUseContext::test_default();
    ctx.is_in_process_teammate = true;
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"cron": "*/15 * * * *", "prompt": "ping", "durable": true}),
        &ctx,
    );
    match result {
        ValidationResult::Invalid { message, .. } => {
            assert!(
                message.contains("durable crons are not supported for teammates"),
                "got: {message}"
            );
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn test_cron_create_allows_durable_for_non_teammate() {
    // Main session / regular subagent (agent_id may be set, but not a teammate).
    let ctx = ToolUseContext::test_default();
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"cron": "*/15 * * * *", "prompt": "ping", "durable": true}),
        &ctx,
    );
    assert!(matches!(result, ValidationResult::Valid));
}

#[test]
fn test_cron_create_allows_session_cron_for_teammate() {
    let mut ctx = ToolUseContext::test_default();
    ctx.is_in_process_teammate = true;
    let result = <CronCreateTool as DynTool>::validate_input(
        &CronCreateTool,
        &json!({"cron": "*/15 * * * *", "prompt": "ping", "durable": false}),
        &ctx,
    );
    assert!(matches!(result, ValidationResult::Valid));
}

// ── R7-T21: CronList output shape tests ──

#[tokio::test]
async fn test_cron_list_returns_jobs_wrapper_when_empty() {
    let ctx = ToolUseContext::test_default();
    // The default test context's NoOpScheduleStore returns an empty list.
    let result = <CronListTool as DynTool>::execute(&CronListTool, json!({}), &ctx)
        .await
        .unwrap();
    // Output shape: `{ jobs: [] }`. Not a bare array, not a string.
    assert!(
        result.data["jobs"].is_array(),
        "CronList output must be wrapped as {{ jobs: [...] }}, got: {:?}",
        result.data
    );
    assert_eq!(result.data["jobs"].as_array().unwrap().len(), 0);
}

// ── render_for_model — cron tool envelopes ──────────────

#[test]
fn cron_create_render_recurring_durable() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({
        "id": "abc-123",
        "humanSchedule": "every Monday at 09:00",
        "recurring": true,
        "durable": true,
        "status": "created",
    });
    let parts = <CronCreateTool as DynTool>::render_for_model(&CronCreateTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    // Durable recurring: now actually scheduled + persisted (firing wired).
    assert!(
        text.starts_with("Scheduled recurring job abc-123"),
        "got: {text}"
    );
    assert!(text.contains("(every Monday at 09:00)"), "got: {text}");
    assert!(
        text.contains(&format!(
            "persisted to {}/scheduled_tasks.json",
            coco_utils_common::COCO_CONFIG_DIR_NAME
        )),
        "got: {text}"
    );
    assert!(text.contains("Auto-expires after 7 days"), "got: {text}");
}

#[test]
fn cron_create_render_one_shot_session_only() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({
        "id": "x",
        "humanSchedule": "Feb 28 14:30",
        "recurring": false,
        "durable": false,
        "status": "created",
    });
    let parts = <CronCreateTool as DynTool>::render_for_model(&CronCreateTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.starts_with("Scheduled one-shot task x"), "got: {text}");
    assert!(text.contains("session-only"), "got: {text}");
    assert!(text.contains("fires once"), "got: {text}");
}

#[tokio::test]
async fn schedule_wakeup_creates_session_only_one_shot() {
    let store = Arc::new(InMemoryScheduleStore::new());
    let mut ctx = ToolUseContext::test_default();
    ctx.schedules = store.clone();

    let result = <ScheduleWakeupTool as DynTool>::execute(
        &ScheduleWakeupTool,
        json!({
            "delaySeconds": 30,
            "reason": "watching CI",
            "prompt": "/loop check CI"
        }),
        &ctx,
    )
    .await
    .unwrap();

    assert_eq!(result.data["clampedDelaySeconds"], 60);
    assert_eq!(result.data["wasClamped"], true);
    assert!(result.data["scheduledFor"].as_i64().unwrap() >= now_ms() + 60_000);
    let tasks = store.list_all_cron_tasks().await.unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].prompt, "/loop check CI");
    assert_eq!(tasks[0].recurring, None);
    assert_eq!(tasks[0].durable, Some(false));
}

#[test]
fn schedule_wakeup_validate_input_requires_prompt_and_reason() {
    let ctx = ToolUseContext::test_default();
    let missing_prompt = <ScheduleWakeupTool as DynTool>::validate_input(
        &ScheduleWakeupTool,
        &json!({"delaySeconds": 120, "reason": "idle", "prompt": ""}),
        &ctx,
    );
    assert!(matches!(missing_prompt, ValidationResult::Invalid { .. }));

    let missing_reason = <ScheduleWakeupTool as DynTool>::validate_input(
        &ScheduleWakeupTool,
        &json!({"delaySeconds": 120, "reason": "", "prompt": "/loop check"}),
        &ctx,
    );
    assert!(matches!(missing_reason, ValidationResult::Invalid { .. }));
}

#[test]
fn schedule_wakeup_render_reports_clamp() {
    use coco_tool_runtime::ToolResultContentPart;
    let parts = <ScheduleWakeupTool as DynTool>::render_for_model(
        &ScheduleWakeupTool,
        &json!({
            "scheduledFor": now_ms() + 60_000,
            "clampedDelaySeconds": 60,
            "wasClamped": true,
            "id": "wake-1"
        }),
    );
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.contains("Next wakeup scheduled"), "got: {text}");
    assert!(text.contains("clamped to 60s"), "got: {text}");
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[test]
fn cron_delete_render_uses_cancelled_verb() {
    use super::CronDeleteTool;
    use coco_tool_runtime::ToolResultContentPart;
    // Render format: `Cancelled job ${id}.`.
    let data = json!({"id": "job-42"});
    let parts = <CronDeleteTool as DynTool>::render_for_model(&CronDeleteTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "Cancelled job job-42.");
}

#[test]
fn cron_list_render_empty_branch() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({"jobs": []});
    let parts = <CronListTool as DynTool>::render_for_model(&CronListTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "No scheduled tasks.");
}

#[test]
fn cron_list_render_summarizes_jobs() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({
        "jobs": [
            {"id": "job-1", "humanSchedule": "every 5 min", "prompt": "ping"},
            {"id": "job-2", "humanSchedule": "Monday 9am", "prompt": "weekly review"},
        ]
    });
    let parts = <CronListTool as DynTool>::render_for_model(&CronListTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.starts_with("2 scheduled tasks:"), "got: {text}");
    assert!(text.contains("job-1: every 5 min → ping"), "got: {text}");
    assert!(
        text.contains("job-2: Monday 9am → weekly review"),
        "got: {text}"
    );
}
