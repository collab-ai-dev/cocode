use super::*;

#[test]
fn human_user_gets_old_prompt() {
    let h = InitPromptHandler {
        user_type: UserType::Human,
        features: Features::with_defaults(),
        project_root: None,
    };
    assert_eq!(h.select_prompt(), OLD_INIT_PROMPT);
}

#[test]
fn ant_user_with_new_init_feature_gets_new_prompt() {
    let mut features = Features::empty();
    features.enable(Feature::NewInit);
    let h = InitPromptHandler {
        user_type: UserType::Ant,
        features,
        project_root: None,
    };
    assert_eq!(h.select_prompt(), NEW_INIT_PROMPT);
}

#[test]
fn ant_user_without_new_init_falls_back_to_old() {
    let h = InitPromptHandler {
        user_type: UserType::Ant,
        features: Features::empty(),
        project_root: None,
    };
    assert_eq!(h.select_prompt(), OLD_INIT_PROMPT);
}

#[tokio::test]
async fn execute_returns_prompt_variant() {
    let h = InitPromptHandler {
        user_type: UserType::Human,
        features: Features::with_defaults(),
        project_root: None,
    };
    match h.execute_command("").await.unwrap() {
        CommandResult::Prompt {
            progress_message,
            parts,
        } => {
            assert_eq!(progress_message, "analyzing your codebase");
            assert!(matches!(&parts[0], PromptPart::Text { .. }));
        }
        other => panic!("unexpected: {other:?}"),
    }
}
