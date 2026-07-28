use pretty_assertions::assert_eq;

use super::MAX_CLASSIFIER_SCHEMA_CHARS;
use super::agent_tool_input;
use super::prompt_with_schema;
use coco_tool_runtime::SubagentDispatch;

fn permission_context(mode: coco_types::PermissionMode) -> coco_types::ToolPermissionContext {
    coco_types::ToolPermissionContext {
        mode,
        additional_dirs: Default::default(),
        permission_rule_source_roots: Default::default(),
        allow_rules: Default::default(),
        deny_rules: Default::default(),
        ask_rules: Default::default(),
        bypass_available: false,
        pre_plan_mode: None,
        stripped_dangerous_rules: None,
        session_plan_file: None,
    }
}

fn dispatch<'a>(
    prompt: &'a str,
    subagent_type: Option<&'a str>,
    output_schema: Option<&'a serde_json::Value>,
    ctx: &'a coco_types::ToolPermissionContext,
) -> SubagentDispatch<'a> {
    SubagentDispatch {
        prompt,
        subagent_type,
        output_schema,
        permission_context: ctx,
        messages: &[],
        cwd: None,
    }
}

#[test]
fn dispatch_is_rendered_as_the_agent_tool_call_it_stands_for() {
    // The classifier judges this exact shape, so it has to match AgentTool's
    // wire schema — a drift here is a drift in what gets screened.
    let ctx = permission_context(coco_types::PermissionMode::Auto);
    assert_eq!(
        agent_tool_input(&dispatch("find the bug", Some("Explore"), None, &ctx)),
        serde_json::json!({ "prompt": "find the bug", "subagent_type": "Explore" })
    );
    assert_eq!(
        agent_tool_input(&dispatch("find the bug", None, None, &ctx)),
        serde_json::json!({ "prompt": "find the bug" })
    );
}

#[test]
fn schema_is_appended_to_the_judged_prompt() {
    let ctx = permission_context(coco_types::PermissionMode::Auto);
    let schema = serde_json::json!({ "type": "object" });
    let judged = prompt_with_schema(&dispatch("do it", None, Some(&schema), &ctx))
        .expect("a small schema is classifiable");
    assert_eq!(judged, "do it\n\n[output schema]\n{\"type\":\"object\"}");
}

#[test]
fn absent_schema_leaves_the_prompt_untouched() {
    let ctx = permission_context(coco_types::PermissionMode::Auto);
    assert_eq!(
        prompt_with_schema(&dispatch("do it", None, None, &ctx)).expect("no schema"),
        "do it"
    );
}

#[test]
fn oversized_schema_fails_closed() {
    // The schema reaches the classifier as prompt text. One too large to show it
    // must refuse the dispatch, not dispatch something unscreened.
    let ctx = permission_context(coco_types::PermissionMode::Auto);
    let schema = serde_json::json!({ "description": "x".repeat(MAX_CLASSIFIER_SCHEMA_CHARS) });
    let error = prompt_with_schema(&dispatch("do it", None, Some(&schema), &ctx))
        .expect_err("an oversized schema must refuse");
    assert!(error.contains("too large to classify safely"), "{error}");
}
