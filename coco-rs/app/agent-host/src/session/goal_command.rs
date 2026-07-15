use std::sync::Arc;

pub const TRUST_GATE_MESSAGE: &str = "/goal is only available in trusted workspaces. Restart, accept the trust dialog, and try again.";

pub fn build_goal_kickoff_prompt(condition: &str) -> String {
    format!(
        "A persistent goal is now active: \"{condition}\". Briefly acknowledge it, then immediately start working toward it — treat the objective as your directive and do not pause to ask the user what to do. The runtime keeps driving turns toward this goal until it is complete or blocked. When you believe the goal is met, call `report_goal_turn` with a completion candidate and cite your evidence; the runtime verifies before completing. Do not tell the user to run `/goal clear` after success — that's only for abandoning a goal early."
    )
}

pub struct GoalStatusModal {
    pub title: String,
    pub body: String,
}

pub async fn build_goal_status_modal_for_session(
    session: &crate::session_runtime::SessionHandle,
    fallback_text: String,
) -> GoalStatusModal {
    match session.goal_runtime().snapshot().await {
        Some(snapshot) => GoalStatusModal {
            title: if snapshot.is_terminal() {
                "Goal completed".to_string()
            } else {
                format!("Goal {:?}", snapshot.status())
            },
            body: goal_snapshot_modal_body(&snapshot),
        },
        None => GoalStatusModal {
            title: "Goal".to_string(),
            body: fallback_text,
        },
    }
}

fn goal_snapshot_modal_body(snapshot: &coco_goals::GoalSnapshot) -> String {
    let mut lines = vec![
        format!("Status: {:?}", snapshot.status()),
        format!(
            "Turns: {} (autonomous {}/{})",
            snapshot.counters.total_turns,
            snapshot.counters.autonomous_turns,
            snapshot.budget.max_autonomous_turns.get(),
        ),
        format!("Tokens: {}", snapshot.usage.total_tokens()),
        String::new(),
        "Objective:".to_string(),
        snapshot.objective.text.to_string(),
    ];
    if let Some(progress) = &snapshot.progress {
        lines.extend([
            String::new(),
            "Last progress:".to_string(),
            progress.summary.to_string(),
        ]);
    }
    if let Some(rejection) = &snapshot.last_rejection {
        lines.extend([
            String::new(),
            "Last completion rejection:".to_string(),
            rejection.detail.to_string(),
        ]);
    }
    // Resume prompt for a stopped goal (paused / blocked / usage / budget), so
    // a provider error or a user pause offers a one-command restart (§11.3).
    if snapshot.status().is_stopped() {
        lines.extend([
            String::new(),
            "/goal resume to continue · /goal clear to abandon".to_string(),
        ]);
    } else if !snapshot.is_terminal() {
        lines.extend([String::new(), "/goal clear to stop early".to_string()]);
    }
    lines.join("\n")
}

pub fn goal_snapshot_changed_notification(
    snapshot: Option<coco_types::GoalSnapshotView>,
) -> coco_types::ServerNotification {
    coco_types::ServerNotification::GoalSnapshotChanged(Box::new(
        coco_types::GoalSnapshotChangedParams { snapshot },
    ))
}

pub fn goal_status_message(payload: coco_types::GoalStatusPayload) -> coco_messages::Message {
    coco_messages::Message::Attachment(coco_messages::AttachmentMessage::silent_goal_status(
        payload,
    ))
}

pub fn goal_status_and_slash_messages(
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) -> Vec<coco_messages::Message> {
    let mut messages = vec![goal_status_message(payload)];
    messages.extend(crate::session_messages::slash_text_messages(
        "goal", args, text, /*is_sensitive*/ false,
    ));
    messages
}

pub async fn append_goal_status_to_history(
    session: &crate::session_runtime::SessionHandle,
    payload: coco_types::GoalStatusPayload,
) -> Vec<Arc<coco_messages::Message>> {
    session
        .append_messages_to_history(vec![goal_status_message(payload)])
        .await
}

pub async fn append_goal_status_and_slash_to_history(
    session: &crate::session_runtime::SessionHandle,
    payload: coco_types::GoalStatusPayload,
    args: &str,
    text: &str,
) -> Vec<Arc<coco_messages::Message>> {
    let messages = goal_status_and_slash_messages(payload, args, text);
    let appended = session.append_messages_to_history(messages.clone()).await;
    session.persist_local_transcript_messages(&messages).await;
    appended
}

/// The current non-terminal goal projected to its bounded snapshot view, or
/// `None` when no live goal exists. Reads the durable goal runtime snapshot —
/// the single source of truth — so control-plane surfaces (`/goal`, resume)
/// emit the same `GoalSnapshotChanged` the engine emits per turn.
pub async fn read_goal_snapshot_view(
    session: &crate::session_runtime::SessionHandle,
) -> Option<coco_types::GoalSnapshotView> {
    let snapshot = session.goal_runtime().snapshot().await?;
    (!snapshot.is_terminal()).then(|| crate::session::goal_view::goal_snapshot_view(&snapshot))
}

pub async fn resolve_goal_request_for_session(
    session: &crate::session_runtime::SessionHandle,
    request: coco_commands::GoalCommandRequest,
    trust_rejected: bool,
) -> GoalOutcome {
    let history_snapshot = session.history_messages().await;
    resolve_goal_request_for_session_with_history(
        session,
        request,
        &history_snapshot,
        trust_rejected,
    )
    .await
}

pub async fn resolve_goal_request_for_session_with_history(
    session: &crate::session_runtime::SessionHandle,
    request: coco_commands::GoalCommandRequest,
    _history: &[Arc<coco_messages::Message>],
    trust_rejected: bool,
) -> GoalOutcome {
    let goal = session.goal_runtime();
    match request {
        coco_commands::GoalCommandRequest::Status => {
            let text = match goal.snapshot().await {
                Some(snapshot) => format_goal_snapshot_status(&snapshot),
                None => "No goal set. Usage: `/goal <objective>`".to_string(),
            };
            GoalOutcome::Text(text)
        }
        coco_commands::GoalCommandRequest::Clear => match goal.snapshot().await {
            Some(snapshot) if !snapshot.is_terminal() => {
                let objective = snapshot.objective.text.to_string();
                let _ = goal
                    .apply(coco_goals::GoalCommand::Clear(coco_goals::Clear {
                        goal_id: snapshot.goal_id.clone(),
                        at: goal_now(),
                    }))
                    .await;
                GoalOutcome::StatusThenText {
                    status: goal_status_sentinel(true, objective.clone()),
                    text: format!("Goal cleared: {objective}"),
                }
            }
            _ => GoalOutcome::Text("No goal set".to_string()),
        },
        coco_commands::GoalCommandRequest::Pause => match goal.snapshot().await {
            Some(snapshot) if !snapshot.is_terminal() => {
                match goal
                    .apply(coco_goals::GoalCommand::Pause(coco_goals::Pause {
                        goal_id: snapshot.goal_id.clone(),
                        reason: coco_goals::PauseReason::UserInterrupt,
                        at: goal_now(),
                    }))
                    .await
                {
                    Ok(_) => GoalOutcome::Text(
                        "Goal paused. Use `/goal resume` to continue.".to_string(),
                    ),
                    Err(e) => GoalOutcome::Text(format!("Cannot pause goal: {e}")),
                }
            }
            _ => GoalOutcome::Text("No active goal to pause".to_string()),
        },
        coco_commands::GoalCommandRequest::Resume => match goal.snapshot().await {
            Some(snapshot) if snapshot.status().is_stopped() => {
                match goal
                    .apply(coco_goals::GoalCommand::Resume(coco_goals::Resume {
                        goal_id: snapshot.goal_id.clone(),
                        next_lease_id: coco_goals::GoalLeaseId::new(format!(
                            "lease-{}",
                            uuid::Uuid::new_v4()
                        )),
                        at: goal_now(),
                    }))
                    .await
                {
                    Ok(_) => {
                        // Nudge the continuation driver (§10.3) to start a turn for
                        // the now active+queued goal; resume alone is state-only.
                        session.goal_driver_edge().notify_one();
                        GoalOutcome::Text("Goal resumed.".to_string())
                    }
                    Err(e) => GoalOutcome::Text(format!("Cannot resume goal: {e}")),
                }
            }
            Some(_) => GoalOutcome::Text("Goal is not paused".to_string()),
            None => GoalOutcome::Text("No goal set".to_string()),
        },
        coco_commands::GoalCommandRequest::Set { condition } => {
            if trust_rejected {
                return GoalOutcome::Text(TRUST_GATE_MESSAGE.to_string());
            }
            // Replace any existing unfinished goal (a confirmation prompt is a
            // TUI refinement).
            if let Some(existing) = goal.snapshot().await
                && !existing.is_terminal()
            {
                let _ = goal
                    .apply(coco_goals::GoalCommand::Clear(coco_goals::Clear {
                        goal_id: existing.goal_id.clone(),
                        at: goal_now(),
                    }))
                    .await;
            }
            // Bind the session plan artifact at its current revision when a plan
            // file exists (design §5.5 plan-first goal binding).
            let plan = crate::session::goal_plan::current_plan_ref(
                session.session_id().as_str(),
                &session.session_plan_file_path(),
                goal_now(),
            );
            let create = coco_goals::GoalCommand::Create(coco_goals::CreateGoal {
                goal_id: coco_goals::GoalId::new(format!("goal-{}", uuid::Uuid::new_v4())),
                session_id: session.session_id().clone(),
                lease_id: coco_goals::GoalLeaseId::new(format!("lease-{}", uuid::Uuid::new_v4())),
                objective: coco_goals::GoalObjective::new(&condition),
                contract: None,
                policy: coco_goals::CompletionPolicy::CandidateWithEvidence,
                budget: coco_goals::GoalBudget::default(),
                plan,
                mode_gate: None,
                wake_id: coco_goals::WakeId::new(format!("wake-{}", uuid::Uuid::new_v4())),
                at: goal_now(),
            });
            match goal.apply(create).await {
                Ok(_) => GoalOutcome::SetAndRun {
                    status: goal_status_sentinel(false, condition.clone()),
                    text: format!("Goal set: {condition}"),
                    kickoff: build_goal_kickoff_prompt(&condition),
                },
                Err(e) => GoalOutcome::Text(format!("Failed to set goal: {e}")),
            }
        }
    }
}

/// One-line status summary from the durable goal snapshot.
pub fn format_goal_snapshot_status(snapshot: &coco_goals::GoalSnapshot) -> String {
    format!(
        "Goal {:?}: {}\nTurns: {} · autonomous {}/{} · tokens {}",
        snapshot.status(),
        snapshot.objective.text,
        snapshot.counters.total_turns,
        snapshot.counters.autonomous_turns,
        snapshot.budget.max_autonomous_turns.get(),
        snapshot.usage.total_tokens(),
    )
}

fn goal_now() -> coco_goals::Timestamp {
    coco_goals::Timestamp::from_millis(unix_time_ms())
}

pub fn goal_status_sentinel(met: bool, condition: String) -> coco_types::GoalStatusPayload {
    coco_types::GoalStatusPayload {
        met,
        condition,
        sentinel: true,
        ..Default::default()
    }
}

/// Side effects a `/goal` dispatch resolves to, decoupled from each runner's
/// I/O substrate (TUI events vs AppServer history vs headless `Vec`). The caller
/// performs the actual emit / append / engine-run via its own sinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalOutcome {
    /// Show `text`; no transcript mutation, no engine run. Covers status,
    /// "No goal set", and both gate rejections.
    Text(String),
    /// Append the `status` sentinel attachment, then show `text`. Emitted by
    /// `clear` when a goal was actually active.
    StatusThenText {
        status: coco_types::GoalStatusPayload,
        text: String,
    },
    /// Append the `status` sentinel, show `text`, then run the engine with
    /// `kickoff` as the user prompt. Emitted by a successful `set`.
    SetAndRun {
        status: coco_types::GoalStatusPayload,
        text: String,
        kickoff: String,
    },
}

/// The command-echo argument string for a `/goal` request, matching the
/// upstream transcript framing: empty for status, `clear` for any clear
/// keyword, the raw condition for a set.
pub fn goal_display_args(request: &coco_commands::GoalCommandRequest) -> &str {
    match request {
        coco_commands::GoalCommandRequest::Status => "",
        coco_commands::GoalCommandRequest::Clear => "clear",
        coco_commands::GoalCommandRequest::Pause => "pause",
        coco_commands::GoalCommandRequest::Resume => "resume",
        coco_commands::GoalCommandRequest::Set { condition } => condition,
    }
}

pub fn unix_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "goal_command.test.rs"]
mod tests;
