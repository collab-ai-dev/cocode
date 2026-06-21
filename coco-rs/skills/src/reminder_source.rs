//! `SkillsSource` impl on [`crate::SkillManager`].
//!
//! `listing()` renders the full skill catalog as a bullet list of
//! `- name: description` entries with each description capped at
//! [`MAX_LISTING_DESCRIPTION_CHARS`] (per-entry bound) and no skill ever
//! dropped. The full 1%-context-budget shrink-to-names-only path needs the
//! model context window threaded through `SkillsSource::listing` and stays
//! in `skills::generate_skill_tool_prompt` for the static system-prompt
//! injection.
//!
//! `invoked()` returns empty by default — tracking which skills were
//! invoked this session requires per-CLI / per-QueryEngine state that
//! is wired via a separate subsystem (follow-up work).

use async_trait::async_trait;
use coco_config::SkillOverrideTiers;
use coco_system_reminder::InvokedSkillEntry;
use coco_system_reminder::SkillsSource;
use coco_types::DynamicSkillPayload;
use coco_types::SkillDiscoveryPayload;
use coco_types::SkillDiscoverySkill;
use coco_types::SkillDiscoverySource;
use coco_types::SkillOverrideState;

use crate::SkillManager;
use crate::overrides::effective_skill_state;

const MAX_SKILL_DISCOVERY_DESCRIPTION_CHARS: usize = 500;

/// Per-entry description cap for the per-turn skill listing so one verbose
/// skill cannot bloat the reminder. Bundled-vs-budget shrink to names-only
/// (1% context budget) needs the model context window threaded through
/// `SkillsSource::listing` and remains a follow-up; capping each entry here
/// bounds the size without dropping any skill.
const MAX_LISTING_DESCRIPTION_CHARS: usize = 250;

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out = s.chars().take(max.saturating_sub(3)).collect::<String>();
    out.push_str("...");
    out
}

#[async_trait]
impl SkillsSource for SkillManager {
    async fn listing(&self, agent_id: Option<&str>, tiers: &SkillOverrideTiers) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        // Build the canonical sorted list once for stable order.
        // `SkillManager::all()` returns owned `Arc<SkillDefinition>`
        // so we keep the `Arc`s alive for the borrow.
        let owned_skills = self.all();

        // Apply the 4-state override gates:
        // - Off → skip entirely
        // - NameOnly → emit `- name` (no description)
        // - DMI && effective != On → skip (XG$)
        // - On / UserInvocableOnly → emit `- name[: desc]`
        let entries: Vec<(&str, Option<&str>)> = owned_skills
            .iter()
            .filter_map(|s| {
                let effective = effective_skill_state(s, tiers);
                if s.disable_model_invocation && effective != SkillOverrideState::On {
                    return None;
                }
                match effective {
                    SkillOverrideState::Off => None,
                    SkillOverrideState::NameOnly => Some((s.name.as_str(), None)),
                    SkillOverrideState::On | SkillOverrideState::UserInvocableOnly => {
                        Some((s.name.as_str(), Some(s.description.as_str())))
                    }
                }
            })
            .collect::<Vec<_>>();
        let mut entries = entries;
        entries.sort_by(|a, b| a.0.cmp(b.0));
        if entries.is_empty() {
            return None;
        }
        let names: Vec<&str> = entries.iter().map(|(n, _)| *n).collect();

        // Only announce skills the agent has not seen yet. Returns `None`
        // once everything is announced so subsequent turns skip the
        // redundant injection.
        let (delta, _is_initial) = self.take_unannounced_skills(agent_id, &names);
        if delta.is_empty() {
            return None;
        }
        let delta_set: std::collections::HashSet<&str> = delta.iter().map(String::as_str).collect();
        let body = entries
            .iter()
            .filter(|(name, _)| delta_set.contains(*name))
            .map(|(name, desc)| match desc {
                Some(d) if !d.is_empty() => {
                    format!(
                        "- {name}: {}",
                        truncate_chars(d, MAX_LISTING_DESCRIPTION_CHARS)
                    )
                }
                _ => format!("- {name}"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(body)
    }

    async fn invoked(&self, _agent_id: Option<&str>) -> Vec<InvokedSkillEntry> {
        // SkillManager doesn't track invocation state — that's per-session
        // tracking owned by the CLI / session layer. Return empty for now;
        // future work: plumb `InvokedSkillsTracker` through a separate
        // trait impl on a wrapper that combines `SkillManager` + tracker.
        Vec::new()
    }

    /// **Local-heuristic**: this is a local keyword-match heuristic, not an
    /// LLM-backed search call.
    /// Returns matches keyed by case-insensitive substring on name (split
    /// on `- _`) plus 5+-char word overlap with description and
    /// `when_to_use`. Sorted alphabetically and capped at 5.
    ///
    /// Signal is stamped `"local_keyword_match"` so downstream telemetry
    /// can tell the heuristic from a future LLM-backed producer. When the
    /// LLM-backed path lands,
    /// swap this implementation and update the signal value.
    async fn skill_discovery(
        &self,
        user_input: &str,
        tiers: &SkillOverrideTiers,
    ) -> Option<SkillDiscoveryPayload> {
        let needle = user_input.to_lowercase();
        let mut matches: Vec<_> = self
            .all()
            .into_iter()
            .filter(|s| !s.disabled)
            // `off`-overridden skills must not surface as
            // recommendations — the user explicitly hid them.
            .filter(|s| effective_skill_state(s, tiers) != SkillOverrideState::Off)
            .filter(|s| {
                let name = s.name.to_lowercase();
                let desc = s.description.to_lowercase();
                let when = s.when_to_use.as_deref().unwrap_or("").to_lowercase();
                !needle.trim().is_empty()
                    && (needle.contains(&name)
                        || name.split(['-', '_']).any(|part| needle.contains(part))
                        || desc.split_whitespace().any(|word| {
                            word.len() >= 5
                                && needle
                                    .contains(word.trim_matches(|c: char| !c.is_alphanumeric()))
                        })
                        || when.split_whitespace().any(|word| {
                            word.len() >= 5
                                && needle
                                    .contains(word.trim_matches(|c: char| !c.is_alphanumeric()))
                        }))
            })
            .collect();
        matches.sort_by(|a, b| a.name.cmp(&b.name));
        matches.truncate(5);
        if matches.is_empty() {
            return None;
        }
        let skills = matches
            .iter()
            .map(|s| SkillDiscoverySkill {
                name: s.name.clone(),
                description: truncate_chars(&s.description, MAX_SKILL_DISCOVERY_DESCRIPTION_CHARS),
                short_id: None,
            })
            .collect();
        Some(SkillDiscoveryPayload {
            skills,
            signal: "local_keyword_match".to_string(),
            source: SkillDiscoverySource::Native,
        })
    }

    async fn activate_skills_for_paths(
        &self,
        file_paths: &[std::path::PathBuf],
        cwd: &std::path::Path,
    ) -> Vec<String> {
        self.activate_for_paths(file_paths, cwd)
    }

    async fn load_dynamic_skill_dir(
        &self,
        skill_dir: &std::path::Path,
        cwd: &std::path::Path,
    ) -> Option<DynamicSkillPayload> {
        let mut skills = crate::discover_skills(&[skill_dir.to_path_buf()]);
        if skills.is_empty() {
            return None;
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        let skill_names = skills
            .iter()
            .filter(|s| !s.disabled)
            .map(|s| s.name.clone())
            .collect::<Vec<_>>();
        if skill_names.is_empty() {
            return None;
        }
        for skill in skills {
            self.register(skill);
        }
        let display_path = skill_dir
            .strip_prefix(cwd)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| skill_dir.display().to_string());
        Some(DynamicSkillPayload {
            skill_dir: skill_dir.display().to_string(),
            skill_names,
            display_path,
        })
    }
}

#[cfg(test)]
#[path = "reminder_source.test.rs"]
mod tests;
