use super::*;

#[test]
fn goal_display_args_matches_request_variant() {
    assert_eq!(
        goal_display_args(&coco_commands::GoalCommandRequest::Status),
        ""
    );
    assert_eq!(
        goal_display_args(&coco_commands::GoalCommandRequest::Clear),
        "clear"
    );
    assert_eq!(
        goal_display_args(&coco_commands::GoalCommandRequest::Set {
            condition: "ship it".to_string(),
        }),
        "ship it"
    );
}

#[test]
fn goal_status_sentinel_marks_sentinel_and_condition() {
    let payload = goal_status_sentinel(/*met*/ true, "finish migration".to_string());
    assert!(payload.sentinel);
    assert!(payload.met);
    assert_eq!(payload.condition, "finish migration");
}

#[test]
fn build_goal_kickoff_prompt_embeds_condition() {
    let prompt = build_goal_kickoff_prompt("all tests pass");
    assert!(prompt.contains("all tests pass"));
    assert!(prompt.contains("report_goal_turn"));
}
