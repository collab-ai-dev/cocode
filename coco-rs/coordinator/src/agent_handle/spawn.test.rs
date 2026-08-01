// Reads the process cwd, legitimate outside session-owned code; opts out of
// the workspace-wide `std::env::current_dir` gate (clippy.toml, §6.5/D-37).
#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use super::*;

#[test]
fn hook_context_carries_parent_session_id() {
    let session_id = coco_types::SessionId::try_new("parent-session").unwrap();

    let ctx = hook_ctx_for_subagent(
        "/tmp/project",
        &session_id,
        Some("agent-1"),
        Some("general-purpose"),
    );

    assert_eq!(ctx.session_id.as_str(), "parent-session");
    assert_eq!(ctx.agent_id.as_deref(), Some("agent-1"));
    assert_eq!(ctx.agent_type.as_deref(), Some("general-purpose"));
}

fn assistant_patch_call(patch: &str) -> Arc<coco_messages::Message> {
    Arc::new(coco_messages::create_assistant_message(
        vec![coco_messages::AssistantContent::ToolCall(
            coco_messages::ToolCallContent::new(
                "toolu_patch",
                coco_types::ToolName::ApplyPatch.as_str(),
                serde_json::json!({"patch": patch}),
            ),
        )],
        "test-model",
        coco_types::TokenUsage::default(),
    ))
}

fn tool_result(
    call_id: &str,
    tool_name: coco_types::ToolName,
    is_error: bool,
) -> Arc<coco_messages::Message> {
    Arc::new(coco_messages::create_tool_result_message(
        call_id,
        tool_name.as_str(),
        coco_types::ToolId::Builtin(tool_name),
        "ok",
        is_error,
    ))
}

#[test]
fn collect_written_paths_in_messages_counts_successful_apply_patch() {
    let cwd = std::env::current_dir().expect("cwd");
    let patch = "*** Begin Patch\n*** Add File: notes.md\n+hello\n*** End Patch\n";
    let messages = vec![
        assistant_patch_call(patch),
        tool_result(
            "toolu_patch",
            coco_types::ToolName::ApplyPatch,
            /*is_error*/ false,
        ),
    ];

    let paths = collect_written_paths_in_messages(&messages, &cwd);

    assert_eq!(paths, vec![cwd.join("notes.md")]);
}

#[test]
fn collect_written_paths_in_messages_ignores_failed_apply_patch() {
    let cwd = std::env::current_dir().expect("cwd");
    let patch = "*** Begin Patch\n*** Add File: notes.md\n+hello\n*** End Patch\n";
    let messages = vec![
        assistant_patch_call(patch),
        tool_result(
            "toolu_patch",
            coco_types::ToolName::ApplyPatch,
            /*is_error*/ true,
        ),
    ];

    let paths = collect_written_paths_in_messages(&messages, &cwd);

    assert!(paths.is_empty());
}

#[test]
fn fold_session_usage_into_task_progress_carries_live_cost_split() {
    let mut tracker = coco_types::TaskProgress::default();
    let totals = coco_types::SessionUsageTotals {
        input_tokens: 1_000,
        output_tokens: 200,
        cache_read_input_tokens: 400,
        input_cost_usd: 0.010,
        cache_read_cost_usd: 0.001,
        cache_creation_cost_usd: 0.002,
        output_cost_usd: 0.020,
        total_cost_usd: 0.033,
        ..Default::default()
    };

    assert!(fold_session_usage_into_task_progress(&mut tracker, &totals));

    assert_eq!(tracker.input_tokens, 1_000);
    assert_eq!(tracker.output_tokens, 200);
    assert_eq!(tracker.cache_read_tokens, 400);
    assert_eq!(tracker.cost_micro_usd, 33_000);
    assert_eq!(tracker.input_cost_micro_usd, 13_000);
    assert_eq!(tracker.output_cost_micro_usd, 20_000);

    let stale = coco_types::SessionUsageTotals {
        input_tokens: 900,
        output_tokens: 100,
        cache_read_input_tokens: 300,
        input_cost_usd: 0.005,
        output_cost_usd: 0.010,
        total_cost_usd: 0.015,
        ..Default::default()
    };

    assert!(!fold_session_usage_into_task_progress(&mut tracker, &stale));
    assert_eq!(tracker.cost_micro_usd, 33_000);
    assert_eq!(tracker.input_cost_micro_usd, 13_000);
    assert_eq!(tracker.output_cost_micro_usd, 20_000);
}

/// The drain must bridge a child engine's `TaskPanelChanged` snapshot to
/// the surface's panel sink — subagent engines share the leader's
/// `ToolAppState`, so the snapshot is session-authoritative — while every
/// other event stays isolated (only the tracked progress projections
/// leave the drain, via the task registry).
#[tokio::test]
async fn test_spawn_task_event_drain_bridges_task_panel_changed_only() {
    let registry: coco_tool_runtime::AgentTaskRegistryRef =
        Arc::new(coco_tool_runtime::NoOpBackgroundTaskHandle);
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(8);
    let (panel_tx, mut panel_rx) = tokio::sync::mpsc::channel(8);
    spawn_task_event_drain(
        registry,
        Arc::new(coco_tool_runtime::NoOpAgentLivenessReporter),
        "task-1".into(),
        "Explore".into(),
        event_rx,
        Some(panel_tx),
    );

    let params = coco_types::TaskPanelChangedParams {
        plan_tasks: Vec::new(),
        todos_by_agent: std::collections::HashMap::new(),
        expanded_view: coco_types::ExpandedView::Tasks,
        verification_nudge_pending: true,
        generation: 1,
    };
    event_tx
        .send(coco_types::CoreEvent::Protocol(
            coco_types::ServerNotification::TaskPanelChanged(params),
        ))
        .await
        .expect("drain must be alive");
    // A non-panel event must NOT be bridged.
    event_tx
        .send(coco_types::CoreEvent::Stream(
            coco_types::AgentStreamEvent::TextDelta {
                turn_id: "turn-1".into(),
                delta: "chunk".into(),
            },
        ))
        .await
        .expect("drain must be alive");
    drop(event_tx);

    let forwarded = panel_rx.recv().await.expect("panel snapshot bridged");
    match forwarded {
        coco_types::CoreEvent::Protocol(coco_types::ServerNotification::TaskPanelChanged(p)) => {
            assert!(matches!(p.expanded_view, coco_types::ExpandedView::Tasks));
            assert!(p.verification_nudge_pending);
        }
        other => panic!("expected TaskPanelChanged, got {other:?}"),
    }
    // Drain exit drops its `panel_tx` clone; a `None` here proves the
    // TextDelta was consumed without being bridged.
    assert!(
        panel_rx.recv().await.is_none(),
        "only TaskPanelChanged is bridged to the panel sink"
    );
}

struct ChannelLivenessReporter {
    tx: tokio::sync::mpsc::UnboundedSender<coco_tool_runtime::AgentExecutionPhase>,
}

#[async_trait::async_trait]
impl coco_tool_runtime::AgentLivenessReporter for ChannelLivenessReporter {
    async fn record_agent_activity(
        &self,
        _task_id: &str,
        phase: coco_tool_runtime::AgentExecutionPhase,
    ) {
        let _ = self.tx.send(phase);
    }
}

#[tokio::test]
async fn event_drain_stays_in_running_tool_until_parallel_batch_finishes() {
    let registry: coco_tool_runtime::AgentTaskRegistryRef =
        Arc::new(coco_tool_runtime::NoOpBackgroundTaskHandle);
    let (phase_tx, mut phase_rx) = tokio::sync::mpsc::unbounded_channel();
    let liveness = Arc::new(ChannelLivenessReporter { tx: phase_tx });
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(8);
    spawn_task_event_drain(
        registry,
        liveness,
        "task-parallel".into(),
        "Explore".into(),
        event_rx,
        None,
    );

    for call_id in ["first", "second"] {
        event_tx
            .send(coco_types::CoreEvent::Stream(
                coco_types::AgentStreamEvent::ToolUseStarted {
                    call_id: call_id.into(),
                    name: "Read".into(),
                    batch_id: Some("batch".into()),
                },
            ))
            .await
            .expect("drain alive");
    }
    event_tx
        .send(coco_types::CoreEvent::Stream(
            coco_types::AgentStreamEvent::ToolUseCompleted {
                call_id: "first".into(),
                name: "Read".into(),
                output: String::new(),
                is_error: false,
            },
        ))
        .await
        .expect("drain alive");
    event_tx
        .send(coco_types::CoreEvent::Stream(
            coco_types::AgentStreamEvent::ToolUseCompleted {
                call_id: "second".into(),
                name: "Read".into(),
                output: String::new(),
                is_error: false,
            },
        ))
        .await
        .expect("drain alive");

    assert_eq!(
        phase_rx.recv().await,
        Some(coco_tool_runtime::AgentExecutionPhase::RunningTool)
    );
    assert_eq!(
        phase_rx.recv().await,
        Some(coco_tool_runtime::AgentExecutionPhase::RunningTool)
    );
    assert_eq!(
        phase_rx.recv().await,
        Some(coco_tool_runtime::AgentExecutionPhase::RunningTool)
    );
    assert_eq!(
        phase_rx.recv().await,
        Some(coco_tool_runtime::AgentExecutionPhase::AwaitingModel)
    );
}

#[test]
fn builtin_and_mcp_lifecycles_with_same_id_are_independent() {
    let mut active = std::collections::HashSet::from([
        ActiveToolCall::Builtin("shared".into()),
        ActiveToolCall::Mcp("shared".into()),
    ]);
    active.remove(&ActiveToolCall::Mcp("shared".into()));
    assert_eq!(
        activity_phase(&active),
        coco_tool_runtime::AgentExecutionPhase::RunningTool
    );
    active.remove(&ActiveToolCall::Builtin("shared".into()));
    assert_eq!(
        activity_phase(&active),
        coco_tool_runtime::AgentExecutionPhase::AwaitingModel
    );
}
