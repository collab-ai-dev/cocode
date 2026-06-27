use super::*;

#[tokio::test]
async fn handler_outside_repo_returns_text_not_prompt() {
    // Use a tempdir with no git repo. cwd is overridden via with_cwd to
    // avoid mutating process-wide state (which would race with other tests).
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = CommitPromptHandler::with_cwd(tmp.path().to_path_buf())
        .execute_command("")
        .await
        .unwrap();

    match result {
        CommandResult::Text(s) => assert!(s.contains("Not a git repository")),
        other => panic!("expected Text outside repo, got {other:?}"),
    }
}

#[tokio::test]
async fn handler_in_repo_emits_prompt_with_status_substitution() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path();

    let init = tokio::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(path)
        .status()
        .await
        .expect("git init");
    if !init.success() {
        // Skip if git isn't available in the test environment.
        return;
    }
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(path)
        .status()
        .await;
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .status()
        .await;

    let result = CommitPromptHandler::with_cwd(path.to_path_buf())
        .execute_command("with extra note")
        .await
        .unwrap();

    match result {
        CommandResult::Prompt { parts, .. } => {
            let text = match parts.into_iter().next().expect("part") {
                PromptPart::Text { text } => text,
                other => panic!("expected text part, got {other:?}"),
            };
            // The !`...` markers must have been replaced.
            assert!(!text.contains("!`git status`"));
            assert!(!text.contains("!`git diff HEAD`"));
            // The user's extra guidance is appended.
            assert!(text.contains("Additional guidance"));
            assert!(text.contains("with extra note"));
        }
        other => panic!("expected Prompt in a git repo, got {other:?}"),
    }
}
