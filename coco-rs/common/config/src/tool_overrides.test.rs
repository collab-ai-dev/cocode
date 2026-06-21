use super::*;

#[test]
fn unknown_model_is_none_without_overrides() {
    let overrides = resolve_tool_overrides("claude-opus-4-7", None);
    assert_eq!(overrides, ToolOverrides::none());
}

#[test]
fn gpt5_routes_file_mutation_through_apply_patch_regardless_of_provider() {
    // Function takes no `provider` — gpt-5 served by OpenAI direct,
    // Azure, or any compat gateway returns the same diff. Provider is
    // a routing concern, not a capability axis.
    let overrides = resolve_tool_overrides("gpt-5", None);
    assert!(overrides.is_extra(&ToolId::Builtin(ToolName::ApplyPatch)));
    // Both Edit and Write are excluded — apply_patch is the sole
    // file-mutation tool (its Add File hunk creates new files).
    assert!(!overrides.permits(&ToolId::Builtin(ToolName::Edit)));
    assert!(!overrides.permits(&ToolId::Builtin(ToolName::Write)));
}

#[test]
fn gpt5_variants_all_get_the_diff() {
    for model in ["gpt-5", "gpt-5-mini", "gpt-5-2025-01-01"] {
        let overrides = resolve_tool_overrides(model, None);
        assert!(
            overrides.is_extra(&ToolId::Builtin(ToolName::ApplyPatch)),
            "expected apply_patch for {model}"
        );
    }
}

#[test]
fn user_tool_overrides_layer_on_top_of_builtin() {
    // Built-in: gpt-5 excludes Edit + Write in favor of apply_patch.
    // User additionally excludes Bash.
    let info = ModelInfo {
        model_id: "gpt-5".into(),
        tool_overrides: Some(
            ToolOverrides::default().with_excluded(ToolId::Builtin(ToolName::Bash)),
        ),
        ..Default::default()
    };
    let overrides = resolve_tool_overrides("gpt-5", Some(&info));
    assert!(overrides.is_extra(&ToolId::Builtin(ToolName::ApplyPatch)));
    assert!(!overrides.permits(&ToolId::Builtin(ToolName::Edit)));
    assert!(!overrides.permits(&ToolId::Builtin(ToolName::Write)));
    assert!(!overrides.permits(&ToolId::Builtin(ToolName::Bash)));
    // Untouched baseline tools stay.
    assert!(overrides.permits(&ToolId::Builtin(ToolName::Read)));
}

#[test]
fn user_overrides_can_opt_custom_model_into_apply_patch() {
    // Custom finetune that the built-in registry doesn't know about
    // — user declares `tool_overrides` directly.
    let info = ModelInfo {
        model_id: "internal/custom-coder-v3".into(),
        tool_overrides: Some(
            ToolOverrides::default()
                .with_extra(ToolId::Builtin(ToolName::ApplyPatch))
                .with_excluded(ToolId::Builtin(ToolName::Edit)),
        ),
        ..Default::default()
    };
    let overrides = resolve_tool_overrides("internal/custom-coder-v3", Some(&info));
    assert!(overrides.is_extra(&ToolId::Builtin(ToolName::ApplyPatch)));
    assert!(!overrides.permits(&ToolId::Builtin(ToolName::Edit)));
}

#[test]
fn user_excluded_wins_over_builtin_extra() {
    // Pathological: built-in adds apply_patch, user excludes it. User wins.
    let info = ModelInfo {
        model_id: "gpt-5".into(),
        tool_overrides: Some(
            ToolOverrides::default().with_excluded(ToolId::Builtin(ToolName::ApplyPatch)),
        ),
        ..Default::default()
    };
    let overrides = resolve_tool_overrides("gpt-5", Some(&info));
    assert!(!overrides.permits(&ToolId::Builtin(ToolName::ApplyPatch)));
    assert!(!overrides.is_extra(&ToolId::Builtin(ToolName::ApplyPatch)));
}
