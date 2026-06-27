//! Post-spawn handoff classifier.
//!
//! Runs after a subagent completes. Fail-opens when the `SideQueryHandle`
//! isn't installed.
//!
//! **W6.2 full**: the underlying logic now lives in free async
//! functions taking pre-cloned `Arc`s, so the detached engine task
//! in [`super::spawn::spawn_subagent`]'s sync path can call them
//! without holding a `&self` borrow across an `await` point.

use super::SwarmAgentHandle;

/// Prepend the classifier-unavailable warning to a sub-agent's output.
/// An empty body yields the warning alone.
fn prepend_unavailable_warning(body: Option<&str>) -> String {
    match body.map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) => format!("{}\n\n{b}", coco_subagent::UNAVAILABLE_WARNING),
        None => coco_subagent::UNAVAILABLE_WARNING.to_string(),
    }
}

/// Free-fn implementation of [`SwarmAgentHandle::classify_handoff_if_needed`].
/// Pre-cloned `side_query` lets the function run inside a detached
/// `tokio::spawn` body (W6.2 sync detach race).
pub(crate) async fn classify_handoff_inline(
    agent_type: &str,
    qr: &coco_tool_runtime::AgentQueryResult,
    side_query: Option<&coco_tool_runtime::SideQueryHandle>,
    permission_mode: Option<coco_types::PermissionMode>,
) -> Option<String> {
    let Some(side_query) = side_query else {
        return qr.response_text.clone();
    };

    // Classification only runs in `auto` permission mode (`default` /
    // `acceptEdits` already require user confirmation upstream;
    // `bypassPermissions` opts out). Without this gate the two-stage
    // classifier LLM side-query fired after *every* subagent completion
    // regardless of mode — extra cost plus spurious `SECURITY WARNING:`
    // rewrites in non-auto modes.
    if !coco_subagent::handoff_classifier_active(permission_mode, /*feature_enabled=*/ true) {
        return qr.response_text.clone();
    }

    // Build the transcript first, then skip when it is empty
    // (no read-only / tool-count exemption).
    let transcript = coco_subagent::build_handoff_transcript_summary(&qr.messages);
    if !coco_subagent::should_classify(&transcript) {
        return qr.response_text.clone();
    }
    let (sys1, user1) =
        coco_subagent::handoff_stage1_prompts(agent_type, &transcript, qr.tool_use_count);

    let stage1_text = match side_query
        .query(coco_tool_runtime::SideQueryRequest::simple(
            &sys1,
            &user1,
            "subagent-handoff-stage1",
        ))
        .await
    {
        Ok(resp) => resp.text.unwrap_or_default(),
        // Classifier unavailable: fail-open but prepend the warning so
        // the parent verifies the sub-agent's work.
        Err(_) => return Some(prepend_unavailable_warning(qr.response_text.as_deref())),
    };

    let stage1_verdict = coco_subagent::parse_classifier_response(&stage1_text);
    if matches!(stage1_verdict, coco_subagent::HandoffClassification::Safe) {
        return qr.response_text.clone();
    }

    let (sys2, user2) = coco_subagent::handoff_stage2_prompts(&stage1_text, &transcript);
    let stage2_text = match side_query
        .query(coco_tool_runtime::SideQueryRequest::simple(
            &sys2,
            &user2,
            "subagent-handoff-stage2",
        ))
        .await
    {
        Ok(resp) => resp.text.unwrap_or_default(),
        Err(_) => return Some(prepend_unavailable_warning(qr.response_text.as_deref())),
    };

    let final_verdict = coco_subagent::parse_classifier_response(&stage2_text);
    match coco_subagent::render_block_message(&final_verdict) {
        Some(blocked) => Some(blocked),
        None => qr.response_text.clone(),
    }
}

impl SwarmAgentHandle {
    /// Method wrapper around [`classify_handoff_inline`] for `&self`
    /// callers (tests + any non-detach paths). The W6.2 sync detach
    /// race goes through the free fn directly so it can run inside
    /// a detached `tokio::spawn` closure. Marked `#[allow(dead_code)]`
    /// because production callers use the free fn; tests still
    /// invoke this wrapper.
    #[allow(dead_code)]
    pub(crate) async fn classify_handoff_if_needed(
        &self,
        agent_type: &str,
        qr: &coco_tool_runtime::AgentQueryResult,
    ) -> Option<String> {
        // Tests exercise the classifier under `auto` mode (the only mode
        // that triggers it); production calls the free fn directly with
        // the spawn request's resolved permission mode.
        classify_handoff_inline(
            agent_type,
            qr,
            self.side_query(),
            Some(coco_types::PermissionMode::Auto),
        )
        .await
    }
}
