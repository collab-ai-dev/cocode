use super::*;

#[tokio::test]
async fn outside_repo_returns_text() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let result = CommitPushPrHandler::with_cwd(tmp.path().to_path_buf())
        .execute_command("")
        .await
        .unwrap();
    match result {
        CommandResult::Text(s) => assert!(s.contains("Not a git repository")),
        other => panic!("expected Text outside repo, got {other:?}"),
    }
}

#[tokio::test]
async fn in_repo_substitutes_context_and_uses_default_branch_fallback() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path();

    // Init a bare repo with no remote — `symbolic-ref refs/remotes/origin/HEAD`
    // fails, so we exercise the `init.defaultBranch` fallback path.
    let init = tokio::process::Command::new("git")
        .args(["init", "-q", "-b", "trunk"])
        .current_dir(path)
        .status()
        .await
        .expect("git init");
    if !init.success() {
        return; // git not available
    }
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.email", "t@example.com"])
        .current_dir(path)
        .status()
        .await;
    let _ = tokio::process::Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .status()
        .await;
    let _ = tokio::process::Command::new("git")
        .args(["config", "init.defaultBranch", "trunk"])
        .current_dir(path)
        .status()
        .await;

    let result = CommitPushPrHandler::with_cwd(path.to_path_buf())
        .execute_command("focus on the parser changes")
        .await
        .unwrap();

    match result {
        CommandResult::Prompt { parts, .. } => {
            let text = match parts.into_iter().next().expect("part") {
                PromptPart::Text { text } => text,
                other => panic!("expected text part, got {other:?}"),
            };
            // Substitutions applied — the !`...` markers must be gone.
            assert!(!text.contains("!`git status`"));
            assert!(!text.contains("!`git diff HEAD`"));
            assert!(!text.contains("!`gh pr view"));
            // Default-branch placeholder is resolved.
            assert!(!text.contains("{{DEFAULT_BRANCH}}"));
            assert!(
                text.contains("trunk"),
                "expected trunk to surface as default branch"
            );
            // Whoami placeholder is resolved.
            assert!(!text.contains("{{WHOAMI}}"));
            // User guidance appended.
            assert!(text.contains("Additional instructions from user"));
            assert!(text.contains("focus on the parser changes"));
        }
        other => panic!("expected Prompt, got {other:?}"),
    }
}
