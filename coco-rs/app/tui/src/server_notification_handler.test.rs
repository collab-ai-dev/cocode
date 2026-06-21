//! Tests for server notification handler.
//!
//! Tests use `CoreEvent` directly. Turn-lifecycle and tool-use-lifecycle
//! suites that asserted on TUI-synthesised buffer entries are gone —
//! the engine pushes authoritative `Message::Assistant` /
//! `Message::ToolResult` via `MessageAppended`; the TUI just renders
//! the cells. Reasoning-token metadata is stamped via
//! `TranscriptView::record_reasoning_tokens` — exercised by the cell
//! renderer tests in `state::transcript_view` and `widgets`.

use std::time::Duration;
use std::time::Instant;

use coco_types::CoreEvent;
use coco_types::ServerNotification;

use coco_types::AgentStreamEvent;

use crate::server_notification_handler::handle_event_for_test as handle_core_event;
use crate::state::AppState;
use crate::state::session::HookEntry;
use crate::state::session::HookEntryStatus;
use crate::state::session::QueuedCommandDisplay;
use crate::state::session::SubagentInstance;
use crate::state::session::SubagentKind;
use crate::state::session::SubagentStatus;
use crate::state::session::TaskEntry;
use crate::state::session::TaskEntryStatus;

#[test]
fn test_bg_agent_task_started_bridges_into_subagents() {
    // BgAgent TaskStarted (wire `task_type == "local_agent"`) populates
    // `session.subagents`. The bridge links via `tool_use_id` so the
    // inline `AgentProgressLine` renderer can later attach the row to
    // the parent `Agent` tool execution.
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskStarted(
            coco_types::TaskStartedParams {
                task_id: "agent-bg-1".into(),
                tool_use_id: Some("tu-42".into()),
                description: "Investigate auth flow".into(),
                task_type: Some("local_agent".into()),
                workflow_name: None,
                prompt: None,
                agent_name: None,
                team_name: None,
                color: None,
                backend_kind: None,
            },
        )),
    );

    assert_eq!(state.session.subagents.len(), 1);
    let agent = &state.session.subagents[0];
    assert_eq!(agent.agent_id, "agent-bg-1");
    assert_eq!(agent.description, "Investigate auth flow");
    assert_eq!(agent.status, crate::state::session::SubagentStatus::Running);
    // active_tasks projection still works.
    assert_eq!(state.session.active_tasks.len(), 1);
}

#[test]
fn test_shell_task_started_does_not_create_subagent() {
    // Only `local_agent` / `in_process_teammate` task_types bridge into
    // `session.subagents`; `local_bash` (and `dream`) stay in
    // `active_tasks` only — they're not subagents.
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskStarted(
            coco_types::TaskStartedParams {
                task_id: "sh-1".into(),
                tool_use_id: Some("tu-99".into()),
                description: "sleep 30".into(),
                task_type: Some("local_bash".into()),
                workflow_name: None,
                prompt: None,
                agent_name: None,
                team_name: None,
                color: None,
                backend_kind: None,
            },
        )),
    );

    assert_eq!(state.session.subagents.len(), 0);
    assert_eq!(state.session.active_tasks.len(), 1);
}

#[test]
fn test_in_process_teammate_task_started_creates_teammate_kind_row() {
    // Coordinator emits `task/started` with `task_type == "in_process_teammate"`
    // and the optional teammate metadata populated. coco-rs projects that into
    // a SubagentInstance with kind=Teammate, team_name set, tool_use_id None.
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskStarted(
            coco_types::TaskStartedParams {
                task_id: "researcher@my-team".into(),
                tool_use_id: None,
                description: "Kick off auth research".into(),
                task_type: Some("in_process_teammate".into()),
                workflow_name: None,
                prompt: None,
                agent_name: Some("researcher".into()),
                team_name: Some("my-team".into()),
                color: Some("blue".into()),
                backend_kind: Some("in_process".into()),
            },
        )),
    );

    assert_eq!(state.session.subagents.len(), 1);
    let agent = &state.session.subagents[0];
    assert_eq!(agent.agent_id, "researcher@my-team");
    assert!(matches!(
        agent.kind,
        crate::state::session::SubagentKind::Teammate
    ));
    assert_eq!(agent.agent_type, "researcher");
    assert_eq!(agent.team_name.as_deref(), Some("my-team"));
    assert_eq!(agent.color, Some(coco_types::AgentColorName::Blue));
    // active_tasks projection works in parallel.
    assert_eq!(state.session.active_tasks.len(), 1);
}

#[test]
fn test_teammate_task_started_dedupes_on_task_id() {
    // Re-emit with the same task_id is a no-op (coordinator may
    // republish refresh-style events without duplicating rows).
    let mut state = AppState::new();
    let params = coco_types::TaskStartedParams {
        task_id: "r@t".into(),
        tool_use_id: None,
        description: "".into(),
        task_type: Some("in_process_teammate".into()),
        workflow_name: None,
        prompt: None,
        agent_name: Some("r".into()),
        team_name: Some("t".into()),
        color: None,
        backend_kind: Some("in_process".into()),
    };
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskStarted(params.clone())),
    );
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskStarted(params)),
    );
    // active_tasks accumulates — the dedup is only for the subagent projection.
    assert_eq!(state.session.subagents.len(), 1);
}

#[test]
fn test_bg_agent_task_started_marks_subagent_kind() {
    // The BgAgent bridge should set `kind == Subagent`, not Teammate.
    let mut state = AppState::new();
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskStarted(
            coco_types::TaskStartedParams {
                task_id: "agent-bg-X".into(),
                tool_use_id: Some("tu-1".into()),
                description: "task".into(),
                task_type: Some("local_agent".into()),
                workflow_name: None,
                prompt: None,
                agent_name: None,
                team_name: None,
                color: None,
                backend_kind: None,
            },
        )),
    );
    let agent = &state.session.subagents[0];
    assert!(matches!(
        agent.kind,
        crate::state::session::SubagentKind::Subagent
    ));
    assert_eq!(agent.team_name, None);
}

#[test]
fn test_bg_agent_task_completed_updates_subagent_status() {
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskStarted(
            coco_types::TaskStartedParams {
                task_id: "agent-bg-1".into(),
                tool_use_id: Some("tu-42".into()),
                description: "task".into(),
                task_type: Some("local_agent".into()),
                workflow_name: None,
                prompt: None,
                agent_name: None,
                team_name: None,
                color: None,
                backend_kind: None,
            },
        )),
    );

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TaskCompleted(
            coco_types::TaskCompletedParams {
                task_id: "agent-bg-1".into(),
                tool_use_id: Some("tu-42".into()),
                status: coco_types::TaskCompletionStatus::Completed,
                output_file: String::new(),
                summary: "Found 7 callers".into(),
                usage: None,
            },
        )),
    );

    let agent = &state.session.subagents[0];
    assert_eq!(
        agent.status,
        crate::state::session::SubagentStatus::Completed
    );
    assert_eq!(agent.final_message.as_deref(), Some("Found 7 callers"));
}

#[test]
fn test_permission_request_shows_prompt() {
    let mut state = AppState::new();
    let ready_at = Instant::now() + Duration::from_secs(2);

    handle_core_event(
        &mut state,
        CoreEvent::Tui(coco_types::TuiOnlyEvent::ApprovalRequired {
            request_id: "req-1".into(),
            tool_name: "Bash".into(),
            description: "Execute command".into(),
            display_input: coco_types::PermissionDisplayInput::Command("rm -rf /tmp/test".into()),
            show_always_allow: true,
            choices: None,
            detail: None,
            permission_suggestions: vec![],
            original_input: None,
            cwd: None,
            worker_badge: None,
        }),
    );
    assert!(!state.has_active_surface());
    assert!(state.ui.flush_delayed_permissions(ready_at));

    assert!(state.has_active_surface());
    assert!(matches!(
        state.ui.interaction.active_prompt,
        Some(crate::state::PanePromptState::Permission(_))
    ));
    match state.ui.interaction.active_prompt.as_ref() {
        Some(crate::state::PanePromptState::Permission(state)) => {
            assert!(state.show_always_allow);
        }
        other => panic!("expected permission state, got {other:?}"),
    }
}

#[test]
fn test_permission_request_hides_always_allow_when_disabled() {
    let mut state = AppState::new();
    let ready_at = Instant::now() + Duration::from_secs(2);

    handle_core_event(
        &mut state,
        CoreEvent::Tui(coco_types::TuiOnlyEvent::ApprovalRequired {
            request_id: "req-1".into(),
            tool_name: "Bash".into(),
            description: "Execute command".into(),
            display_input: coco_types::PermissionDisplayInput::Command("rm -rf /tmp/test".into()),
            show_always_allow: false,
            choices: None,
            detail: None,
            permission_suggestions: vec![],
            original_input: None,
            cwd: None,
            worker_badge: None,
        }),
    );
    assert!(state.ui.flush_delayed_permissions(ready_at));

    match state.ui.interaction.active_prompt.as_ref() {
        Some(crate::state::PanePromptState::Permission(state)) => {
            assert!(!state.show_always_allow);
        }
        other => panic!("expected permission state, got {other:?}"),
    }
}

#[test]
fn test_exit_plan_mode_permission_uses_dedicated_detail() {
    let mut state = AppState::new();
    let ready_at = Instant::now() + Duration::from_secs(2);

    handle_core_event(
        &mut state,
        CoreEvent::Tui(coco_types::TuiOnlyEvent::ApprovalRequired {
            request_id: "req-plan".into(),
            tool_name: coco_types::ToolName::ExitPlanMode.as_str().into(),
            description: "Exit plan mode?".into(),
            display_input: coco_types::PermissionDisplayInput::Empty,
            show_always_allow: true,
            choices: Some(vec![coco_types::PermissionAskChoice {
                value: "yes-default-keep-context".into(),
                label: "Yes, manually approve edits".into(),
                description: None,
            }]),
            detail: Some(coco_types::PermissionRequestDetail::ExitPlanMode {
                outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
                plan: Some("# Plan".into()),
                plan_file_path: Some("/tmp/plan.md".into()),
                allowed_prompts: vec![coco_types::ExitPlanModeAllowedPrompt {
                    tool: "Bash".into(),
                    prompt: "cargo test".into(),
                }],
            }),
            permission_suggestions: vec![],
            original_input: Some(serde_json::json!({
                "outcome": "implementation_plan",
                "plan": "# Plan",
                "planFilePath": "/tmp/plan.md",
                "allowedPrompts": [{"tool": "Bash", "prompt": "cargo test"}]
            })),
            cwd: None,
            worker_badge: None,
        }),
    );
    assert!(state.ui.flush_delayed_permissions(ready_at));

    match state.ui.interaction.active_prompt.as_ref() {
        Some(crate::state::PanePromptState::Permission(state)) => {
            assert!(!state.show_always_allow);
            let crate::state::PermissionDetail::ExitPlanMode {
                allowed_prompts, ..
            } = &state.detail
            else {
                panic!("expected ExitPlanMode detail")
            };
            assert_eq!(
                allowed_prompts,
                &vec![coco_types::ExitPlanModeAllowedPrompt {
                    tool: "Bash".to_string(),
                    prompt: "cargo test".to_string(),
                }]
            );
        }
        other => panic!("expected permission state, got {other:?}"),
    }
}

#[test]
fn test_exit_plan_mode_no_plan_permission_uses_yes_no_choices() {
    let mut state = AppState::new();
    let ready_at = Instant::now() + Duration::from_secs(2);

    handle_core_event(
        &mut state,
        CoreEvent::Tui(coco_types::TuiOnlyEvent::ApprovalRequired {
            request_id: "req-plan-no-plan".into(),
            tool_name: coco_types::ToolName::ExitPlanMode.as_str().into(),
            description: "Exit plan mode?".into(),
            display_input: coco_types::PermissionDisplayInput::Empty,
            show_always_allow: true,
            choices: Some(vec![
                coco_types::PermissionAskChoice {
                    value: "yes-default-keep-context".into(),
                    label: "Yes, exit plan mode".into(),
                    description: None,
                },
                coco_types::PermissionAskChoice {
                    value: "no".into(),
                    label: "No, keep planning".into(),
                    description: None,
                },
            ]),
            detail: Some(coco_types::PermissionRequestDetail::ExitPlanMode {
                outcome: coco_types::ExitPlanModeOutcome::NoImplementationPlan,
                plan: None,
                plan_file_path: None,
                allowed_prompts: Vec::new(),
            }),
            permission_suggestions: vec![],
            original_input: Some(serde_json::json!({
                "outcome": "no_implementation_plan",
                "planFilePath": "/tmp/stale-plan.md"
            })),
            cwd: None,
            worker_badge: None,
        }),
    );
    assert!(state.ui.flush_delayed_permissions(ready_at));

    let Some(crate::state::PanePromptState::Permission(prompt)) =
        state.ui.interaction.active_prompt.as_ref()
    else {
        panic!("expected permission state")
    };
    let labels: Vec<&str> = prompt
        .choices
        .as_ref()
        .expect("choices")
        .iter()
        .map(|choice| choice.label.as_str())
        .collect();
    assert_eq!(labels, vec!["Yes, exit plan mode", "No, keep planning"]);
    assert!(!prompt.choices.as_ref().unwrap().iter().any(|choice| {
        choice.label.contains("edit")
            || choice
                .description
                .as_deref()
                .unwrap_or_default()
                .contains("implementation")
    }));
}

#[test]
fn test_error_shows_toast() {
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::Error(coco_types::ErrorParams {
            message: "API rate limit".into(),
            category: None,
            retryable: true,
        })),
    );
    assert!(state.ui.has_toasts());
}

#[test]
fn test_mcp_status() {
    let mut state = AppState::new();

    // New server
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::McpStartupStatus(
            coco_types::McpStartupStatusParams {
                server: "github".into(),
                status: coco_types::McpConnectionStatus::Connected,
            },
        )),
    );
    assert_eq!(state.session.mcp_servers.len(), 1);
    assert_eq!(state.session.connected_mcp_count(), 1);

    // Update existing — disconnected
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::McpStartupStatus(
            coco_types::McpStartupStatusParams {
                server: "github".into(),
                status: coco_types::McpConnectionStatus::Failed,
            },
        )),
    );
    assert_eq!(state.session.mcp_servers.len(), 1);
    assert_eq!(state.session.connected_mcp_count(), 0);
}

#[test]
fn test_session_ended_quits() {
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::SessionEnded(
            coco_types::SessionEndedParams {
                reason: "max_turns".into(),
            },
        )),
    );
    assert!(state.should_exit());
}

#[test]
fn test_permission_mode_changed_covers_plan_entry_and_exit() {
    let mut state = AppState::new();
    assert!(!state.is_plan_mode());

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::PermissionModeChanged(
            coco_types::PermissionModeChangedParams {
                mode: coco_types::PermissionMode::Plan,
                bypass_available: true,
            },
        )),
    );
    assert!(state.is_plan_mode());
    assert!(state.session.bypass_permissions_available);

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::PermissionModeChanged(
            coco_types::PermissionModeChangedParams {
                mode: coco_types::PermissionMode::AcceptEdits,
                bypass_available: false,
            },
        )),
    );
    assert!(!state.is_plan_mode());
    assert_eq!(
        state.session.permission_mode,
        coco_types::PermissionMode::AcceptEdits
    );
    assert!(!state.session.bypass_permissions_available);
}

#[test]
fn test_context_compacted_toast() {
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::ContextCompacted(
            coco_types::ContextCompactedParams {
                removed_messages: 10,
                summary_tokens: 500,
                trigger: coco_types::CompactTrigger::Auto,
                pre_tokens: None,
                post_tokens: None,
            },
        )),
    );
    assert!(state.ui.has_toasts());
}

#[test]
fn test_compaction_protocol_updates_running_state() {
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::CompactionStarted),
    );
    assert!(state.session.is_compacting);
    assert!(state.session.compaction_started_at.is_some());

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::CompactionPhase(
            coco_types::CompactionPhaseParams {
                phase: coco_types::CompactionPhase::Summarizing,
                hook_type: None,
            },
        )),
    );
    assert_eq!(
        state.session.compaction_phase,
        Some(crate::state::session::CompactionPhaseLabel::Summarizing)
    );

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::ContextCompacted(
            coco_types::ContextCompactedParams {
                removed_messages: 2,
                summary_tokens: 10,
                trigger: coco_types::CompactTrigger::Manual,
                pre_tokens: Some(100),
                post_tokens: Some(40),
            },
        )),
    );
    assert!(!state.session.is_compacting);
    assert!(state.session.compaction_started_at.is_none());
    assert!(state.session.compaction_phase.is_none());
}

/// Regression: a stream delta arriving before TurnStarted must not be
/// silently dropped. Before the fix, handle_stream would no-op when
/// `state.ui.streaming` was None, so the first delta content was lost
/// whenever the channel reordered emission across senders.
#[test]
fn test_text_delta_before_turn_started_creates_streaming_state() {
    let mut state = AppState::new();
    assert!(state.ui.streaming.is_none());

    let changed = handle_core_event(
        &mut state,
        CoreEvent::Stream(AgentStreamEvent::TextDelta {
            turn_id: "t1".into(),
            delta: "orphan".into(),
        }),
    );

    assert!(changed);
    let streaming = state
        .ui
        .streaming
        .as_ref()
        .expect("streaming state must be created lazily");
    assert_eq!(
        streaming.content, "orphan",
        "delta must be appended, not dropped"
    );
}

/// Same invariant for ThinkingDelta.
#[test]
fn test_thinking_delta_before_turn_started_creates_streaming_state() {
    let mut state = AppState::new();
    assert!(state.ui.streaming.is_none());

    let changed = handle_core_event(
        &mut state,
        CoreEvent::Stream(AgentStreamEvent::ThinkingDelta {
            turn_id: "t1".into(),
            delta: "early thought".into(),
        }),
    );

    assert!(changed);
    let streaming = state
        .ui
        .streaming
        .as_ref()
        .expect("streaming state must be created lazily");
    assert_eq!(streaming.thinking, "early thought");
}

#[test]
fn test_session_reset_clears_transcript_adjacent_state() {
    let mut state = AppState::new();
    handle_core_event(
        &mut state,
        CoreEvent::Stream(AgentStreamEvent::ToolUseQueued {
            call_id: "tool-1".to_string(),
            name: "Read".to_string(),
            input: serde_json::json!({}),
        }),
    );
    state.session.set_busy(true);
    state.session.session_state = coco_types::SessionState::Running;
    state
        .session
        .queued_commands
        .push_back(QueuedCommandDisplay {
            id: "q1".to_string(),
            preview: "queued".to_string(),
            editable: true,
        });
    state.session.active_hooks.push(HookEntry {
        hook_id: "h1".to_string(),
        hook_name: "hook".to_string(),
        status: HookEntryStatus::Running,
        output: None,
    });
    state.session.prompt_suggestions = vec!["suggestion".to_string()];
    state
        .session
        .local_command_output
        .push_back("line".to_string());
    state.session.plan_tasks.push(coco_types::TaskRecord {
        id: "p1".to_string(),
        subject: "plan".to_string(),
        description: String::new(),
        active_form: None,
        owner: None,
        status: coco_types::TaskListStatus::InProgress,
        blocks: Vec::new(),
        blocked_by: Vec::new(),
        metadata: None,
    });
    state
        .session
        .todos_by_agent
        .insert("agent".to_string(), Vec::new());
    state.session.expanded_view = coco_types::ExpandedView::Tasks;
    state.session.verification_nudge_pending = true;
    state.ui.streaming = Some(crate::state::StreamingState::default());
    state.ui.ephemeral.start_turn("Working", Instant::now());
    state.ui.collapsed_tools.insert("tool-1".to_string());
    state.ui.show_modal(crate::state::ModalState::Help);

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::SessionResetForResume {
            session_id: "new-session".to_string(),
            agent_id: None,
        }),
    );

    assert!(!state.session.is_busy());
    assert_eq!(state.session.session_state, coco_types::SessionState::Idle);
    assert!(state.session.tool_executions.is_empty());
    assert!(state.session.queued_commands.is_empty());
    assert!(state.session.active_hooks.is_empty());
    assert!(state.session.prompt_suggestions.is_empty());
    assert!(state.session.local_command_output.is_empty());
    assert!(state.session.plan_tasks.is_empty());
    assert!(state.session.todos_by_agent.is_empty());
    assert_eq!(state.session.expanded_view, coco_types::ExpandedView::None);
    assert!(!state.session.verification_nudge_pending);
    assert!(state.ui.streaming.is_none());
    assert!(!state.ui.ephemeral.turn_active());
    assert!(state.ui.collapsed_tools.is_empty());
    assert!(!state.ui.has_active_surface());
}

#[test]
fn test_session_reset_preserves_only_persistent_running_subagents() {
    let mut state = AppState::new();
    state.session.subagents = vec![
        subagent(
            "foreground",
            SubagentKind::Subagent,
            SubagentStatus::Running,
            false,
        ),
        subagent(
            "done-bg",
            SubagentKind::Subagent,
            SubagentStatus::Completed,
            true,
        ),
        subagent(
            "running-bg",
            SubagentKind::Subagent,
            SubagentStatus::Running,
            true,
        ),
        subagent(
            "teammate",
            SubagentKind::Teammate,
            SubagentStatus::Running,
            false,
        ),
    ];
    state.session.active_tasks = state
        .session
        .subagents
        .iter()
        .map(|agent| TaskEntry {
            task_id: agent.agent_id.clone(),
            description: agent.description.clone(),
            status: TaskEntryStatus::Running,
            kind: crate::state::session::TaskEntryKind::Agent,
            started_at_ms: 0,
        })
        .collect();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::SessionResetForResume {
            session_id: "new-session".to_string(),
            agent_id: None,
        }),
    );

    let ids: Vec<&str> = state
        .session
        .subagents
        .iter()
        .map(|agent| agent.agent_id.as_str())
        .collect();
    assert_eq!(ids, vec!["running-bg", "teammate"]);
    let task_ids: Vec<&str> = state
        .session
        .active_tasks
        .iter()
        .map(|task| task.task_id.as_str())
        .collect();
    assert_eq!(task_ids, vec!["running-bg", "teammate"]);
}

#[test]
fn test_history_replaced_applies_same_boundary_cleanup() {
    let mut state = AppState::new();
    state.session.prompt_suggestions = vec!["stale".to_string()];
    state
        .session
        .queued_commands
        .push_back(QueuedCommandDisplay {
            id: "q1".to_string(),
            preview: "queued".to_string(),
            editable: true,
        });
    state.ui.streaming = Some(crate::state::StreamingState::default());

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::HistoryReplaced {
            messages: Vec::new(),
            session_id: String::new(),
            agent_id: None,
        }),
    );

    assert!(state.session.prompt_suggestions.is_empty());
    assert!(state.session.queued_commands.is_empty());
    assert!(state.ui.streaming.is_none());
}

#[test]
fn test_turn_completed_drops_streaming_orphans_keeps_committed() {
    use crate::state::session::ToolStatus;
    let mut state = AppState::new();

    // Build a representative in-flight ledger:
    // - `stream-orphan`: args started streaming but never finalized (no
    //   `ToolUseQueued`) — the leak case. message_uuid = None.
    state
        .session
        .begin_tool_stream("stream-orphan".to_string(), "Bash".to_string());
    // - `queued-unstamped`: queued but its assistant message never committed.
    state.session.start_tool(
        "queued-unstamped".to_string(),
        "Read".to_string(),
        &serde_json::json!({ "file_path": "/x" }),
    );
    // - `queued-stamped`: a committed queued call (stamped to a message).
    state.session.start_tool(
        "queued-stamped".to_string(),
        "Read".to_string(),
        &serde_json::json!({ "file_path": "/y" }),
    );
    // - `running`: genuinely executing.
    state.session.start_tool(
        "running".to_string(),
        "Bash".to_string(),
        &serde_json::json!({ "command": "sleep 1" }),
    );
    state.session.run_tool("running");
    // Stamp only the committed one.
    for t in &mut state.session.tool_executions {
        if t.call_id == "queued-stamped" {
            t.message_uuid = Some(uuid::Uuid::nil());
        }
    }
    assert_eq!(state.session.tool_executions.len(), 4);

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TurnEnded(
            coco_types::TurnEndedParams::completed(
                coco_types::TurnId::from("t-orphan"),
                None,
                Some(coco_messages::StopReason::EndTurn),
            ),
        )),
    );

    // Streaming orphan + uncommitted queued are dropped; only the running and
    // the committed-queued rows survive — no ghost activity row persists.
    let surviving: Vec<(&str, ToolStatus)> = state
        .session
        .tool_executions
        .iter()
        .map(|t| (t.call_id.as_str(), t.status))
        .collect();
    assert_eq!(
        surviving,
        vec![
            ("queued-stamped", ToolStatus::Queued),
            ("running", ToolStatus::Running),
        ]
    );
}

#[test]
fn test_live_status_tokens_start_fresh_and_follow_stream_deltas() {
    let mut state = AppState::new();
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TurnEnded(
            coco_types::TurnEndedParams::completed(
                coco_types::TurnId::from("t-test"),
                Some(coco_types::TokenUsage {
                    input_tokens: coco_types::InputTokens {
                        total: 100,
                        ..Default::default()
                    },
                    output_tokens: coco_types::OutputTokens {
                        total: 50,
                        ..Default::default()
                    },
                }),
                Some(coco_messages::StopReason::EndTurn),
            ),
        )),
    );
    assert_eq!(state.session.token_usage.output_tokens, 50);

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::TurnStarted(
            coco_types::TurnStartedParams {
                turn_id: coco_types::TurnId::from("t-test-2"),
            },
        )),
    );
    assert_eq!(state.ui.ephemeral.live_output_tokens(), 0);

    handle_core_event(
        &mut state,
        CoreEvent::Stream(AgentStreamEvent::TextDelta {
            turn_id: "t2".to_string(),
            delta: "abcdefghijkl".to_string(),
        }),
    );
    assert_eq!(state.ui.ephemeral.live_output_tokens(), 3);
    assert_eq!(
        state.session.token_usage.output_tokens, 50,
        "completed usage remains footer/status-bar data only"
    );
}

#[test]
fn test_session_usage_updated_replaces_footer_usage() {
    let mut state = AppState::new();
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::SessionUsageUpdated(Box::new(
            coco_types::SessionUsageSnapshot {
                session_id: "s1".into(),
                totals: coco_types::SessionUsageTotals {
                    input_tokens: 150,
                    output_tokens: 40,
                    cache_read_input_tokens: 50,
                    total_cost_usd: 0.01,
                    request_count: 2,
                    ..Default::default()
                },
                ..Default::default()
            },
        ))),
    );

    assert_eq!(state.session.token_usage.input_tokens, 150);
    assert_eq!(state.session.token_usage.output_tokens, 40);
    assert_eq!(state.session.token_usage.cache_read_tokens, 50);
    assert_eq!(
        state.session.session_usage.as_ref().unwrap().session_id,
        "s1"
    );
}

fn subagent(
    agent_id: &str,
    kind: SubagentKind,
    status: SubagentStatus,
    is_backgrounded: bool,
) -> SubagentInstance {
    SubagentInstance {
        kind,
        agent_id: agent_id.to_string(),
        agent_type: "agent".to_string(),
        description: agent_id.to_string(),
        status,
        color: None,
        team_name: None,
        started_at_ms: Some(0),
        last_tool_name: None,
        tool_count: 0,
        total_tokens: 0,
        is_backgrounded,
        recent_activities: Vec::new(),
        final_message: None,
        completed_at_ms: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cost_usd: 0.0,
    }
}

#[test]
fn test_model_role_changed_folds_into_session_and_role_map() {
    // G6: the engine→TUI half of the model-picker round-trip. `protocol.rs`
    // folds `ModelRoleChanged` into `model_by_role` (every role) and mirrors
    // the Main role into the status-bar fields. Previously untested.
    let mut state = AppState::new();

    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::ModelRoleChanged(
            coco_types::ModelRoleChangedParams {
                role: coco_types::ModelRole::Main,
                model_id: "claude-opus-4-8".into(),
                provider: "anthropic".into(),
                context_window: Some(200_000),
                effort: Some(coco_types::ReasoningEffort::High),
            },
        )),
    );
    assert_eq!(state.session.model, "claude-opus-4-8");
    assert_eq!(state.session.provider, "anthropic");
    assert_eq!(
        state.session.thinking_effort,
        coco_types::ReasoningEffort::High
    );
    let main = state
        .session
        .model_by_role
        .get(&coco_types::ModelRole::Main)
        .expect("Main binding folded into model_by_role");
    assert_eq!(main.model_id, "claude-opus-4-8");
    assert_eq!(main.provider, "anthropic");
    assert_eq!(main.context_window, Some(200_000));

    // A non-Main role updates ONLY the role map — the status-bar fields
    // (read directly from `session.model`) keep showing Main.
    handle_core_event(
        &mut state,
        CoreEvent::Protocol(ServerNotification::ModelRoleChanged(
            coco_types::ModelRoleChangedParams {
                role: coco_types::ModelRole::Fast,
                model_id: "claude-haiku-4-5".into(),
                provider: "anthropic".into(),
                context_window: Some(200_000),
                effort: None,
            },
        )),
    );
    assert_eq!(
        state.session.model, "claude-opus-4-8",
        "non-Main role must not overwrite the status-bar model"
    );
    let fast = state
        .session
        .model_by_role
        .get(&coco_types::ModelRole::Fast)
        .expect("Fast binding folded into model_by_role");
    assert_eq!(fast.model_id, "claude-haiku-4-5");
}
