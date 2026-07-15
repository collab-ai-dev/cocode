//! Session plan artifact binding for goals (design §5.5).
//!
//! `SessionPlanSource` resolves a goal's [`GoalPlanRef`] to a bounded
//! [`GoalPlanView`] by reading the session's single plan file — path, digest,
//! headings, and unchecked steps — never the full body. It is the lean
//! `PlanArtifactService` seam: the goal store binds by an opaque artifact id, the
//! session owns the path. A temporarily-unreadable plan degrades to a
//! path-only view rather than suppressing the whole goal-context reminder.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use coco_goal_runtime::{GoalPlanView, PlanSource, Result};
use coco_goals::{ContentDigest, GoalPlanRef, PlanArtifactId, PlanRevision, Timestamp};

/// Bounded caps so a large plan never bloats the per-turn reminder (§5.5).
const MAX_HEADINGS: usize = 12;
const MAX_ACTIVE_STEPS: usize = 20;

/// The stable artifact id for a session's single plan file.
pub fn session_plan_artifact_id(session_id: &str) -> PlanArtifactId {
    PlanArtifactId::new(format!("plan-{session_id}"))
}

/// A `GoalPlanRef` binding the session plan file at its current revision/digest,
/// or `None` when no plan file exists yet at goal-creation time.
pub fn current_plan_ref(
    session_id: &str,
    plan_path: &std::path::Path,
    at: Timestamp,
) -> Option<GoalPlanRef> {
    let content = std::fs::read_to_string(plan_path).ok()?;
    Some(GoalPlanRef {
        artifact_id: session_plan_artifact_id(session_id),
        revision: PlanRevision::INITIAL,
        content_digest: Some(content_digest(&content)),
        observed_at: at,
    })
}

/// Non-cryptographic content digest — detects change/stale excerpts only, not a
/// security boundary (§5.5). Deterministic across runs (`DefaultHasher` fixed keys).
fn content_digest(content: &str) -> ContentDigest {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    ContentDigest::new(format!("{:016x}", hasher.finish()))
}

fn extract_headings(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            trimmed
                .starts_with('#')
                .then(|| trimmed.trim_start_matches('#').trim().to_string())
        })
        .filter(|heading| !heading.is_empty())
        .take(MAX_HEADINGS)
        .collect()
}

/// Unchecked Markdown checkbox items (`- [ ] …`) — the active steps.
fn extract_active_steps(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            let rest = trimmed
                .strip_prefix("- [ ]")
                .or_else(|| trimmed.strip_prefix("* [ ]"))?;
            let step = rest.trim();
            (!step.is_empty()).then(|| step.to_string())
        })
        .take(MAX_ACTIVE_STEPS)
        .collect()
}

/// Resolves a goal's plan reference against the session plan file.
pub struct SessionPlanSource {
    plan_path: PathBuf,
}

impl SessionPlanSource {
    pub fn new(plan_path: PathBuf) -> Self {
        Self { plan_path }
    }
}

impl PlanSource for SessionPlanSource {
    fn plan_view(&self, plan_ref: &GoalPlanRef) -> Result<Option<GoalPlanView>> {
        let display_path = self.plan_path.display().to_string();
        match std::fs::read_to_string(&self.plan_path) {
            Ok(content) => {
                let digest = content_digest(&content);
                // Drift: the current file digest differs from the bound one.
                let drifted = plan_ref
                    .content_digest
                    .as_ref()
                    .is_some_and(|bound| bound != &digest);
                Ok(Some(GoalPlanView {
                    artifact_id: plan_ref.artifact_id.clone(),
                    revision: plan_ref.revision,
                    display_path,
                    digest: Some(digest),
                    headings: extract_headings(&content),
                    active_steps: extract_active_steps(&content),
                    drifted,
                }))
            }
            // A temporarily-unreadable plan degrades to a path-only view (§5.5:
            // surface a warning, never silently drop the goal context). The
            // reminder still fires with the objective/budget.
            Err(_) => Ok(Some(GoalPlanView {
                artifact_id: plan_ref.artifact_id.clone(),
                revision: plan_ref.revision,
                display_path,
                digest: None,
                headings: Vec::new(),
                active_steps: Vec::new(),
                drifted: false,
            })),
        }
    }
}

#[cfg(test)]
#[path = "goal_plan.test.rs"]
mod tests;
