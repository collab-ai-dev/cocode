use super::AgentCacheKey;
use super::WorkflowAgentOpts;
use super::canonical_agent_opts;

#[test]
fn canonical_opts_is_order_and_label_insensitive() {
    // Two opts that differ only in cosmetic fields (label, phase, stall_ms) and
    // in the nested-schema key order must produce identical canonical strings.
    let a = WorkflowAgentOpts {
        label: Some("first label".to_string()),
        phase: Some("Plan".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        effort: Some("high".to_string()),
        agent_type: Some("Explore".to_string()),
        isolation: Some(coco_types::AgentIsolation::Worktree),
        schema: Some(serde_json::json!({ "b": 2, "a": 1, "nested": { "y": 1, "x": 2 } })),
        stall_ms: Some(1000),
    };
    let b = WorkflowAgentOpts {
        label: Some("a totally different label".to_string()),
        phase: Some("Plan".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        effort: Some("high".to_string()),
        agent_type: Some("Explore".to_string()),
        isolation: Some(coco_types::AgentIsolation::Worktree),
        // Same logical schema, different key insertion order.
        schema: Some(serde_json::json!({ "nested": { "x": 2, "y": 1 }, "a": 1, "b": 2 })),
        stall_ms: Some(999_999),
    };
    assert_eq!(canonical_agent_opts(&a), canonical_agent_opts(&b));
}

#[test]
fn canonical_opts_distinguishes_cache_relevant_fields() {
    let base = WorkflowAgentOpts {
        model: Some("claude-opus-4-8".to_string()),
        ..WorkflowAgentOpts::default()
    };
    let changed_model = WorkflowAgentOpts {
        model: Some("claude-sonnet-4-8".to_string()),
        ..WorkflowAgentOpts::default()
    };
    assert_ne!(
        canonical_agent_opts(&base),
        canonical_agent_opts(&changed_model)
    );
}

#[test]
fn cache_key_uses_phase_prompt_and_canonical_opts() {
    let opts = WorkflowAgentOpts {
        phase: Some("Build".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        ..WorkflowAgentOpts::default()
    };
    let key = AgentCacheKey::new("do the thing".to_string(), &opts);
    assert_eq!(key.phase_title, "Build");
    assert_eq!(key.prompt, "do the thing");
    assert_eq!(key.canonical_opts, canonical_agent_opts(&opts));

    // No phase → empty phase_title.
    let no_phase = WorkflowAgentOpts::default();
    let key2 = AgentCacheKey::new("x".to_string(), &no_phase);
    assert_eq!(key2.phase_title, "");
}
