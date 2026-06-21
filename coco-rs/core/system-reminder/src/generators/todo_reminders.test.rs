use super::*;
use crate::generator::GeneratorContext;
use crate::types::AttachmentType;
use coco_config::SystemReminderConfig;
use coco_types::TodoRecord;
use pretty_assertions::assert_eq;

fn cfg() -> SystemReminderConfig {
    SystemReminderConfig::default()
}

fn todo(content: &str, status: &str) -> TodoRecord {
    TodoRecord {
        content: content.to_string(),
        status: status.to_string(),
        active_form: format!("Working on {content}"),
    }
}

fn ctx_with_tools<'a>(c: &'a SystemReminderConfig, tools: Vec<&str>) -> GeneratorContext<'a> {
    GeneratorContext::builder(c)
        .tools(tools.into_iter().map(String::from).collect())
        .turns_since_last_todo_write(10)
        .turns_since_last_todo_reminder(10)
        .build()
}

// ── Gate: tool presence ──

#[tokio::test]
async fn skips_when_todowrite_absent() {
    let c = cfg();
    let ctx = ctx_with_tools(&c, vec!["Read", "Write"]);
    assert!(
        TodoRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_brief_tool_present() {
    let c = cfg();
    let ctx = ctx_with_tools(
        &c,
        vec!["TodoWrite", coco_types::ToolName::SendUserMessage.as_str()],
    );
    assert!(
        TodoRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn skips_when_v2_enabled() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(vec!["TodoWrite".to_string()])
        .turns_since_last_todo_write(10)
        .turns_since_last_todo_reminder(10)
        .is_task_v2_enabled(true)
        .build();
    assert!(
        TodoRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

// ── Gate: turn thresholds ──

#[tokio::test]
async fn skips_when_write_not_stale_enough() {
    let c = cfg();
    let ctx = GeneratorContext::builder(&c)
        .tools(vec!["TodoWrite".to_string()])
        .turns_since_last_todo_write(9) // below threshold
        .turns_since_last_todo_reminder(100)
        .build();
    assert!(
        TodoRemindersGenerator
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
        .tools(vec!["TodoWrite".to_string()])
        .turns_since_last_todo_write(100)
        .turns_since_last_todo_reminder(9) // below threshold
        .build();
    assert!(
        TodoRemindersGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn emits_when_both_thresholds_exactly_met() {
    let c = cfg();
    let ctx = ctx_with_tools(&c, vec!["TodoWrite"]);
    let r = TodoRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits at exactly 10/10");
    assert_eq!(r.attachment_type, AttachmentType::TodoReminder);
}

// ── Content format ──

#[tokio::test]
async fn body_matches_ts_string_exactly() {
    let c = cfg();
    let ctx = ctx_with_tools(&c, vec!["TodoWrite"]);
    let r = TodoRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let text = r.content().unwrap();
    // Assert a few distinctive markers.
    assert!(text.contains("The TodoWrite tool hasn't been used recently."));
    assert!(text.contains("NEVER mention this reminder to the user"));
}

#[tokio::test]
async fn empty_todos_omits_bracket_list() {
    let c = cfg();
    let ctx = ctx_with_tools(&c, vec!["TodoWrite"]);
    let r = TodoRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let text = r.content().unwrap();
    assert!(
        !text.contains("Here are the existing contents of your todo list"),
        "empty list → no suffix: {text}"
    );
}

#[tokio::test]
async fn non_empty_todos_append_bracket_list() {
    let c = cfg();
    let todos = vec![
        todo("Finish auth", "in_progress"),
        todo("Write tests", "pending"),
    ];
    let ctx = GeneratorContext::builder(&c)
        .tools(vec!["TodoWrite".to_string()])
        .turns_since_last_todo_write(10)
        .turns_since_last_todo_reminder(10)
        .todos(todos)
        .build();
    let r = TodoRemindersGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let text = r.content().unwrap();
    // Format: "[1. [in_progress] Finish auth\n2. [pending] Write tests]"
    assert!(text.contains("[1. [in_progress] Finish auth"));
    assert!(text.contains("2. [pending] Write tests]"));
    assert!(text.ends_with(']'));
}

// ── Config gate ──

#[tokio::test]
async fn respects_config_flag() {
    let mut c = cfg();
    c.attachments.todo_reminder = false;
    assert!(!TodoRemindersGenerator.is_enabled(&c));
}

// ── Trailing-newline ──

#[test]
fn empty_body_has_trailing_newline() {
    // Base body always ends with `\n` even when the list is empty.
    assert_eq!(
        render_todo_reminder_body(&[]),
        format!("{TODO_REMINDER_BODY}\n")
    );
}

#[test]
fn nonempty_body_has_three_newlines_before_list() {
    let body = render_todo_reminder_body(&[todo("a", "pending")]);
    assert!(
        body.contains("the user\n\n\nHere are the existing contents of your todo list:\n\n["),
        "expected 3 newlines before the list, got: {body:?}"
    );
}
