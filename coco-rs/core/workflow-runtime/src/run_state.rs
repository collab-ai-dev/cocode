//! Per-run governance shared by a workflow and every child `workflow()` it
//! re-enters: the lifetime `agent()` ordinal, the interned phase table, and the
//! resume-replay cursor.
//!
//! These three counters must be **run-scoped, not engine-scoped**. A nested
//! `workflow()` re-enters [`WorkflowEngine::run`](crate::WorkflowEngine::run)
//! with a fresh JS context; if the counters lived in that context a child would
//! restart the agent ordinal (bypassing the lifetime cap and colliding with the
//! parent's progress rows), restart phase numbering, and reset the replay
//! cursor. Carrying one `Arc<WorkflowRunState>` from parent to child is what
//! makes the child share them — the same way the shared `WorkflowHost` Arc is
//! what shares the concurrency semaphore, token budget, journal and abort token.
//!
//! `Send + Sync` (atomics + `Mutex`) purely so an `Arc` can be parked on the
//! host; every access happens on the single engine thread.

use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicI32;
use std::sync::atomic::Ordering;

/// Prefix marking a phase group as a nested `workflow()` rather than a phase
/// the script declared.
pub const CHILD_GROUP_MARKER: &str = "▸";

/// A phase title's resolved slot.
pub struct PhaseSlot {
    /// 1-based phase index. `0` is never assigned — it is reserved for the
    /// renderer's ungrouped "Agents" fallback, so it can never collide with a
    /// declared phase.
    pub index: i32,
    /// `true` the first time this title is interned. The caller emits the
    /// `workflow_phase` progress node only then, so re-entering a phase does
    /// not duplicate the group.
    pub is_new: bool,
}

/// Counters shared by a run and its nested child workflows.
#[derive(Debug)]
pub struct WorkflowRunState {
    /// Next `agent()` ordinal. Advanced once per call, in call order, from the
    /// synchronous prologue of the `agent()` host function.
    next_agent_index: AtomicI32,
    /// Interned phase titles; a title's index is its position + 1.
    phases: Mutex<Vec<String>>,
    /// Resume-replay cursor: once a lookup misses, the run has diverged from
    /// the journalled prefix and later calls stop consulting the cache.
    replay_diverged: AtomicBool,
    /// How many times each child workflow name has been entered, so a script
    /// that calls the same child twice gets two distinct groups.
    child_group_counts: Mutex<Vec<(String, i32)>>,
}

impl WorkflowRunState {
    /// Fresh state that pre-interns `seed_phases` — the `meta.phases[].title`
    /// list. Declaring them up front gives them indices `1..=N` before any
    /// agent runs, so the progress tree renders its full skeleton immediately
    /// instead of growing it. Duplicate and blank titles are dropped.
    pub fn new<I, S>(seed_phases: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut phases: Vec<String> = Vec::new();
        for title in seed_phases {
            let title = title.into();
            if title.is_empty() || phases.contains(&title) {
                continue;
            }
            phases.push(title);
        }
        Self {
            next_agent_index: AtomicI32::new(0),
            phases: Mutex::new(phases),
            replay_diverged: AtomicBool::new(false),
            child_group_counts: Mutex::new(Vec::new()),
        }
    }

    /// The phase title every agent of one `workflow(name)` invocation is pinned
    /// to: `▸ name`, or `▸ name #2` for the second entry of the same name.
    ///
    /// A child runs under exactly one group. Letting its agents scatter into the
    /// parent's phases — or letting its own `phase()` calls open sibling
    /// top-level groups — would make a script's progress tree depend on whether
    /// it happened to be invoked directly or as a child, which is precisely the
    /// substitutability the `workflow()` primitive promises.
    pub fn next_child_group(&self, name: &str) -> String {
        let Ok(mut counts) = self.child_group_counts.lock() else {
            return format!("{CHILD_GROUP_MARKER} {name}");
        };
        let count = match counts.iter_mut().find(|(known, _)| known == name) {
            Some((_, count)) => {
                *count += 1;
                *count
            }
            None => {
                counts.push((name.to_string(), 1));
                1
            }
        };
        if count > 1 {
            format!("{CHILD_GROUP_MARKER} {name} #{count}")
        } else {
            format!("{CHILD_GROUP_MARKER} {name}")
        }
    }

    /// Reserve the next 0-based `agent()` ordinal for the whole run.
    pub fn next_agent_index(&self) -> i32 {
        self.next_agent_index.fetch_add(1, Ordering::Relaxed)
    }

    /// Intern `title`, returning its 1-based index and whether this call
    /// created it. Titles are matched **exactly** — no trimming, no case
    /// folding — which is what makes `meta.phases`, `phase()` and
    /// `agent({phase})` converge on one group when they spell the title the
    /// same way.
    pub fn resolve_phase(&self, title: &str) -> PhaseSlot {
        let Ok(mut phases) = self.phases.lock() else {
            // Poisoned only if a previous holder panicked while holding it;
            // degrade to the ungrouped bucket rather than propagating.
            return PhaseSlot {
                index: 0,
                is_new: false,
            };
        };
        if let Some(position) = phases.iter().position(|known| known == title) {
            return PhaseSlot {
                index: position as i32 + 1,
                is_new: false,
            };
        }
        phases.push(title.to_string());
        PhaseSlot {
            index: phases.len() as i32,
            is_new: true,
        }
    }

    /// Whether the replay cursor may still serve journalled results.
    pub fn replay_allowed(&self) -> bool {
        !self.replay_diverged.load(Ordering::Relaxed)
    }

    /// Latch the cursor as diverged. One-way — nothing re-establishes a prefix,
    /// so every `agent()` call that starts after this re-spawns.
    ///
    /// A call that already passed [`Self::replay_allowed`] before the latch
    /// flipped still consults the journal. That is sound here: keys are
    /// positional, so a hit means *this* call site completed last run, not that
    /// some other call's result is being reused.
    pub fn mark_replay_diverged(&self) {
        self.replay_diverged.store(true, Ordering::Relaxed);
    }
}

impl Default for WorkflowRunState {
    fn default() -> Self {
        Self::new(Vec::<String>::new())
    }
}

#[cfg(test)]
#[path = "run_state.test.rs"]
mod tests;
