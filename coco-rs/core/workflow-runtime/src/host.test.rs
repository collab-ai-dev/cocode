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
fn cache_key_uses_call_index_prompt_and_canonical_opts() {
    let opts = WorkflowAgentOpts {
        phase: Some("Build".to_string()),
        model: Some("claude-opus-4-8".to_string()),
        ..WorkflowAgentOpts::default()
    };
    let key = AgentCacheKey::new(3, "do the thing".to_string(), &opts);
    assert_eq!(key.call_index, 3);
    assert_eq!(key.prompt, "do the thing");
    assert_eq!(key.canonical_opts, canonical_agent_opts(&opts));
}

#[test]
fn cache_key_distinguishes_repeated_identical_calls() {
    // Same prompt, same opts, different call site — distinct keys, so a resumed
    // loop replays each iteration's own recorded result.
    let opts = WorkflowAgentOpts::default();
    let first = AgentCacheKey::new(0, "same prompt".to_string(), &opts);
    let second = AgentCacheKey::new(1, "same prompt".to_string(), &opts);
    assert_ne!(first, second);
}
