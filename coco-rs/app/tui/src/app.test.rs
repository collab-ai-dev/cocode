use std::collections::VecDeque;

use coco_types::AgentStreamEvent;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::TaskProgressParams;
use coco_types::TaskUsage;
use coco_types::TuiOnlyEvent;
use coco_types::WorkflowProgressEvent;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyEventState;
use crossterm::event::KeyModifiers;

use super::DEFERRED_CORE_EVENT_LIMIT;
use super::DeferredCoreEvent;
use super::auto_mode_denied_toast_message;
use super::convert_crossterm_event;
use super::defer_core_event;
use super::handle_classifier_denied;
use crate::events::TuiEvent;
use crate::state::AppState;
use crate::state::ExplainerFetch;
use crate::state::PanePromptState;
use crate::state::PermissionDetail;
use crate::state::PermissionPromptState;
use crate::state::ToastSeverity;

fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
    KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    }
}

fn converts_to_key_event(key_event: KeyEvent) -> bool {
    matches!(
        convert_crossterm_event(Event::Key(key_event)),
        Some(TuiEvent::Key(_))
    )
}

fn classifier_prompt(request_id: &str) -> PermissionPromptState {
    PermissionPromptState {
        request_id: request_id.to_string(),
        tool_name: "Bash".to_string(),
        description: "Run command".to_string(),
        detail: PermissionDetail::Bash {
            command: "rm -rf /tmp/test".to_string(),
            risk_description: None,
            working_dir: None,
        },
        risk_level: None,
        show_always_allow: true,
        classifier_checking: true,
        classifier_auto_approved: None,
        choices: None,
        selected_choice: 0,
        display_input: coco_types::PermissionDisplayInput::Command("rm -rf /tmp/test".to_string()),
        original_input: None,
        cwd: None,
        permission_suggestions: Vec::new(),
        worker_badge: None,
        explanation_visible: false,
        explanation: ExplainerFetch::NotFetched,
        prefix_input: None,
    }
}

#[test]
fn classifier_denied_toast_includes_reason_and_permissions_hint() {
    let mut state = AppState::new();
    state
        .ui
        .push_prompt(PanePromptState::Permission(classifier_prompt("req-1")));

    assert!(handle_classifier_denied(
        &mut state,
        "req-1",
        "destructive filesystem operation",
    ));

    match state.ui.interaction.active_prompt.as_ref() {
        Some(PanePromptState::Permission(prompt)) => assert!(!prompt.classifier_checking),
        other => panic!("expected permission prompt, got {other:?}"),
    }
    assert_eq!(state.ui.toasts.len(), 1);
    assert_eq!(state.ui.toasts[0].severity, ToastSeverity::Warning);
    assert_eq!(
        state.ui.toasts[0].message,
        "bash denied by auto mode · destructive filesystem operation · /permissions"
    );
    assert_eq!(state.ui.recent_denials.len(), 1);
    assert_eq!(state.ui.recent_denials[0].display, "Run command");
    assert_eq!(
        state.ui.recent_denials[0].reason,
        "destructive filesystem operation"
    );
}

#[test]
fn classifier_denied_toast_truncates_long_reason() {
    let reason = "x".repeat(81);

    let message = auto_mode_denied_toast_message("Bash", &reason);

    assert_eq!(
        message,
        format!(
            "bash denied by auto mode · {}… · /permissions",
            "x".repeat(79)
        )
    );
}

#[test]
fn classifier_denied_toast_truncates_by_utf16_units_without_splitting_chars() {
    let exact_utf16_limit = "😀".repeat(40);
    assert_eq!(
        auto_mode_denied_toast_message("Bash", &exact_utf16_limit),
        format!("bash denied by auto mode · {exact_utf16_limit} · /permissions")
    );

    let over_utf16_limit = format!("{}a", "😀".repeat(40));
    assert_eq!(
        auto_mode_denied_toast_message("Bash", &over_utf16_limit),
        format!(
            "bash denied by auto mode · {}… · /permissions",
            "😀".repeat(39)
        )
    );
}

#[test]
fn crossterm_filter_accepts_key_press() {
    assert!(converts_to_key_event(key(
        KeyCode::Left,
        KeyModifiers::NONE,
        KeyEventKind::Press,
    )));
}

#[test]
fn crossterm_filter_accepts_navigation_repeats() {
    assert!(converts_to_key_event(key(
        KeyCode::Left,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    )));
    assert!(converts_to_key_event(key(
        KeyCode::Right,
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    )));
}

#[test]
fn crossterm_filter_rejects_key_release() {
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Left,
            KeyModifiers::NONE,
            KeyEventKind::Release,
        )))
        .is_none()
    );
}

#[test]
fn crossterm_filter_rejects_exit_chord_repeats() {
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Char('d'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
}

#[test]
fn crossterm_filter_rejects_one_shot_action_repeats() {
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Enter,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Esc,
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
}

#[test]
fn crossterm_filter_accepts_plain_character_repeat_only() {
    assert!(converts_to_key_event(key(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
        KeyEventKind::Repeat,
    )));
    assert!(
        convert_crossterm_event(Event::Key(key(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        )))
        .is_none()
    );
}

fn lossy_text(n: usize) -> CoreEvent {
    CoreEvent::Stream(AgentStreamEvent::TextDelta {
        turn_id: format!("turn-{n}").into(),
        delta: "x".to_string(),
    })
}

fn workflow_progress_event(progress: TaskProgressParams) -> CoreEvent {
    CoreEvent::Protocol(ServerNotification::TaskProgress(progress))
}

fn task_progress(
    description: &str,
    workflow_progress: Vec<WorkflowProgressEvent>,
) -> TaskProgressParams {
    TaskProgressParams {
        task_id: "workflow-1".to_string(),
        tool_use_id: None,
        description: description.to_string(),
        usage: TaskUsage {
            total_tokens: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            tool_uses: 0,
            duration_ms: 0,
            cost_usd: 0.0,
            input_cost_usd: 0.0,
            output_cost_usd: 0.0,
        },
        last_tool_name: None,
        summary: None,
        agent_type: None,
        recent_activities: Vec::new(),
        workflow_progress,
    }
}

#[test]
fn deferred_event_buffer_coalesces_stream_deltas() {
    let mut buffer = VecDeque::new();

    assert!(matches!(
        defer_core_event(
            &mut buffer,
            CoreEvent::Stream(AgentStreamEvent::TextDelta {
                turn_id: "t1".into(),
                delta: "hello ".to_string(),
            }),
        ),
        DeferredCoreEvent::Buffered
    ));
    assert!(matches!(
        defer_core_event(
            &mut buffer,
            CoreEvent::Stream(AgentStreamEvent::TextDelta {
                turn_id: "t1".into(),
                delta: "world".to_string(),
            }),
        ),
        DeferredCoreEvent::Buffered
    ));

    assert_eq!(buffer.len(), 1);
    let CoreEvent::Stream(AgentStreamEvent::TextDelta { delta, .. }) = &buffer[0] else {
        panic!("expected coalesced text delta");
    };
    assert_eq!(delta, "hello world");
}

#[test]
fn deferred_event_buffer_merges_workflow_task_progress_deltas() {
    let mut buffer = VecDeque::new();
    let phase = WorkflowProgressEvent::WorkflowPhase {
        index: 0,
        title: "Build".to_string(),
    };
    let log = WorkflowProgressEvent::WorkflowLog {
        message: "Compiled".to_string(),
    };

    assert!(matches!(
        defer_core_event(
            &mut buffer,
            workflow_progress_event(task_progress("first", vec![phase.clone()])),
        ),
        DeferredCoreEvent::Buffered
    ));
    assert!(matches!(
        defer_core_event(
            &mut buffer,
            workflow_progress_event(task_progress("second", vec![log.clone()])),
        ),
        DeferredCoreEvent::Buffered
    ));

    assert_eq!(buffer.len(), 1);
    let CoreEvent::Protocol(ServerNotification::TaskProgress(progress)) = &buffer[0] else {
        panic!("expected coalesced task progress");
    };
    assert_eq!(progress.description, "second");
    assert_eq!(progress.workflow_progress, vec![phase, log]);
}

#[test]
fn deferred_event_buffer_replaces_cumulative_workflow_task_progress() {
    let mut buffer = VecDeque::new();
    let phase = WorkflowProgressEvent::WorkflowPhase {
        index: 0,
        title: "Build".to_string(),
    };
    let log = WorkflowProgressEvent::WorkflowLog {
        message: "Compiled".to_string(),
    };

    assert!(matches!(
        defer_core_event(
            &mut buffer,
            workflow_progress_event(task_progress("first", vec![phase.clone()])),
        ),
        DeferredCoreEvent::Buffered
    ));
    assert!(matches!(
        defer_core_event(
            &mut buffer,
            workflow_progress_event(task_progress("second", vec![phase.clone(), log.clone()],)),
        ),
        DeferredCoreEvent::Buffered
    ));

    let CoreEvent::Protocol(ServerNotification::TaskProgress(progress)) = &buffer[0] else {
        panic!("expected coalesced task progress");
    };
    assert_eq!(progress.description, "second");
    assert_eq!(progress.workflow_progress, vec![phase, log]);
}

#[test]
fn deferred_event_buffer_drops_lossy_overflow() {
    let mut buffer = VecDeque::new();
    for n in 0..DEFERRED_CORE_EVENT_LIMIT {
        assert!(matches!(
            defer_core_event(&mut buffer, lossy_text(n)),
            DeferredCoreEvent::Buffered
        ));
    }

    assert!(matches!(
        defer_core_event(&mut buffer, lossy_text(DEFERRED_CORE_EVENT_LIMIT)),
        DeferredCoreEvent::Dropped
    ));
    assert_eq!(buffer.len(), DEFERRED_CORE_EVENT_LIMIT);
}

#[test]
fn deferred_event_buffer_preserves_terminal_events_at_capacity() {
    let mut buffer = VecDeque::new();
    for n in 0..DEFERRED_CORE_EVENT_LIMIT {
        assert!(matches!(
            defer_core_event(&mut buffer, lossy_text(n)),
            DeferredCoreEvent::Buffered
        ));
    }

    let terminal = CoreEvent::Tui(TuiOnlyEvent::PromptEditorCompleted {
        content: "done".to_string(),
        modified: true,
    });
    assert!(matches!(
        defer_core_event(&mut buffer, terminal),
        DeferredCoreEvent::Buffered
    ));
    assert_eq!(buffer.len(), DEFERRED_CORE_EVENT_LIMIT);
    assert!(buffer.iter().any(|event| matches!(
        event,
        CoreEvent::Tui(TuiOnlyEvent::PromptEditorCompleted { .. })
    )));
}

#[test]
fn deferred_event_buffer_processes_non_lossy_when_no_lossy_slot_exists() {
    let mut buffer = VecDeque::new();
    for n in 0..DEFERRED_CORE_EVENT_LIMIT {
        buffer.push_back(CoreEvent::Protocol(ServerNotification::KeepAlive {
            timestamp: n as i64,
        }));
    }

    let event = CoreEvent::Protocol(ServerNotification::KeepAlive { timestamp: 999 });
    let DeferredCoreEvent::ProcessNow(event) = defer_core_event(&mut buffer, event) else {
        panic!("expected oldest non-lossy event to process immediately");
    };
    assert!(matches!(
        *event,
        CoreEvent::Protocol(ServerNotification::KeepAlive { timestamp: 0 })
    ));
    assert_eq!(buffer.len(), DEFERRED_CORE_EVENT_LIMIT);
    assert!(matches!(
        buffer.back(),
        Some(CoreEvent::Protocol(ServerNotification::KeepAlive {
            timestamp: 999,
        }))
    ));
}
