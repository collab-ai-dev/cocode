//! Skill and command search autocomplete (/command mentions).
//!
//! Uses nucleo for fuzzy matching with weighted scoring.

use nucleo::Matcher;
use nucleo::Utf32String;
use nucleo::pattern::AtomKind;
use nucleo::pattern::CaseMatching;
use nucleo::pattern::Normalization;
use nucleo::pattern::Pattern;

use crate::widgets::suggestion_popup::SuggestionItem;

/// Events from skill search to TUI.
#[derive(Debug, Clone)]
pub enum SkillSearchEvent {
    /// Search results ready.
    SearchResult {
        query: String,
        /// Byte offset where the trigger started (sentinel).
        start_pos: usize,
        suggestions: Vec<SuggestionItem>,
    },
}

/// Where a searchable command/skill came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSourceTag {
    Builtin,
    Bundled,
    User,
    Project,
    Plugin,
    Mcp,
}

impl CommandSourceTag {
    /// Short annotation shown in autocomplete suggestions.
    fn annotation(self) -> Option<&'static str> {
        match self {
            CommandSourceTag::Builtin => None,
            CommandSourceTag::Bundled => Some("bundled"),
            CommandSourceTag::User => Some("user"),
            CommandSourceTag::Project => Some("project"),
            CommandSourceTag::Plugin => Some("plugin"),
            CommandSourceTag::Mcp => Some("mcp"),
        }
    }
}

/// A loaded skill/command definition for autocomplete.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub description: Option<String>,
    pub aliases: Vec<String>,
    pub source: CommandSourceTag,
    pub argument_hint: Option<String>,
}

/// Scored match result for internal sorting.
struct ScoredMatch {
    score: u32,
    info_idx: usize,
}

const MAX_SUGGESTIONS: usize = 15;

/// Manages skill/command search (synchronous, in-memory with nucleo fuzzy matching).
pub struct SkillSearchManager {
    skills: Vec<SkillInfo>,
}

impl SkillSearchManager {
    /// Create with a list of available skills/commands.
    pub fn new(skills: Vec<SkillInfo>) -> Self {
        Self { skills }
    }

    /// Create empty (skills loaded later).
    pub fn empty() -> Self {
        Self { skills: Vec::new() }
    }

    /// Update the skill list.
    pub fn set_skills(&mut self, skills: Vec<SkillInfo>) {
        self.skills = skills;
    }

    /// Search skills/commands matching the query using nucleo fuzzy matching
    /// with weighted keys (name: 3, parts: 2, aliases: 2, description: 0.5).
    pub fn search(&self, query: &str) -> Vec<SuggestionItem> {
        if query.is_empty() {
            return self
                .skills
                .iter()
                .take(MAX_SUGGESTIONS)
                .map(to_suggestion)
                .collect();
        }

        let pattern = Pattern::new(
            query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut matcher = Matcher::default();
        let mut matches: Vec<ScoredMatch> = Vec::new();

        for (idx, skill) in self.skills.iter().enumerate() {
            let best_score = best_match_score(&pattern, &mut matcher, skill);
            if best_score > 0 {
                matches.push(ScoredMatch {
                    score: best_score,
                    info_idx: idx,
                });
            }
        }

        // Sort by score descending, then name ascending for ties
        matches.sort_by(|a, b| {
            b.score.cmp(&a.score).then_with(|| {
                self.skills[a.info_idx]
                    .name
                    .cmp(&self.skills[b.info_idx].name)
            })
        });

        matches
            .into_iter()
            .take(MAX_SUGGESTIONS)
            .map(|m| to_suggestion(&self.skills[m.info_idx]))
            .collect()
    }
}

impl Default for SkillSearchManager {
    fn default() -> Self {
        Self::empty()
    }
}

/// Compute the best weighted score for a skill across all searchable fields.
fn best_match_score(pattern: &Pattern, matcher: &mut Matcher, skill: &SkillInfo) -> u32 {
    let mut best: u32 = 0;

    // Name: weight 3
    let name_haystack = Utf32String::from(skill.name.as_str());
    if let Some(score) = pattern.score(name_haystack.slice(..), matcher) {
        best = best.max(score.saturating_mul(3));
    }

    // Name parts (split by hyphens/colons): weight 2
    for part in skill.name.split(['-', ':', '_']) {
        let part_haystack = Utf32String::from(part);
        if let Some(score) = pattern.score(part_haystack.slice(..), matcher) {
            best = best.max(score.saturating_mul(2));
        }
    }

    // Aliases: weight 2
    for alias in &skill.aliases {
        let alias_haystack = Utf32String::from(alias.as_str());
        if let Some(score) = pattern.score(alias_haystack.slice(..), matcher) {
            best = best.max(score.saturating_mul(2));
        }
    }

    // Description: weight 0.5 (divide by 2)
    if let Some(desc) = &skill.description {
        let desc_haystack = Utf32String::from(desc.as_str());
        if let Some(score) = pattern.score(desc_haystack.slice(..), matcher) {
            best = best.max(score / 2);
        }
    }

    best
}

/// Convert a SkillInfo into a SuggestionItem for display.
fn to_suggestion(skill: &SkillInfo) -> SuggestionItem {
    let mut label = format!("/{}", skill.name);
    if let Some(hint) = &skill.argument_hint {
        label.push_str(&format!(" {hint}"));
    }

    let mut desc_parts = Vec::new();
    if let Some(desc) = &skill.description {
        desc_parts.push(desc.clone());
    }
    if let Some(annotation) = skill.source.annotation() {
        desc_parts.push(format!("({annotation})"));
    }

    SuggestionItem {
        label,
        description: if desc_parts.is_empty() {
            None
        } else {
            Some(desc_parts.join(" "))
        },
        metadata: None,
    }
}

#[cfg(test)]
#[path = "skill_search.test.rs"]
mod tests;
