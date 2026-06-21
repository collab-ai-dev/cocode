use super::*;

#[tokio::test]
async fn test_noop_handle_returns_unavailable_error() {
    let h = NoOpSkillHandle;
    let err = h
        .invoke_skill(
            "any_skill",
            "",
            SubagentInheritance::default(),
            SkillGateContext::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, SkillInvocationError::Unavailable { .. }));
    // Error formats cleanly for model-visible tool_result text.
    let rendered = err.to_string();
    assert!(rendered.contains("no skill runtime"));
}

#[test]
fn test_error_variants_all_format_cleanly() {
    let cases = [
        SkillInvocationError::NotFound { name: "foo".into() },
        SkillInvocationError::Disabled { name: "foo".into() },
        SkillInvocationError::HiddenFromModel { name: "foo".into() },
        SkillInvocationError::OverrideOff { name: "foo".into() },
        SkillInvocationError::OverrideUserOnlyNoTrigger { name: "foo".into() },
        SkillInvocationError::Expansion {
            name: "foo".into(),
            reason: "bad arg".into(),
        },
        SkillInvocationError::Forked {
            reason: "child failed".into(),
        },
        SkillInvocationError::RemoteUnsupported,
        SkillInvocationError::Unavailable {
            reason: "no runtime".into(),
        },
    ];
    for err in &cases {
        // Every variant must produce a non-empty rendered string
        // — synthesized tool_result text depends on this.
        assert!(!err.to_string().is_empty(), "empty error text: {err:?}");
    }
}
