use coco_types::WorkflowAgentState;
use coco_types::WorkflowProgressEvent;
use pretty_assertions::assert_eq;

use super::MAX_WORKFLOW_PROGRESS_NODES;
use super::apply_workflow_progress;
use super::stamp_progress_time;

fn agent(index: i32, state: WorkflowAgentState, label: &str) -> WorkflowProgressEvent {
    WorkflowProgressEvent::WorkflowAgent {
        index,
        state,
        label: label.to_string(),
        phase_title: None,
        phase_index: None,
        agent_id: None,
        model: None,
        started_at: None,
        queued_at: None,
        last_progress_at: None,
        tokens: None,
        tool_calls: None,
        duration_ms: None,
        cached: false,
        result_preview: None,
        prompt_preview: None,
        error: None,
        skipped: false,
    }
}

fn log(message: &str) -> WorkflowProgressEvent {
    WorkflowProgressEvent::WorkflowLog {
        message: message.to_string(),
    }
}

fn phase(index: i32, title: &str) -> WorkflowProgressEvent {
    WorkflowProgressEvent::WorkflowPhase {
        index,
        title: title.to_string(),
    }
}

#[test]
fn test_apply_workflow_progress_upserts_agent_by_index() {
    let mut nodes = Vec::new();
    apply_workflow_progress(&mut nodes, agent(0, WorkflowAgentState::Start, "scan"));
    apply_workflow_progress(&mut nodes, agent(1, WorkflowAgentState::Start, "fix"));
    apply_workflow_progress(&mut nodes, agent(0, WorkflowAgentState::Done, "scan"));

    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0], agent(0, WorkflowAgentState::Done, "scan"));
    assert_eq!(nodes[1], agent(1, WorkflowAgentState::Start, "fix"));
}

#[test]
fn test_apply_workflow_progress_keys_agents_and_phases_separately() {
    let mut nodes = Vec::new();
    apply_workflow_progress(&mut nodes, agent(1, WorkflowAgentState::Start, "scan"));
    apply_workflow_progress(&mut nodes, phase(1, "Scan"));

    // Same index, different kind — two nodes, not an overwrite.
    assert_eq!(nodes.len(), 2);
}

#[test]
fn test_apply_workflow_progress_appends_every_log() {
    let mut nodes = Vec::new();
    apply_workflow_progress(&mut nodes, log("one"));
    apply_workflow_progress(&mut nodes, log("one"));

    assert_eq!(nodes, vec![log("one"), log("one")]);
}

#[test]
fn test_apply_workflow_progress_trims_oldest_logs_only() {
    let mut nodes = Vec::new();
    apply_workflow_progress(&mut nodes, agent(0, WorkflowAgentState::Start, "scan"));
    apply_workflow_progress(&mut nodes, phase(1, "Scan"));
    for i in 0..(MAX_WORKFLOW_PROGRESS_NODES * 2) {
        apply_workflow_progress(&mut nodes, log(&format!("line {i}")));
    }

    // The trim fires at 2× and cuts back to the mark, so the array settles just
    // above it rather than growing with the 1000 logs pushed.
    assert!(
        nodes.len() <= MAX_WORKFLOW_PROGRESS_NODES + 1,
        "array stayed bounded: {}",
        nodes.len()
    );
    // Structure survives the trim …
    assert_eq!(nodes[0], agent(0, WorkflowAgentState::Start, "scan"));
    assert_eq!(nodes[1], phase(1, "Scan"));
    // … and the surviving logs are the most recent ones.
    assert_eq!(
        nodes.last(),
        Some(&log(&format!(
            "line {}",
            MAX_WORKFLOW_PROGRESS_NODES * 2 - 1
        )))
    );
    assert!(!nodes.contains(&log("line 0")));
}

#[test]
fn test_stamp_progress_time_only_touches_agent_nodes() {
    let mut event = agent(0, WorkflowAgentState::Start, "scan");
    stamp_progress_time(&mut event, 1_700_000_000_000);
    let WorkflowProgressEvent::WorkflowAgent {
        last_progress_at, ..
    } = &event
    else {
        panic!("expected an agent node");
    };
    assert_eq!(*last_progress_at, Some(1_700_000_000_000));

    let mut untouched = log("hello");
    stamp_progress_time(&mut untouched, 1_700_000_000_000);
    assert_eq!(untouched, log("hello"));
}
