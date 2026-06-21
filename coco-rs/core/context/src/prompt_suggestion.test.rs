use super::*;

#[test]
fn test_generate_with_git() {
    let ctx = SuggestionContext {
        has_git: true,
        git_has_uncommitted: true,
        ..Default::default()
    };
    let suggestions = generate_suggestions(&ctx);
    assert!(
        suggestions
            .iter()
            .any(|s| s.category == PromptSuggestionCategory::GitOperation)
    );
}

#[test]
fn test_generate_with_files() {
    let ctx = SuggestionContext {
        recent_files: vec!["src/main.rs".to_string()],
        ..Default::default()
    };
    let suggestions = generate_suggestions(&ctx);
    assert!(suggestions.iter().any(|s| s.text.contains("main.rs")));
}

#[test]
fn test_first_turn_onboarding() {
    let ctx = SuggestionContext {
        is_first_turn: true,
        has_claude_md: false,
        ..Default::default()
    };
    let suggestions = generate_suggestions(&ctx);
    assert!(suggestions.iter().any(|s| s.text.contains("CLAUDE.md")));
}

#[test]
fn test_error_recovery_suggestions() {
    let ctx = SuggestionContext {
        has_errors: true,
        has_test_failures: true,
        ..Default::default()
    };
    let suggestions = generate_suggestions(&ctx);
    assert!(
        suggestions
            .iter()
            .any(|s| s.text.contains("Fix the errors"))
    );
    assert!(suggestions.iter().any(|s| s.text.contains("failing tests")));
}

#[test]
fn test_pr_suggestion_on_feature_branch() {
    let ctx = SuggestionContext {
        has_git: true,
        git_branch: Some("feat/new-feature".to_string()),
        ..Default::default()
    };
    let suggestions = generate_suggestions(&ctx);
    assert!(suggestions.iter().any(|s| s.text.contains("PR")));
}

#[test]
fn test_suggestions_sorted_by_priority() {
    let ctx = SuggestionContext {
        has_git: true,
        is_first_turn: true,
        git_has_uncommitted: true,
        ..Default::default()
    };
    let suggestions = generate_suggestions(&ctx);
    for window in suggestions.windows(2) {
        assert!(window[0].priority <= window[1].priority);
    }
}
