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
        killed_by: None,
        task_type: coco_types::TaskType::BgAgent,
        progress_summary: None,
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
    assert_eq!(text, "Task \"code review\" (42) was stopped.");
}

#[tokio::test]
async fn killed_renders_attribution() {
    let c = SystemReminderConfig::default();
    let mut stopped = snap("42", "code review", TaskRunStatus::Killed);
    stopped.killed_by = Some(coco_types::TaskKilledBy::Parent);
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![stopped])
        .build();
    let text = TaskStatusGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_eq!(text, "Task \"code review\" (42) was stopped by the agent.");
}

#[tokio::test]
async fn running_includes_anti_duplicate_warning() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "42".into(),
            description: "scan repo".into(),
            status: TaskRunStatus::Running,
            killed_by: None,
            task_type: coco_types::TaskType::BgAgent,
            progress_summary: Some("10/100 files".into()),
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
    // Running agent status includes the SendMessage tool ref so the
    // model knows it can steer the running agent.
    assert!(text.contains("use SendMessage only if you need to communicate"));
    assert!(!text.contains("/tmp/task-42.log"));
    assert!(!text.contains("partial log"));
    assert!(!text.contains("partial output"));
    assert!(!text.contains("Read"));
}

#[tokio::test]
async fn running_without_output_file_includes_send_message_ref() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "7".into(),
            description: "lint".into(),
            status: TaskRunStatus::Running,
            killed_by: None,
            task_type: coco_types::TaskType::BgAgent,
            progress_summary: None,
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
    assert!(text.contains("use SendMessage only if you need to communicate"));
    assert!(!text.contains("TaskOutput"));
    assert!(!text.contains("Read"));
}

#[tokio::test]
async fn running_shell_is_duplicate_warning_only() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "b1".into(),
            description: "cargo test".into(),
            status: TaskRunStatus::Running,
            killed_by: None,
            task_type: coco_types::TaskType::Shell,
            progress_summary: Some("ignored for shell".into()),
            output_file_path: Some("/tmp/task-b1.log".into()),
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
    assert!(text.contains("Background command \"cargo test\" (b1) is still running."));
    assert!(text.contains("Do NOT spawn a duplicate."));
    assert!(!text.contains("Recent output"));
    assert!(!text.contains("/tmp/task-b1.log"));
    assert!(!text.contains("SendMessage"));
    assert!(!text.contains("TaskOutput"));
    assert!(!text.contains("Read"));
}

#[tokio::test]
async fn running_teammate_does_not_include_send_message_or_read_hint() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "t1".into(),
            description: "worker@test".into(),
            status: TaskRunStatus::Running,
            killed_by: None,
            task_type: coco_types::TaskType::Teammate,
            progress_summary: None,
            output_file_path: Some("/tmp/task-t1.log".into()),
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
    assert!(text.contains("Teammate task \"worker@test\" (t1) is still running."));
    assert!(!text.contains("Recent output"));
    assert!(!text.contains("SendMessage"));
    assert!(!text.contains("Use Read"));
}

#[tokio::test]
async fn completed_uses_typed_wire_name_and_output_path_status() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "x".into(),
            description: "tidy".into(),
            status: TaskRunStatus::Completed,
            killed_by: None,
            task_type: coco_types::TaskType::BgAgent,
            progress_summary: Some("removed 3 files".into()),
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
    // Format: parts joined by space:
    // `(type: ...) (status: ...) (description: ...) <output-path-status>`.
    assert!(text.starts_with("Task x"));
    assert!(text.contains("(type: local_agent)"));
    assert!(text.contains("(status: completed)"));
    assert!(text.contains("(description: tidy)"));
    assert!(!text.contains("removed 3 files"));
    assert!(text.contains("Result output path is unavailable."));
    assert!(!text.contains("TaskOutput"));
}

#[tokio::test]
async fn failed_with_output_file_references_path() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .task_statuses(vec![TaskStatusSnapshot {
            task_id: "9".into(),
            description: "build".into(),
            status: TaskRunStatus::Failed,
            killed_by: None,
            task_type: coco_types::TaskType::Shell,
            progress_summary: None,
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
    assert!(!text.contains("compile error"));
    assert!(text.contains("Read the output file to restore the result: /tmp/task-9.log"));
}
