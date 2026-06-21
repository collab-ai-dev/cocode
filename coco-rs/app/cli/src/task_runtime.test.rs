use super::*;
use coco_tool_runtime::{
    AgentCompletionPayload, AgentRegistration as AR, AgentUsage, AgentWorktree,
    BackgroundShellRequest, TaskHandle,
};
use coco_types::TaskStatus;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn rt() -> Arc<TaskRuntime> {
    Arc::new(TaskRuntime::new(Arc::new(coco_tasks::TaskManager::new())))
}

/// Capture-everything sink for assertions in tests.
#[derive(Default, Clone)]
struct CapturingSink {
    captured: Arc<tokio::sync::Mutex<Vec<coco_tasks::TaskNotification>>>,
}

#[async_trait::async_trait]
impl coco_tasks::NotificationSink for CapturingSink {
    async fn push(&self, n: coco_tasks::TaskNotification) {
        self.captured.lock().await.push(n);
    }
}

fn rt_with_sink(sink: CapturingSink) -> Arc<TaskRuntime> {
    Arc::new(
        TaskRuntime::new(Arc::new(coco_tasks::TaskManager::new()))
            .with_notification_sink(Arc::new(sink) as coco_tasks::NotificationSinkRef),
    )
}

#[tokio::test]
async fn register_creates_running_task_with_tool_use_id() {
    let rt = rt();
    let cancel = CancellationToken::new();
    let task_id = rt
        .register_agent_task(
            "explore something",
            Some("toolu_01"),
            None,
            cancel,
            AR::Foreground,
        )
        .await;

    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(state.id, task_id);
    assert_eq!(state.status, TaskStatus::Running);
    assert_eq!(state.tool_use_id.as_deref(), Some("toolu_01"));
    assert_eq!(state.description, "explore something");
}

#[tokio::test]
async fn register_agent_task_with_id_preserves_caller_id() {
    let rt = rt();
    let task_id = coco_types::generate_task_id(coco_types::TaskType::BgAgent);
    let returned = rt
        .register_agent_task_with_id(
            task_id.clone(),
            "explore something",
            Some("toolu_01"),
            None,
            CancellationToken::new(),
            AR::Background,
        )
        .await;

    assert_eq!(returned, task_id);
    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(state.id, task_id);
    assert!(state.is_backgrounded());
    assert_eq!(state.tool_use_id.as_deref(), Some("toolu_01"));
}

#[tokio::test]
async fn register_teammate_task_creates_queryable_task_projection() {
    let rt = rt();
    let task_id = rt
        .register_teammate_task(coco_tool_runtime::TeammateTaskRegistration::new(
            "worker",
            "test",
            coco_types::BackendType::Tmux,
            Some("%1".to_string()),
            "do work".to_string(),
            CancellationToken::new(),
        ))
        .await;

    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(state.task_type(), coco_types::TaskType::Teammate);
    let extras = state.teammate_extras().expect("teammate extras");
    assert_eq!(extras.agent_ref.to_string(), "worker@test");
    assert_eq!(extras.backend_type, coco_types::BackendType::Tmux);
    assert_eq!(extras.pane_id.as_deref(), Some("%1"));

    let by_agent = rt
        .teammate_task_state("worker@test")
        .await
        .expect("teammate row");
    assert_eq!(by_agent.id, task_id);
}

#[tokio::test]
async fn teammate_task_stop_rejects_terminal_rows() {
    let rt = rt();
    let cancel = CancellationToken::new();
    let task_id = rt
        .register_teammate_task(coco_tool_runtime::TeammateTaskRegistration::new(
            "worker",
            "test",
            coco_types::BackendType::InProcess,
            None,
            "do work".to_string(),
            cancel,
        ))
        .await;
    rt.complete_teammate_task(
        "worker@test",
        TaskStatus::Completed,
        Some("done".to_string()),
        None,
    )
    .await;

    let err = rt.kill_task(&task_id).await.unwrap_err();
    assert!(
        err.to_string().contains("not running"),
        "terminal teammate rows must not be stoppable: {err}"
    );
}

#[tokio::test]
async fn output_delta_returns_appended_chunks_with_offset() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;

    rt.append_output(&task_id, "first ").await;
    rt.append_output(&task_id, "second").await;

    let delta1 = rt.get_task_output_delta(&task_id, 0).await.unwrap();
    assert_eq!(delta1.content, "first second");
    assert_eq!(delta1.new_offset, 12);
    assert!(!delta1.is_complete);

    let delta2 = rt
        .get_task_output_delta(&task_id, delta1.new_offset)
        .await
        .unwrap();
    assert!(delta2.content.is_empty());
    assert!(!delta2.is_complete);
}

#[tokio::test]
async fn mark_completed_sets_terminal_status_and_appends_response() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    rt.mark_completed(
        &task_id,
        AgentCompletionPayload {
            result: Some("final answer".into()),
            ..Default::default()
        },
    )
    .await;
    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(state.status, TaskStatus::Completed);
    let delta = rt.get_task_output_delta(&task_id, 0).await.unwrap();
    assert_eq!(delta.content, "final answer");
    assert!(delta.is_complete);
}

#[tokio::test]
async fn mark_completed_pushes_rich_agent_notification() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);
    let task_id = rt
        .register_agent_task(
            "build",
            Some("toolu_x"),
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;
    rt.mark_completed(
        &task_id,
        AgentCompletionPayload {
            result: Some("done".into()),
            usage: Some(AgentUsage {
                usage: coco_types::TokenUsage {
                    input_tokens: coco_types::InputTokens {
                        total: 400,
                        ..Default::default()
                    },
                    output_tokens: coco_types::OutputTokens {
                        total: 100,
                        ..Default::default()
                    },
                },
                tool_uses: 3,
                duration_ms: 6000,
                cost_usd: 0.0,
            }),
            worktree: Some(AgentWorktree {
                path: "/tmp/wt".into(),
                branch: Some("feat/x".into()),
            }),
        },
    )
    .await;
    let captured = captured.lock().await;
    assert_eq!(captured.len(), 1);
    let kind = &captured[0].kind;
    let coco_tasks::NotificationKind::AgentTerminal {
        status,
        result,
        usage,
        worktree,
        ..
    } = kind
    else {
        panic!("expected AgentTerminal, got {kind:?}");
    };
    assert_eq!(*status, coco_tasks::TerminalStatus::Completed);
    assert_eq!(result.as_deref(), Some("done"));
    assert_eq!(usage.as_ref().unwrap().total_tokens, 500);
    assert_eq!(worktree.as_ref().unwrap().branch.as_deref(), Some("feat/x"));
    assert_eq!(captured[0].tool_use_id.as_deref(), Some("toolu_x"));
}

#[tokio::test]
async fn terminal_agent_notification_is_latched() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);
    let task_id = rt
        .register_agent_task(
            "build",
            Some("toolu_x"),
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;

    rt.mark_completed(&task_id, AgentCompletionPayload::default())
        .await;
    rt.mark_completed(&task_id, AgentCompletionPayload::default())
        .await;

    let captured = captured.lock().await;
    assert_eq!(captured.len(), 1, "terminal notification should fire once");
}

#[tokio::test]
async fn mark_failed_appends_error_and_flips_status() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    rt.mark_failed(&task_id, "transport crash").await;
    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(state.status, TaskStatus::Failed);
    let delta = rt.get_task_output_delta(&task_id, 0).await.unwrap();
    assert!(delta.content.contains("transport crash"));
    assert!(delta.is_complete);
}

/// D1 (W1): `kill_task` is single-responsibility — it ONLY fires the
/// cancel token. State transition (`update_status(Killed)`) and the
/// `<task-notification>` push are the driver's job. For an agent task,
/// the bg-agent closure in `coordinator::spawn_background` observes
/// cancel and calls `mark_failed`; for a shell task,
/// `apply_shell_terminal_state` runs on `WaitOutcome::Cancelled`. Doing
/// any of that here would double-fire SDK events + notifications.
/// In this unit test no driver exists, so the task remains in Running
/// state — that's correct behavior at this seam.
#[tokio::test]
async fn kill_task_only_fires_cancel_token() {
    let rt = rt();
    let cancel = CancellationToken::new();
    let task_id = rt
        .register_agent_task("work", None, None, cancel.clone(), AR::Foreground)
        .await;

    rt.kill_task(&task_id).await.unwrap();

    assert!(cancel.is_cancelled(), "cancel token must fire");
    // Status stays Running — no driver in this unit test to flip it.
    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(
        state.status,
        TaskStatus::Running,
        "kill_task must NOT update status directly; the driver does that"
    );
}

#[tokio::test]
async fn kill_task_does_not_push_notification_directly() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);
    let task_id = rt
        .register_agent_task(
            "explore",
            None,
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;
    rt.kill_task(&task_id).await.unwrap();
    let captured = captured.lock().await;
    assert!(
        captured.is_empty(),
        "kill_task must not push a notification — that's the driver's job. \
         Got {} notification(s).",
        captured.len()
    );
}

#[tokio::test]
async fn kill_task_rejects_terminal_task() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    rt.mark_completed(&task_id, AgentCompletionPayload::default())
        .await;

    assert!(
        rt.kill_task(&task_id).await.is_err(),
        "terminal tasks must not remain stoppable"
    );
}

#[tokio::test]
async fn kill_task_rejects_removed_task() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    rt.manager().remove_task(&task_id).await;

    assert!(
        rt.kill_task(&task_id).await.is_err(),
        "removed tasks must not remain stoppable through stale controls"
    );
}

/// End-to-end check: the canonical kill path is cancel → driver
/// observes cancel → driver calls `mark_failed` → ONE notification
/// gets pushed (no race / no duplicate).
#[tokio::test]
async fn kill_then_mark_failed_pushes_exactly_one_notification() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);
    let task_id = rt
        .register_agent_task(
            "explore",
            None,
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;
    rt.kill_task(&task_id).await.unwrap();
    // Simulate what the bg-agent closure does in production after
    // observing the cancel.
    rt.mark_failed(&task_id, "task cancelled by leader").await;
    let captured = captured.lock().await;
    assert_eq!(captured.len(), 1, "exactly one notification expected");
    assert!(
        matches!(
            captured[0].kind,
            coco_tasks::NotificationKind::AgentTerminal { .. }
        ),
        "expected AgentTerminal, got {:?}",
        captured[0].kind
    );
}

#[tokio::test]
async fn subscribe_terminal_fires_on_mark_completed() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    let signal = rt
        .subscribe_terminal(&task_id)
        .await
        .expect("entry must exist");
    let rt2 = rt.clone();
    let id2 = task_id.clone();
    let _handle = tokio::spawn(async move {
        rt2.mark_completed(&id2, AgentCompletionPayload::default())
            .await;
    });
    let final_status =
        tokio::time::timeout(std::time::Duration::from_secs(2), signal.await_terminal())
            .await
            .expect("terminal signal must fire within 2s");
    assert!(final_status.is_terminal());
}

#[tokio::test]
async fn subscribe_terminal_already_terminal_returns_immediately() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    rt.mark_completed(&task_id, AgentCompletionPayload::default())
        .await;
    // After termination, subscriber should see the stored terminal value.
    let signal = rt
        .subscribe_terminal(&task_id)
        .await
        .expect("entry retained");
    let s = tokio::time::timeout(std::time::Duration::from_secs(1), signal.await_terminal())
        .await
        .expect("should resolve immediately");
    assert!(s.is_terminal());
}

#[tokio::test]
async fn list_tasks_returns_all_registered() {
    let rt = rt();
    let _ = rt
        .register_agent_task("a", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    let _ = rt
        .register_agent_task("b", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    assert_eq!(rt.list_tasks().await.len(), 2);
}

#[tokio::test]
async fn unknown_task_id_errors() {
    let rt = rt();
    assert!(rt.get_task_status("ghost").await.is_err());
    assert!(rt.get_task_output_delta("ghost", 0).await.is_err());
    assert!(rt.kill_task("ghost").await.is_err());
    assert!(rt.subscribe_terminal("ghost").await.is_none());
    assert!(
        coco_tool_runtime::TaskHandle::detach_handle(&*rt, "ghost")
            .await
            .is_none()
    );
    assert_eq!(
        rt.signal_detach("ghost").await,
        coco_tool_runtime::DetachOutcome::Unknown
    );
    assert!(rt.read_terminal_outputs("ghost").await.is_err());
}

// ── W2: detach signal contract tests ────────────────────────────────

/// First `signal_detach` returns `Detached` (and notifies); subsequent
/// calls return `AlreadyDetached` (CAS-guarded).
#[tokio::test]
async fn signal_detach_is_idempotent() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    assert_eq!(
        rt.signal_detach(&task_id).await,
        coco_tool_runtime::DetachOutcome::Detached,
        "first signal must fire"
    );
    assert_eq!(
        rt.signal_detach(&task_id).await,
        coco_tool_runtime::DetachOutcome::AlreadyDetached,
        "second signal must be no-op"
    );
    assert_eq!(
        rt.signal_detach(&task_id).await,
        coco_tool_runtime::DetachOutcome::AlreadyDetached,
        "third signal must be no-op"
    );
}

/// `signal_detach` wakes an awaiter on the per-task `Notify`:
/// resolving the signal wakes the awaiter exactly once.
#[tokio::test]
async fn signal_detach_wakes_notify_awaiter() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    let notify = coco_tool_runtime::TaskHandle::detach_handle(&*rt, &task_id)
        .await
        .expect("entry must exist");
    // Race: signal first, then assert the awaiter wakes within 1s.
    let rt2 = rt.clone();
    let id2 = task_id.clone();
    let _handle = tokio::spawn(async move {
        // Small delay so the awaiter is parked first.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        rt2.signal_detach(&id2).await;
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), notify.notified())
        .await
        .expect("awaiter must wake within 1s");
}

/// `signal_detach` flips `TaskStateBase.is_backgrounded()` for
/// `LocalAgent` tasks so the TUI panel filter can hide detached
/// tasks. After the unification refactor `is_backgrounded` lives on
/// the canonical row (was on `BgAgentExtras` pre-refactor).
#[tokio::test]
async fn signal_detach_flips_is_backgrounded_for_local_agent() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    let state_before = rt.manager().get(&task_id).await.expect("task exists");
    assert!(!state_before.is_backgrounded());
    rt.signal_detach(&task_id).await;
    let state_after = rt.manager().get(&task_id).await.expect("task still exists");
    assert!(
        state_after.is_backgrounded(),
        "is_backgrounded must flip to true after signal_detach"
    );
}

/// `read_terminal_outputs` returns the on-disk content as `stdout`
/// (stdout+stderr merged in coco-rs file mode). `interrupted` is true
/// only when the task ended in `Killed`.
#[tokio::test]
async fn read_terminal_outputs_returns_disk_content() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    rt.append_output(&task_id, "line1\n").await;
    rt.append_output(&task_id, "line2\n").await;
    rt.mark_completed(&task_id, AgentCompletionPayload::default())
        .await;
    let outputs = rt
        .read_terminal_outputs(&task_id)
        .await
        .expect("must succeed");
    assert!(outputs.stdout.contains("line1"));
    assert!(outputs.stdout.contains("line2"));
    assert!(outputs.stderr.is_empty(), "merged into stdout in fg path");
    assert!(
        !outputs.interrupted,
        "Completed (not Killed) → interrupted=false"
    );
}

#[tokio::test]
async fn mark_completed_cancels_per_task_token() {
    let rt = rt();
    let cancel = CancellationToken::new();
    let task_id = rt
        .register_agent_task("work", None, None, cancel.clone(), AR::Foreground)
        .await;
    assert!(!cancel.is_cancelled());
    rt.mark_completed(&task_id, AgentCompletionPayload::default())
        .await;
    assert!(cancel.is_cancelled());
}

#[tokio::test]
async fn mark_failed_cancels_per_task_token() {
    let rt = rt();
    let cancel = CancellationToken::new();
    let task_id = rt
        .register_agent_task("work", None, None, cancel.clone(), AR::Foreground)
        .await;
    rt.mark_failed(&task_id, "boom").await;
    assert!(cancel.is_cancelled());
}

#[cfg(not(windows))]
#[tokio::test]
async fn shell_spawn_runs_command_and_marks_completed() {
    let rt = rt();
    let task_id = rt
        .spawn_shell_task(BackgroundShellRequest {
            command: "echo hello-bg".into(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
            timeout_ms: Some(5_000),
            description: "echo test".into(),
            tool_use_id: Some("toolu_sh1".into()),
            issuing_agent: None,
            progress_tx: None,
            progress_throttle_ms: 1000,
            auto_detach_ms: None,
            kill_on_timeout: true,
            sandbox_state: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
        })
        .await
        .expect("shell spawn should succeed");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let state = rt.get_task_status(&task_id).await.unwrap();
        if state.status.is_terminal() {
            assert_eq!(state.status, TaskStatus::Completed);
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("shell task did not complete within 5s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let delta = rt.get_task_output_delta(&task_id, 0).await.unwrap();
    assert!(delta.content.contains("hello-bg"));
    assert!(delta.is_complete);
}

#[cfg(not(windows))]
#[tokio::test]
async fn shell_spawn_propagates_nonzero_exit_as_failed() {
    let rt = rt();
    let task_id = rt
        .spawn_shell_task(BackgroundShellRequest {
            command: "exit 7".into(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
            timeout_ms: Some(5_000),
            description: "fail".into(),
            tool_use_id: None,
            issuing_agent: None,
            progress_tx: None,
            progress_throttle_ms: 1000,
            auto_detach_ms: None,
            kill_on_timeout: true,
            sandbox_state: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
        })
        .await
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let final_status = loop {
        let state = rt.get_task_status(&task_id).await.unwrap();
        if state.status.is_terminal() {
            break state.status;
        }
        if std::time::Instant::now() > deadline {
            panic!("task did not exit");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert_eq!(final_status, TaskStatus::Failed);
}

#[cfg(not(windows))]
#[tokio::test]
async fn shell_spawn_threads_tool_use_id_and_agent_id_into_notification() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);

    let _task_id = rt
        .spawn_shell_task(BackgroundShellRequest {
            command: "true".into(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
            timeout_ms: Some(5_000),
            description: "noop".into(),
            tool_use_id: Some("toolu_bash99".into()),
            issuing_agent: Some("agent-3".into()),
            progress_tx: None,
            progress_throttle_ms: 1000,
            auto_detach_ms: None,
            kill_on_timeout: true,
            sandbox_state: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
        })
        .await
        .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if !captured.lock().await.is_empty() {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("notification didn't arrive");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let captured = captured.lock().await;
    let n = &captured[0];
    assert_eq!(n.tool_use_id.as_deref(), Some("toolu_bash99"));
    assert_eq!(n.agent_id.as_deref(), Some("agent-3"));
    matches!(
        n.kind,
        coco_tasks::NotificationKind::ShellTerminal {
            status: coco_tasks::TerminalStatus::Completed,
            ..
        }
    );
}

#[cfg(not(windows))]
#[tokio::test]
async fn shell_stop_suppresses_model_notification() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);

    let task_id = rt
        .spawn_shell_task(BackgroundShellRequest {
            command: "sleep 5".into(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
            timeout_ms: Some(10_000),
            description: "sleep".into(),
            tool_use_id: Some("toolu_stop".into()),
            issuing_agent: None,
            progress_tx: None,
            progress_throttle_ms: 1000,
            auto_detach_ms: None,
            kill_on_timeout: true,
            sandbox_state: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
        })
        .await
        .unwrap();

    rt.kill_task(&task_id).await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let state = rt.get_task_status(&task_id).await.unwrap();
        if state.status == TaskStatus::Killed {
            assert!(state.notified);
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("shell task did not observe stop within 5s");
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let captured = captured.lock().await;
    assert!(
        captured.is_empty(),
        "TaskStop on shell must suppress XML notification; got {captured:?}"
    );
}

#[tokio::test]
async fn no_sink_means_no_panic_on_terminal() {
    let rt = rt();
    let task_id = rt
        .register_agent_task(
            "explore",
            None,
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;
    rt.mark_completed(&task_id, AgentCompletionPayload::default())
        .await;
}

// ── W6: Dream task registration ─────────────────────────────────────

/// W6: `register_dream_task` creates a task with `TaskType::Dream`
/// so the TUI panel + `TaskList` tool can differentiate auto-memory
/// consolidation from user-spawned subagents.
#[tokio::test]
async fn register_dream_task_creates_dream_typed_task() {
    let rt = rt();
    let task_id = rt
        .register_dream_task("auto-dream consolidation", CancellationToken::new())
        .await;
    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(state.task_type(), coco_types::TaskType::Dream);
    assert_eq!(state.status, TaskStatus::Running);
    assert_eq!(state.description, "auto-dream consolidation");
    assert!(
        task_id.starts_with('d'),
        "dream task ID must use the 'd' prefix per generate_task_id, got: {task_id}"
    );
}

#[tokio::test]
async fn register_dream_task_supports_kill_via_cancel_token() {
    let rt = rt();
    let cancel = CancellationToken::new();
    let task_id = rt.register_dream_task("auto-dream", cancel.clone()).await;
    assert!(!cancel.is_cancelled());
    rt.kill_task(&task_id).await.unwrap();
    assert!(cancel.is_cancelled());
}

// ── W4: complete_silent contract (sync agent path) ──────────────────

/// W4: `complete_silent` flips an undetached foreground task to a
/// terminal status without pushing a `<task-notification>`. The row
/// stays in the manager so the panel-grace sweep can evict it later;
/// eager removal would drop the row before the 30s grace window.
#[tokio::test]
async fn complete_silent_marks_foreground_terminal_without_notification() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);
    let task_id = rt
        .register_agent_task(
            "sync-work",
            None,
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;

    rt.complete_silent(&task_id, true).await;

    let state = rt
        .get_task_status(&task_id)
        .await
        .expect("row must survive complete_silent; sweep evicts later");
    assert_eq!(state.status, TaskStatus::Completed);
    assert!(
        !state.is_backgrounded(),
        "complete_silent must not flip foreground tasks into backgrounded state"
    );
    let captured = captured.lock().await;
    assert!(
        captured.is_empty(),
        "complete_silent must NOT push a notification. Got {} notification(s).",
        captured.len()
    );
}

#[tokio::test]
async fn complete_silent_marks_foreground_failed_without_notification() {
    let sink = CapturingSink::default();
    let captured = sink.captured.clone();
    let rt = rt_with_sink(sink);
    let task_id = rt
        .register_agent_task(
            "sync-work",
            None,
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;

    rt.complete_silent(&task_id, false).await;

    let state = rt
        .get_task_status(&task_id)
        .await
        .expect("row must survive complete_silent; sweep evicts later");
    assert_eq!(state.status, TaskStatus::Failed);
    assert!(!state.is_backgrounded());
    assert!(captured.lock().await.is_empty());
}

#[tokio::test]
async fn complete_silent_keeps_detached_task() {
    let rt = rt();
    let task_id = rt
        .register_agent_task(
            "sync-work",
            None,
            None,
            CancellationToken::new(),
            AR::Foreground,
        )
        .await;

    assert_eq!(
        rt.signal_detach(&task_id).await,
        coco_tool_runtime::DetachOutcome::Detached
    );
    rt.complete_silent(&task_id, true).await;

    let state = rt.get_task_status(&task_id).await.unwrap();
    assert_eq!(state.status, TaskStatus::Completed);
    assert!(state.is_backgrounded());
}

/// W4: `complete_silent` fires the per-task cancel token (so any
/// downstream timer / watcher exits) and broadcasts the terminal
/// status via the watch channel (so `TaskOutput(block=true)` waiters
/// resolve).
#[tokio::test]
async fn complete_silent_fires_cancel_and_broadcasts_terminal() {
    let rt = rt();
    let cancel = CancellationToken::new();
    let task_id = rt
        .register_agent_task("sync-work", None, None, cancel.clone(), AR::Foreground)
        .await;
    let signal = rt
        .subscribe_terminal(&task_id)
        .await
        .expect("entry must exist");

    assert!(!cancel.is_cancelled());

    rt.complete_silent(&task_id, true).await;

    assert!(cancel.is_cancelled(), "cancel token must fire");
    let final_status =
        tokio::time::timeout(std::time::Duration::from_secs(1), signal.await_terminal())
            .await
            .expect("terminal signal must fire within 1s");
    assert_eq!(final_status, TaskStatus::Completed);
}

// ── W3: progress timer + auto-detach + exit_code tests ──────────────

/// W3: `read_terminal_outputs` returns the actual `exit_code` for a
/// completed shell task. Validates that the shell driver persists
/// the code into the `OnceLock` slot via `apply_shell_terminal_state`.
#[cfg(not(windows))]
#[tokio::test]
async fn shell_spawn_persists_exit_code_for_terminal_outputs() {
    let rt = rt();
    let task_id = rt
        .spawn_shell_task(BackgroundShellRequest {
            command: "exit 42".into(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
            timeout_ms: Some(5_000),
            description: "exit-42".into(),
            tool_use_id: None,
            issuing_agent: None,
            progress_tx: None,
            progress_throttle_ms: 1000,
            auto_detach_ms: None,
            kill_on_timeout: true,
            sandbox_state: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
        })
        .await
        .unwrap();

    // Wait for terminal.
    let signal = rt
        .subscribe_terminal(&task_id)
        .await
        .expect("entry must exist");
    tokio::time::timeout(std::time::Duration::from_secs(5), signal.await_terminal())
        .await
        .expect("task must terminate within 5s");

    let outputs = rt
        .read_terminal_outputs(&task_id)
        .await
        .expect("terminal outputs must read");
    assert_eq!(outputs.exit_code, Some(42));
    assert!(!outputs.interrupted, "natural exit is not 'interrupted'");
}

/// W3: progress timer emits `bash_progress` events through the
/// `ProgressSender` while the task runs. The test uses a short
/// `progress_throttle_ms` and a sleeping command to observe at least
/// one tick.
#[cfg(not(windows))]
#[tokio::test]
async fn shell_spawn_emits_progress_events_through_progress_tx() {
    use coco_tool_runtime::ToolProgress;
    use tokio::sync::mpsc;

    let rt = rt();
    let (tx, mut rx) = mpsc::unbounded_channel::<ToolProgress>();
    let _task_id = rt
        .spawn_shell_task(BackgroundShellRequest {
            // Sleep long enough to ensure at least one progress tick
            // at 100 ms throttle, then exit.
            command: "sleep 0.3 && echo done".into(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
            timeout_ms: Some(5_000),
            description: "progress-test".into(),
            tool_use_id: Some("toolu_progress".into()),
            issuing_agent: None,
            progress_tx: Some(tx),
            progress_throttle_ms: 100,
            auto_detach_ms: None,
            kill_on_timeout: true,
            sandbox_state: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
        })
        .await
        .unwrap();

    let progress = tokio::time::timeout(std::time::Duration::from_secs(3), rx.recv())
        .await
        .expect("progress event must arrive within 3s")
        .expect("channel must yield at least one progress event");
    assert_eq!(progress.tool_use_id, "toolu_progress");
    assert_eq!(
        progress.data["type"].as_str(),
        Some("bash_progress"),
        "payload must carry `type` field"
    );
    assert_eq!(progress.data["status"].as_str(), Some("running"));
}

/// W3: auto-detach timer fires `signal_detach` after `auto_detach_ms`.
/// Validates idempotency-via-fight: signal_detach is fired by both the
/// timer and the test, and the second call returns `false`.
#[cfg(not(windows))]
#[tokio::test]
async fn shell_spawn_auto_detach_timer_fires() {
    let rt = rt();
    let task_id = rt
        .spawn_shell_task(BackgroundShellRequest {
            // Long-running command so auto-detach beats natural exit.
            command: "sleep 5".into(),
            shell_kind: coco_tool_runtime::BackgroundShellKind::DefaultPlatformShell,
            timeout_ms: Some(10_000),
            description: "auto-detach".into(),
            tool_use_id: None,
            issuing_agent: None,
            progress_tx: None,
            progress_throttle_ms: 1000,
            auto_detach_ms: Some(200),
            kill_on_timeout: false,
            sandbox_state: None,
            sandbox_bypass: coco_sandbox::SandboxBypass::No,
        })
        .await
        .unwrap();

    let notify = coco_tool_runtime::TaskHandle::detach_handle(&*rt, &task_id)
        .await
        .expect("entry must exist");
    tokio::time::timeout(std::time::Duration::from_secs(2), notify.notified())
        .await
        .expect("auto-detach must fire within 2s");

    // Subsequent explicit signal_detach is a no-op (already detached
    // by the timer).
    assert_eq!(
        rt.signal_detach(&task_id).await,
        coco_tool_runtime::DetachOutcome::AlreadyDetached,
        "second signal must be CAS no-op"
    );
    // Cleanup so the test doesn't leak a sleeping subprocess.
    rt.kill_task(&task_id).await.unwrap();
}

#[tokio::test]
async fn read_output_returns_full_buffer_after_completion() {
    let rt = rt();
    let task_id = rt
        .register_agent_task("work", None, None, CancellationToken::new(), AR::Foreground)
        .await;
    rt.append_output(&task_id, "alpha").await;
    rt.mark_completed(
        &task_id,
        AgentCompletionPayload {
            result: Some(" beta".into()),
            ..Default::default()
        },
    )
    .await;
    let buf = rt.read_output(&task_id).await;
    assert!(buf.starts_with("alpha"));
    assert!(buf.contains("beta"));
}
