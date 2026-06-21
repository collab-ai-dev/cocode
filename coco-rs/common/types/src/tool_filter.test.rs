use super::*;
use crate::ToolName;

fn id(name: ToolName) -> ToolId {
    ToolId::Builtin(name)
}

#[test]
fn none_permits_every_baseline_tool() {
    let overrides = ToolOverrides::none();
    assert!(overrides.permits(&id(ToolName::Read)));
    assert!(overrides.permits(&id(ToolName::Bash)));
    assert!(!overrides.is_extra(&id(ToolName::Read)));
}

#[test]
fn excluded_subtracts_from_baseline() {
    // gpt-5 doesn't accept Edit (uses apply_patch instead).
    let overrides = ToolOverrides::default().with_excluded(id(ToolName::Edit));
    assert!(!overrides.permits(&id(ToolName::Edit)));
    assert!(overrides.permits(&id(ToolName::Read)));
    assert!(overrides.permits(&id(ToolName::Bash)));
}

#[test]
fn extra_marks_model_specific_tools() {
    // gpt-5 adds apply_patch — now a typed ToolName variant.
    let overrides = ToolOverrides::default().with_extra(id(ToolName::ApplyPatch));
    assert!(overrides.is_extra(&id(ToolName::ApplyPatch)));
    assert!(!overrides.is_extra(&id(ToolName::Read)));
}

#[test]
fn excluded_wins_over_extra() {
    let overrides = ToolOverrides::default()
        .with_extra(ToolId::Custom("ambiguous_tool".into()))
        .with_excluded(ToolId::Custom("ambiguous_tool".into()));
    assert!(!overrides.permits(&ToolId::Custom("ambiguous_tool".into())));
    assert!(!overrides.is_extra(&ToolId::Custom("ambiguous_tool".into())));
}

#[test]
fn builder_compose() {
    let overrides = ToolOverrides::default()
        .with_extra(id(ToolName::ApplyPatch))
        .with_excluded(id(ToolName::Edit));
    assert!(overrides.is_extra(&id(ToolName::ApplyPatch)));
    assert!(overrides.permits(&id(ToolName::ApplyPatch)));
    assert!(!overrides.permits(&id(ToolName::Edit)));
}

#[test]
fn permits_name_parses_wire_format() {
    let overrides = ToolOverrides::default().with_excluded(id(ToolName::Edit));
    assert!(!overrides.permits_name("Edit"));
    assert!(overrides.permits_name("Read"));
    // Unknown names land in `Custom` and pass through (not excluded).
    assert!(overrides.permits_name("some_plugin_tool"));
}

#[test]
fn accepts_mcp_id() {
    let mcp = ToolId::Mcp {
        server: "slack".into(),
        tool: "send".into(),
    };
    let overrides = ToolOverrides::default().with_excluded(mcp.clone());
    assert!(!overrides.permits(&mcp));
    assert!(!overrides.permits_name("mcp__slack__send"));
}

#[test]
fn merge_layers_user_overrides_on_builtin() {
    // gpt-5 built-in: extra=apply_patch, excluded=Edit.
    let builtin = ToolOverrides::default()
        .with_extra(id(ToolName::ApplyPatch))
        .with_excluded(id(ToolName::Edit));
    // User additionally excludes Bash.
    let user = ToolOverrides::default().with_excluded(id(ToolName::Bash));

    let merged = builtin.merge(&user);
    assert!(merged.is_extra(&id(ToolName::ApplyPatch)));
    assert!(!merged.permits(&id(ToolName::Edit)));
    assert!(!merged.permits(&id(ToolName::Bash)));
    assert!(merged.permits(&id(ToolName::Read)));
}

#[test]
fn merge_user_excluded_wins_over_builtin_extra() {
    // Pathological: built-in adds a tool, user excludes it. User wins.
    let builtin = ToolOverrides::default().with_extra(id(ToolName::ApplyPatch));
    let user = ToolOverrides::default().with_excluded(id(ToolName::ApplyPatch));

    let merged = builtin.merge(&user);
    assert!(!merged.permits(&id(ToolName::ApplyPatch)));
    assert!(!merged.is_extra(&id(ToolName::ApplyPatch)));
}

#[test]
fn tool_filter_unrestricted_allows_all() {
    let f = ToolFilter::unrestricted();
    assert!(f.allows(&id(ToolName::Read)));
    assert!(f.allows(&id(ToolName::Bash)));
}

#[test]
fn tool_filter_empty_allowed_means_no_whitelist() {
    let f = ToolFilter::new(Vec::new(), Vec::new());
    assert!(f.allows(&id(ToolName::Read)));
}

#[test]
fn tool_filter_whitelist_restricts() {
    let f = ToolFilter::new(vec!["Read".into(), "Grep".into()], Vec::new());
    assert!(f.allows(&id(ToolName::Read)));
    assert!(f.allows(&id(ToolName::Grep)));
    assert!(!f.allows(&id(ToolName::Bash)));
}

#[test]
fn tool_filter_disallowed_overrides_whitelist() {
    let f = ToolFilter::new(vec!["Read".into(), "Bash".into()], vec!["Bash".into()]);
    assert!(f.allows(&id(ToolName::Read)));
    assert!(!f.allows(&id(ToolName::Bash)));
}

#[test]
fn tool_filter_disallowed_only() {
    let f = ToolFilter::new(Vec::new(), vec!["Write".into()]);
    assert!(f.allows(&id(ToolName::Read)));
    assert!(!f.allows(&id(ToolName::Write)));
}

#[test]
fn tool_filter_allows_name_parses_wire_format() {
    let f = ToolFilter::new(vec!["Read".into()], vec!["Bash".into()]);
    assert!(f.allows_name("Read"));
    assert!(!f.allows_name("Bash"));
    assert!(!f.allows_name("Edit"));
}

#[test]
fn tool_filter_handles_mcp_wire_format() {
    let f = ToolFilter::new(Vec::new(), vec!["mcp__slack__send".into()]);
    assert!(!f.allows_name("mcp__slack__send"));
    assert!(f.allows_name("Read"));
}

#[test]
fn narrow_with_parent_disallowed_carries_to_child() {
    // Parent disabled Bash; child allowed_tools include Bash. After
    // narrowing, Bash must still be denied.
    let parent = ToolFilter::new(Vec::new(), vec!["Bash".into()]);
    let child = ToolFilter::new(vec!["Read".into(), "Bash".into()], Vec::new());
    let narrowed = child.narrow_with(&parent);
    assert!(narrowed.allows(&id(ToolName::Read)));
    assert!(!narrowed.allows(&id(ToolName::Bash)));
}

#[test]
fn narrow_with_intersects_whitelists() {
    // Parent restricted to {Read, Bash}; child restricted to {Bash, Edit}.
    // Intersection = {Bash}.
    let parent = ToolFilter::new(vec!["Read".into(), "Bash".into()], Vec::new());
    let child = ToolFilter::new(vec!["Bash".into(), "Edit".into()], Vec::new());
    let narrowed = child.narrow_with(&parent);
    assert!(narrowed.allows(&id(ToolName::Bash)));
    assert!(!narrowed.allows(&id(ToolName::Read)));
    assert!(!narrowed.allows(&id(ToolName::Edit)));
}

#[test]
fn narrow_with_parent_only_keeps_parent_whitelist() {
    let parent = ToolFilter::new(vec!["Read".into()], Vec::new());
    let child = ToolFilter::unrestricted();
    let narrowed = child.narrow_with(&parent);
    assert!(narrowed.allows(&id(ToolName::Read)));
    assert!(!narrowed.allows(&id(ToolName::Bash)));
}

#[test]
fn narrow_with_both_unrestricted_stays_unrestricted() {
    let parent = ToolFilter::unrestricted();
    let child = ToolFilter::unrestricted();
    let narrowed = child.narrow_with(&parent);
    assert!(narrowed.allows(&id(ToolName::Read)));
    assert!(narrowed.allows(&id(ToolName::Bash)));
}

#[test]
fn narrow_with_unions_disallowed() {
    let parent = ToolFilter::new(Vec::new(), vec!["Write".into()]);
    let child = ToolFilter::new(Vec::new(), vec!["Bash".into()]);
    let narrowed = child.narrow_with(&parent);
    assert!(!narrowed.allows(&id(ToolName::Write)));
    assert!(!narrowed.allows(&id(ToolName::Bash)));
    assert!(narrowed.allows(&id(ToolName::Read)));
}

#[test]
fn write_edit_tool_default_to_native_for_claude() {
    // No override diff (Claude family): native Write/Edit.
    let overrides = ToolOverrides::none();
    assert_eq!(overrides.write_tool(), ToolName::Write);
    assert_eq!(overrides.edit_tool(), ToolName::Edit);
}

#[test]
fn write_edit_tool_resolve_to_apply_patch_for_gpt5() {
    // gpt-5 diff: Write/Edit excluded, apply_patch added → both map to it.
    let overrides = ToolOverrides::default()
        .with_extra(id(ToolName::ApplyPatch))
        .with_excluded(id(ToolName::Edit))
        .with_excluded(id(ToolName::Write));
    assert_eq!(overrides.write_tool(), ToolName::ApplyPatch);
    assert_eq!(overrides.edit_tool(), ToolName::ApplyPatch);
}

#[test]
fn write_tool_falls_back_to_native_when_excluded_without_apply_patch() {
    // Degenerate: Write excluded but no apply_patch — harmless native fallback.
    let overrides = ToolOverrides::default().with_excluded(id(ToolName::Write));
    assert_eq!(overrides.write_tool(), ToolName::Write);
    // Edit untouched → still Edit.
    assert_eq!(overrides.edit_tool(), ToolName::Edit);
}
