//! User-visible skill-learning notices — surfacing "Learned skill: X" after a
//! successful review fork.
//!
//! Mirrors `coco_memory::NoticeInbox`: the review fork (a detached background
//! task) pushes a [`SkillLearnNotice`] on success; the engine drains the inbox
//! at turn finalize and projects it into history as a user-visible
//! `SystemMessage` plus a model-visible `<system-reminder>`. Because the fork
//! is detached, a notice pushed after turn N's drain surfaces at a later turn's
//! finalize — the same latency as memory's `SystemMemorySavedMessage`.

use std::sync::Arc;
use std::sync::Mutex;

/// How a skill entered the timeline this fork.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillLearnVerb {
    /// A brand-new agent skill was created (quarantined until promoted).
    Learned,
    /// An existing agent skill (or its support files) was updated.
    Updated,
}

impl SkillLearnVerb {
    /// User-facing display verb.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Learned => "Learned",
            Self::Updated => "Improved",
        }
    }
}

/// One queued user-visible skill notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillLearnNotice {
    /// Skill name (the `.agent/<name>` directory basename).
    pub name: String,
    pub verb: SkillLearnVerb,
}

/// Append-only mailbox shared between the review fork and the engine drain
/// hook. Cheap to clone (`Arc` inside).
#[derive(Debug, Default, Clone)]
pub struct SkillLearnInbox {
    inner: Arc<Mutex<Vec<SkillLearnNotice>>>,
}

impl SkillLearnInbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a notice. Recovers from a poisoned mutex (a prior panic) rather
    /// than dropping the user-visible "learned skill" line.
    pub fn push(&self, notice: SkillLearnNotice) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(notice);
    }

    /// Take everything queued and clear the inbox (called once per turn at
    /// finalize). Deduplicates by name, keeping `Learned` over `Updated` when
    /// both fire for the same skill in one drain.
    pub fn drain(&self) -> Vec<SkillLearnNotice> {
        let raw = {
            let mut g = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *g)
        };
        let mut out: Vec<SkillLearnNotice> = Vec::new();
        for notice in raw {
            match out.iter_mut().find(|n| n.name == notice.name) {
                Some(existing) => {
                    // Learned outranks Updated for the same skill.
                    if notice.verb == SkillLearnVerb::Learned {
                        existing.verb = SkillLearnVerb::Learned;
                    }
                }
                None => out.push(notice),
            }
        }
        out
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("SkillLearnInbox mutex poisoned")
            .len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
#[path = "notice.test.rs"]
mod tests;
