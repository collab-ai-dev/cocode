use super::*;
use crate::generator::GeneratorContext;
use crate::generator::TaskRunStatus;
use crate::generator::TaskStatusSnapshot;
use coco_config::SystemReminderConfig;

fn snap(id: &str, desc: &str, status: TaskRunStatus) -> TaskStatusSnapshot {
    TaskStatusSnapshot {
        task_id: id.into(),
        description: desc.into(),
        status,
        task_type: "local_agent".into(),
        delta_summary: None,
        output_file_path: None,
    }
}

#[tokio::test]
async fn skips_when_no_statuses() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).task_statuses(vec![]).build();
    assert!(TaskStatusGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn killed_renders_stopped_message() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![snap("42", "code review", TaskRunStatus::Killed)])
        .build();
    let text = TaskStatusGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_eq!(text, "Task \"code review\" (42) was stopped by the user.");
}

#[tokio::test]
async fn running_includes_anti_duplicate_warning() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "42".into(),
            description: "scan repo".into(),
            status: TaskRunStatus::Running,
            task_type: "local_agent".into(),
            delta_summary: Some("10/100 files".into()),
            output_file_path: Some("/tmp/task-42.log".into()),
        }])
        .build();
    let text = TaskStatusGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("Background agent \"scan repo\" (42) is still running."));
    assert!(text.contains("Progress: 10/100 files"));
    assert!(text.contains("Do NOT spawn a duplicate"));
    assert!(text.contains("/tmp/task-42.log"));
    // Running agent status includes the SendMessage tool ref so the
    // model knows it can steer the running agent.
    assert!(text.contains("send it a message with SendMessage"));
}

#[tokio::test]
async fn running_without_output_file_includes_tool_refs() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "7".into(),
            description: "lint".into(),
            status: TaskRunStatus::Running,
            task_type: "local_agent".into(),
            delta_summary: None,
            output_file_path: None,
        }])
        .build();
    let text = TaskStatusGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    // No output path → must reference both TaskOutput and SendMessage tools.
    assert!(text.contains("check its progress with the TaskOutput tool"));
    assert!(text.contains("send it a message with SendMessage"));
}

#[tokio::test]
async fn completed_includes_delta_when_set() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "x".into(),
            description: "tidy".into(),
            status: TaskRunStatus::Completed,
            task_type: "local_agent".into(),
            delta_summary: Some("removed 3 files".into()),
            output_file_path: None,
        }])
        .build();
    let text = TaskStatusGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    // Format: parts joined by space,
    // `(type: ...) (status: ...) (description: ...) Delta: ... <output-or-tool-ref>`.
    assert!(text.starts_with("Task x"));
    assert!(text.contains("(type: local_agent)"));
    assert!(text.contains("(status: completed)"));
    assert!(text.contains("(description: tidy)"));
    assert!(text.contains("Delta: removed 3 files"));
    // No output file → must reference TaskOutput tool.
    assert!(text.contains("You can check its output using the TaskOutput tool."));
}

#[tokio::test]
async fn failed_with_output_file_references_path() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "9".into(),
            description: "build".into(),
            status: TaskRunStatus::Failed,
            task_type: "local_bash".into(),
            delta_summary: Some("compile error".into()),
            output_file_path: Some("/tmp/task-9.log".into()),
        }])
        .build();
    let text = TaskStatusGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.contains("Task 9"));
    assert!(text.contains("(type: local_bash)"));
    assert!(text.contains("(status: failed)"));
    assert!(text.contains("(description: build)"));
    assert!(text.contains("Delta: compile error"));
    assert!(text.contains("Read the output file to retrieve the result: /tmp/task-9.log"));
}
