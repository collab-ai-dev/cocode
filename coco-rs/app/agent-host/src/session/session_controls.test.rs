use super::*;

#[test]
fn parse_permissions_mutation_keeps_read_only_args_as_none() {
    assert_eq!(parse_permissions_mutation(""), None);
    assert_eq!(parse_permissions_mutation("list"), None);
    assert_eq!(parse_permissions_mutation("allow"), None);
    assert_eq!(parse_permissions_mutation("deny "), None);
}

#[test]
fn permission_mutation_action_builds_session_allow_rule() {
    let Some(PermissionMutationAction::Apply {
        update,
        confirmation,
    }) = permission_mutation_action("allow Bash")
    else {
        panic!("expected apply action");
    };
    let coco_types::PermissionUpdate::AddRules { rules, destination } = update else {
        panic!("expected add-rules update");
    };
    assert_eq!(
        destination,
        coco_types::PermissionUpdateDestination::Session
    );
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].source, coco_types::PermissionRuleSource::Session);
    assert_eq!(rules[0].behavior, coco_types::PermissionBehavior::Allow);
    assert_eq!(rules[0].value.tool_pattern, "Bash");
    assert!(confirmation.contains("Added allow rule"));
}

#[test]
fn permission_mutation_action_builds_session_deny_rule() {
    let Some(PermissionMutationAction::Apply {
        update,
        confirmation,
    }) = permission_mutation_action("deny Write")
    else {
        panic!("expected apply action");
    };
    let coco_types::PermissionUpdate::AddRules { rules, destination } = update else {
        panic!("expected add-rules update");
    };
    assert_eq!(
        destination,
        coco_types::PermissionUpdateDestination::Session
    );
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0].source, coco_types::PermissionRuleSource::Session);
    assert_eq!(rules[0].behavior, coco_types::PermissionBehavior::Deny);
    assert_eq!(rules[0].value.tool_pattern, "Write");
    assert!(confirmation.contains("Added deny rule"));
}

#[test]
fn permission_mutation_action_builds_reset_confirmation() {
    let Some(PermissionMutationAction::Reset { confirmation }) =
        permission_mutation_action(" reset ")
    else {
        panic!("expected reset action");
    };
    assert!(confirmation.contains("Session permission rules reset"));
    assert!(confirmation.contains(coco_utils_common::COCO_CONFIG_DIR_NAME));
}
