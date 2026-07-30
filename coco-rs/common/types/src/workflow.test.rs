use pretty_assertions::assert_eq;

use super::*;

#[test]
fn test_resolve_absent_yields_medium_marked_default() {
    assert_eq!(
        ResolvedWorkflowSize::resolve(None),
        ResolvedWorkflowSize {
            size: WorkflowSizeGuideline::Medium,
            is_default: true,
        }
    );
}

#[test]
fn test_resolve_explicit_medium_is_not_default() {
    // The distinction is load-bearing: only an explicitly chosen guideline
    // drops the /config pointer from the sentence.
    assert_eq!(
        ResolvedWorkflowSize::resolve(Some(WorkflowSizeGuideline::Medium)),
        ResolvedWorkflowSize {
            size: WorkflowSizeGuideline::Medium,
            is_default: false,
        }
    );
}

#[test]
fn test_unrestricted_emits_no_tool_sentence() {
    let resolved = ResolvedWorkflowSize::resolve(Some(WorkflowSizeGuideline::Unrestricted));
    assert_eq!(resolved.tool_description_sentence(), "");
}

#[test]
fn test_default_sentence_names_the_default_and_the_escape_hatch() {
    assert_eq!(
        ResolvedWorkflowSize::resolve(None).tool_description_sentence(),
        "This session has the default workflow size guideline: medium — keep workflows under 15 \
         agents. This is a guideline, not a hard limit — follow it unless the user's prompt calls \
         for a different scale. The user can raise or remove it with \
         `/config workflowSizeGuideline <unrestricted|small|medium|large>`."
    );
}

#[test]
fn test_explicit_sentence_omits_the_escape_hatch() {
    assert_eq!(
        ResolvedWorkflowSize::resolve(Some(WorkflowSizeGuideline::Small))
            .tool_description_sentence(),
        "A workflow size guideline is configured for this session: small — keep workflows under 5 \
         agents. This is a guideline, not a hard limit — follow it unless the user's prompt calls \
         for a different scale."
    );
}

#[test]
fn test_change_notice_special_cases_unrestricted() {
    assert_eq!(
        ResolvedWorkflowSize::resolve(Some(WorkflowSizeGuideline::Unrestricted)).change_notice(),
        "Workflow size is now unrestricted — no size guideline applies."
    );
    assert_eq!(
        ResolvedWorkflowSize::resolve(Some(WorkflowSizeGuideline::Large)).change_notice(),
        "The workflow size guideline for this session changed: large — keep workflows under 50 \
         agents. This is a guideline, not a hard limit — follow it unless the user's prompt calls \
         for a different scale."
    );
}

#[test]
fn test_agent_caps_are_the_calibrated_ladder() {
    assert_eq!(WorkflowSizeGuideline::Unrestricted.agent_cap(), None);
    assert_eq!(WorkflowSizeGuideline::Small.agent_cap(), Some(5));
    assert_eq!(WorkflowSizeGuideline::Medium.agent_cap(), Some(15));
    assert_eq!(WorkflowSizeGuideline::Large.agent_cap(), Some(50));
}

#[test]
fn test_wire_form_is_snake_case() {
    assert_eq!(
        serde_json::to_value(WorkflowSizeGuideline::Unrestricted).expect("serialize"),
        serde_json::json!("unrestricted")
    );
    assert_eq!(
        serde_json::from_value::<WorkflowSizeGuideline>(serde_json::json!("large"))
            .expect("deserialize"),
        WorkflowSizeGuideline::Large
    );
}
