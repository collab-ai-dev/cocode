//! The skill-review fork.
//!
//! Mirrors `coco-memory`'s `ExtractService::run`: after an eligible turn, fork
//! a sandboxed sub-agent that reviews the session and writes skill patches /
//! new agent-owned skills under the agent skills directory. Routing, fencing,
//! and cache-parity all reuse the same `coco-tool-runtime` spawn primitives:
//!
//! - `ModelRole::Memory` (via a synthetic `AgentDefinition`) so the operator's
//!   `settings.models.memory` steers this background self-improvement work —
//!   the same knob memory extraction/dream use.
//! - [`crate::fence::SkillWriteHandle`] as the inner-ring write fence, composed
//!   with `AgentSpawnConstraints.allowed_write_roots` (outer ring).
//! - `require_can_use_tool = true` so the fence runs even if a PreToolUse hook
//!   pre-approves a tool (higher-stakes than memory: skills are executable).
//! - `fork_context_messages` (not a spawn-mode variant) to give the child the
//!   parent's message slice — same as memory.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};

use coco_tool_runtime::{AgentSpawnConstraints, AgentSpawnRequest};
use coco_types::messages::Message;
use coco_types::{AgentDefinition, AgentTypeId, ForkLabel, ModelRole, SessionId};

use coco_skills::agent_scope::agent_skills_dir;

use crate::fence::create_skill_write_handle;

/// Shared swappable cell for the agent handle, so a delayed `install_agent`
/// at bootstrap propagates atomically (same cell memory's services use).
pub use coco_tool_runtime::AgentSlot;

/// Default per-fork turn cap for a skill review.
pub const DEFAULT_REVIEW_MAX_TURNS: i32 = 6;

/// Outcome of one review-fork run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillReviewOutcome {
    /// The fork ran; `paths_written` is how many files it touched.
    Completed { paths_written: usize },
    /// The fork failed to spawn / run.
    Failed { reason: String },
}

/// Spawns skill-review forks under the agent skills write fence.
pub struct SkillReviewService {
    agent: AgentSlot,
    agent_root: PathBuf,
    config_home: PathBuf,
}

impl SkillReviewService {
    /// Build a review service writing under `<config_home>/skills/.agent`.
    pub fn new(agent: AgentSlot, config_home: &Path) -> Self {
        Self {
            agent,
            agent_root: agent_skills_dir(config_home),
            config_home: config_home.to_path_buf(),
        }
    }

    /// Run one review fork over `fork_context` (the parent's message slice).
    pub async fn run(
        &self,
        session_id: SessionId,
        fork_context: Vec<Arc<Message>>,
    ) -> SkillReviewOutcome {
        // Ensure the fenced root exists so the fork's first write lands.
        if let Err(e) = std::fs::create_dir_all(&self.agent_root) {
            return SkillReviewOutcome::Failed {
                reason: format!("could not create agent skills dir: {e}"),
            };
        }

        let prompt = build_skill_review_prompt(&self.agent_root);

        // Synthetic definition pins `ModelRole::Memory` — background
        // self-improvement shares the memory role's model, not the (often
        // expensive, foreground) review role. Same pattern as memory.
        let def = Arc::new(AgentDefinition {
            agent_type: AgentTypeId::Custom("skill-review".into()),
            name: "skill-review".into(),
            model_role: Some(ModelRole::Memory),
            ..Default::default()
        });

        let request = AgentSpawnRequest {
            prompt,
            description: Some("skill review".into()),
            session_id: Some(session_id),
            subagent_type: Some(coco_types::SubagentType::GeneralPurpose.as_str().into()),
            definition: Some(def),
            isolation: None,
            fork_context_messages: fork_context,
            constraints: Some(AgentSpawnConstraints {
                max_turns: Some(DEFAULT_REVIEW_MAX_TURNS),
                allowed_write_roots: vec![self.agent_root.clone()],
            }),
            skip_transcript: true,
            can_use_tool: Some(create_skill_write_handle(self.agent_root.clone())),
            // Run the fence even under a hook Allow — skills are executable.
            require_can_use_tool: true,
            fork_label: Some(ForkLabel::SkillReview),
            ..Default::default()
        };

        // Clone the inner handle under the sync guard, drop it before await.
        let agent = self
            .agent
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match agent.spawn_agent(request).await {
            Ok(resp) => {
                let paths_written = resp.paths_written.len();
                // Trusted-code provenance stamp + patch telemetry over the
                // exact paths the spawn driver reported. The prompt asks the
                // LLM to write the provenance keys, but only this pass makes
                // them reliable (omission or injected `origin: user` is
                // corrected here).
                if paths_written > 0 {
                    let agent_root = self.agent_root.clone();
                    let config_home = self.config_home.clone();
                    let paths = resp.paths_written;
                    let now = chrono::Utc::now().to_rfc3339();
                    let stamped = tokio::task::spawn_blocking(move || {
                        crate::stamp::stamp_written_skills(&agent_root, &config_home, &paths, &now)
                    })
                    .await;
                    match stamped {
                        Ok(stamped) => tracing::debug!(
                            target: "coco_skill_learn::review",
                            paths_written,
                            stamped,
                            "review fork wrote skills"
                        ),
                        Err(e) => tracing::warn!(
                            target: "coco_skill_learn::review",
                            "provenance stamp task failed: {e}"
                        ),
                    }
                }
                SkillReviewOutcome::Completed { paths_written }
            }
            Err(reason) => SkillReviewOutcome::Failed { reason },
        }
    }
}

/// Build the skill-review prompt. `agent_root` is interpolated so the fork
/// knows where it may write.
fn build_skill_review_prompt(agent_root: &Path) -> String {
    let root = agent_root.display();
    format!(
        "You are a skill-review subagent running after a coding session. Review the \
conversation and decide whether a reusable *skill* should be created or updated so \
future sessions handle this class of task better.\n\
\n\
You may ONLY write under the agent skills directory: {root}\n\
Layout is MANDATORY: each skill is a directory — {root}/<skill-name>/SKILL.md \
(kebab-case name, `# Heading` + frontmatter), optionally with supporting reference \
files (.md/.txt) next to the SKILL.md. Files written anywhere else under the root \
are invisible to the loader.\n\
Skills elsewhere (user/project skills) are read-only to you — if one of them had a \
gap, you cannot edit it; capture the missing knowledge in an agent skill under your \
directory instead (pick a name that does not collide with the existing skill).\n\
\n\
Signals worth capturing, strongest first:\n\
- The user corrected you (or showed frustration) and the correction generalizes \
beyond this session.\n\
- Something took real trial-and-error and the answer is stable: exact commands, \
flags, workflow order, non-obvious gotchas.\n\
- A procedure recurred that a checklist would compress.\n\
\n\
Do NOT capture:\n\
- Environment-dependent failures or transient errors that resolved on retry.\n\
- Negative claims about tools ('X does not work') — they harden into refusals \
that outlive the bug that caused them.\n\
- One-off task narratives that will never recur.\n\
- Secrets of any kind (tokens, keys, credentials, private data).\n\
\n\
Preference order — do the FIRST that applies, then stop:\n\
1. UPDATE an agent skill in your directory that was used this session but had a gap.\n\
2. UPDATE an existing umbrella skill in your directory that covers this class of task.\n\
3. ADD a supporting reference file to an existing agent skill.\n\
4. CREATE a new umbrella skill for a genuinely reusable, class-level procedure.\n\
If two agent skills overlap, fold the content into one and note the overlap there — \
never create a third.\n\
\n\
Rules:\n\
- Prefer updating over creating; prefer doing nothing over persisting something \
below the capture bar. A real user correction usually IS worth persisting.\n\
- Include frontmatter `origin: agent`, `created-by: review`, and \
`created-at: <RFC3339 now>` on skills you create. New skills start quarantined \
(users can run them via /name; the model cannot auto-invoke them) and are \
promoted automatically once they prove useful.\n\
- NEVER include `shell:` or `hooks:` frontmatter — agent skills load inert.\n\
- Keep skills concise and class-level (an umbrella), not one-off.\n\
- If nothing meets the bar, do nothing and finish.\n"
    )
}

#[cfg(test)]
#[path = "review.test.rs"]
mod tests;
