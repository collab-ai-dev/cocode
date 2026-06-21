//! Prompt suggestions — context-aware suggestions for user input.

/// A prompt suggestion shown to the user.
#[derive(Debug, Clone)]
pub struct PromptSuggestion {
    pub text: String,
    pub description: String,
    pub category: PromptSuggestionCategory,
    pub priority: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSuggestionCategory {
    QuickAction,
    RecentFile,
    GitOperation,
    Configuration,
    Custom,
}

/// Context for generating suggestions.
#[derive(Debug, Clone, Default)]
pub struct SuggestionContext {
    pub cwd: String,
    pub has_git: bool,
    pub recent_files: Vec<String>,
    pub recent_commands: Vec<String>,
    pub has_claude_md: bool,
    pub is_first_turn: bool,
    pub has_errors: bool,
    pub has_test_failures: bool,
    pub git_has_uncommitted: bool,
    pub git_branch: Option<String>,
}

/// Generate suggestions based on current context.
///
/// Context-aware suggestions based on cwd, git state, recent files,
/// and session history.
pub fn generate_suggestions(ctx: &SuggestionContext) -> Vec<PromptSuggestion> {
    let mut suggestions = Vec::new();

    // First-turn suggestions (onboarding)
    if ctx.is_first_turn {
        if !ctx.has_claude_md {
            suggestions.push(PromptSuggestion {
                text: "Help me set up this project with a CLAUDE.md".into(),
                description: "Initialize project configuration".into(),
                category: PromptSuggestionCategory::Configuration,
                priority: 0,
            });
        }
        suggestions.push(PromptSuggestion {
            text: "Give me an overview of this codebase".into(),
            description: "Explore the project structure".into(),
            category: PromptSuggestionCategory::QuickAction,
            priority: 1,
        });
    }

    // Git-based suggestions
    if ctx.has_git {
        if ctx.git_has_uncommitted {
            suggestions.push(PromptSuggestion {
                text: "Review and commit my changes".into(),
                description: "Create a git commit with current changes".into(),
                category: PromptSuggestionCategory::GitOperation,
                priority: 2,
            });
            suggestions.push(PromptSuggestion {
                text: "What changed since the last commit?".into(),
                description: "Show recent changes".into(),
                category: PromptSuggestionCategory::GitOperation,
                priority: 3,
            });
        }
        if let Some(branch) = &ctx.git_branch
            && branch != "main"
            && branch != "master"
        {
            suggestions.push(PromptSuggestion {
                text: format!("Create a PR for the {branch} branch"),
                description: "Create a pull request".into(),
                category: PromptSuggestionCategory::GitOperation,
                priority: 4,
            });
        }
    }

    // Error recovery suggestions
    if ctx.has_errors {
        suggestions.push(PromptSuggestion {
            text: "Fix the errors in my code".into(),
            description: "Diagnose and fix compilation/runtime errors".into(),
            category: PromptSuggestionCategory::QuickAction,
            priority: 1,
        });
    }
    if ctx.has_test_failures {
        suggestions.push(PromptSuggestion {
            text: "Fix the failing tests".into(),
            description: "Diagnose and fix test failures".into(),
            category: PromptSuggestionCategory::QuickAction,
            priority: 1,
        });
    }

    // Recent file suggestions
    for file in ctx.recent_files.iter().take(3) {
        suggestions.push(PromptSuggestion {
            text: format!("What does {file} do?"),
            description: format!("Explain {file}"),
            category: PromptSuggestionCategory::RecentFile,
            priority: 5,
        });
    }

    // General quick actions (always available)
    suggestions.push(PromptSuggestion {
        text: "Find and fix any bugs in the codebase".into(),
        description: "Bug hunting".into(),
        category: PromptSuggestionCategory::QuickAction,
        priority: 10,
    });
    suggestions.push(PromptSuggestion {
        text: "Write tests for the recent changes".into(),
        description: "Add test coverage".into(),
        category: PromptSuggestionCategory::QuickAction,
        priority: 11,
    });
    suggestions.push(PromptSuggestion {
        text: "Refactor this code to improve readability".into(),
        description: "Code cleanup".into(),
        category: PromptSuggestionCategory::QuickAction,
        priority: 12,
    });

    suggestions.sort_by_key(|s| s.priority);
    suggestions
}

/// Legacy function signature for backward compatibility.
pub fn generate_suggestions_simple(
    _cwd: &str,
    has_git: bool,
    recent_files: &[String],
    _recent_commands: &[String],
) -> Vec<PromptSuggestion> {
    generate_suggestions(&SuggestionContext {
        has_git,
        recent_files: recent_files.to_vec(),
        is_first_turn: true,
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "prompt_suggestion.test.rs"]
mod tests;
