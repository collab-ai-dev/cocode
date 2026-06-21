//! Input suggestions — command, file, and history suggestions.

/// A suggestion for the input prompt.
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub text: String,
    pub display: String,
    pub category: SuggestionCategory,
    pub score: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionCategory {
    Command,
    File,
    History,
    Symbol,
    Directory,
}

/// Get command suggestions based on partial input.
pub fn get_command_suggestions(prefix: &str, commands: &[&str]) -> Vec<Suggestion> {
    let lower = prefix.to_lowercase();
    commands
        .iter()
        .filter(|cmd| cmd.to_lowercase().starts_with(&lower))
        .map(|cmd| Suggestion {
            text: format!("/{cmd}"),
            display: format!("/{cmd}"),
            category: SuggestionCategory::Command,
            score: 1.0,
        })
        .collect()
}

/// Get file suggestions based on partial path.
pub fn get_file_suggestions(prefix: &str, cwd: &str, limit: usize) -> Vec<Suggestion> {
    let dir = if prefix.contains('/') {
        let parent = std::path::Path::new(prefix)
            .parent()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| cwd.to_string());
        if parent.starts_with('/') {
            parent
        } else {
            format!("{cwd}/{parent}")
        }
    } else {
        cwd.to_string()
    };

    let prefix_lower = prefix.to_lowercase();
    let mut suggestions = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.to_lowercase().starts_with(&prefix_lower) || prefix.is_empty() {
                let is_dir = entry.path().is_dir();
                suggestions.push(Suggestion {
                    text: if is_dir {
                        format!("{name}/")
                    } else {
                        name.clone()
                    },
                    display: if is_dir { format!("{name}/") } else { name },
                    category: if is_dir {
                        SuggestionCategory::Directory
                    } else {
                        SuggestionCategory::File
                    },
                    score: 0.8,
                });
                if suggestions.len() >= limit {
                    break;
                }
            }
        }
    }

    suggestions
}

#[cfg(test)]
#[path = "suggestions.test.rs"]
mod tests;
