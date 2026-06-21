use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;
use coco_types::TaskListStatus;
use coco_types::TaskRecord;
use coco_types::ToolName;
use pretty_assertions::assert_eq;

fn cfg() -> SystemReminderConfig {
    SystemReminderConfig::default()
}

/// Default "tools" list used by the emit-path tests.
/// TaskUpdate must be present, Brief must be absent.
fn tools_with_task_update() -> Vec<String> {
    vec![ToolName::TaskUpdate.as_str().to_string()]
}

fn task(id: &str, subject: &str, status: TaskListStatus) -> TaskRecord {
    TaskRecord {
        id: id.to_string(),
        subject: subject.to_string(),
        description: String::new(),
        active_form: None,
        owner: None,
        status,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: Default::default(),
    }
}

#[tokio::test]
async fn skips_when_v2_disabled() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(tools_with_task_update())
        .is_task_v2_enabled(false)
        .turns_since_last_task_tool(100)
        .turns_since_last_task_reminder(100)
        .build();
    assert!(
        TaskRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_brief_tool_present() {
    // Brief tool is the primary I/O channel → suppress the nag.
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(vec![
            ToolName::TaskUpdate.as_str().to_string(),
            ToolName::SendUserMessage.as_str().to_string(),
        ])
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(100)
        .turns_since_last_task_reminder(100)
        .build();
    assert!(
        TaskRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_task_update_tool_absent() {
    // `TASK_UPDATE_TOOL_NAME` must be in the registry.
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(vec![ToolName::Read.as_str().to_string()])
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(100)
        .turns_since_last_task_reminder(100)
        .build();
    assert!(
        TaskRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_task_tool_too_recent() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(tools_with_task_update())
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(9)
        .turns_since_last_task_reminder(100)
        .build();
    assert!(
        TaskRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_reminder_too_recent() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(tools_with_task_update())
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(100)
        .turns_since_last_task_reminder(9)
        .build();
    assert!(
        TaskRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_when_thresholds_met_and_v2_enabled() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(tools_with_task_update())
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(10)
        .turns_since_last_task_reminder(10)
        .build();
    let r = TaskRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    assert_eq!(r.attachment_type, AttachmentType::TaskReminder);
}

#[tokio::test]
async fn body_matches_ts_string_markers() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(tools_with_task_update())
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(10)
        .turns_since_last_task_reminder(10)
        .build();
    let r = TaskRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let text = r.content().unwrap();
    assert!(text.contains("The task tools haven't been used recently."));
    assert!(text.contains("consider using TaskCreate to add new tasks"));
    assert!(text.contains("NEVER mention this reminder to the user"));
}

#[tokio::test]
async fn empty_tasks_omits_list_suffix() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(tools_with_task_update())
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(10)
        .turns_since_last_task_reminder(10)
        .build();
    let r = TaskRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let text = r.content().unwrap();
    assert!(
        !text.contains("Here are the existing tasks"),
        "empty list → no suffix: {text}"
    );
}

#[tokio::test]
async fn non_empty_tasks_format_matches_ts_id_status_subject() {
    let c = cfg();
    let tasks = vec![
        task("42", "Fix auth", TaskListStatus::InProgress),
        task("43", "Write tests", TaskListStatus::Pending),
    ];
    let ctx = GeneratorContext::builder(&c)
        .tools(tools_with_task_update())
        .is_task_v2_enabled(true)
        .turns_since_last_task_tool(10)
        .turns_since_last_task_reminder(10)
        .plan_tasks(tasks)
        .build();
    let r = TaskRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let text = r.content().unwrap();
    // Format: "#${id}. [${status}] ${subject}" joined by newlines.
    assert!(text.contains("#42. [in_progress] Fix auth"));
    assert!(text.contains("#43. [pending] Write tests"));
}

#[tokio::test]
async fn respects_config_flag() {
    let mut c = cfg();
    c.attachments.task_reminder = false;
    assert!(!TaskRemindersGenerator.is_enabled(&c));
}

// ── Trailing-newline ──

#[test]
fn empty_body_has_trailing_newline() {
    assert_eq!(
        render_task_reminder_body(&[]),
        format!("{}\n", task_reminder_body())
    );
}

#[test]
fn nonempty_body_has_three_newlines_before_list() {
    let body = render_task_reminder_body(&[task("1", "x", TaskListStatus::Pending)]);
    assert!(
        body.contains("the user\n\n\nHere are the existing tasks:\n\n"),
        "expected 3 newlines before the list, got: {body:?}"
    );
}
