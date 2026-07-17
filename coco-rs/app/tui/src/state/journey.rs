//! `/journey` overlay state — the learning-timeline view.
//!
//! Copies the `PermissionsEditor` shape: a dedicated TUI-local context (so
//! `j`/`k` navigation isn't eaten by a generic picker filter), a list mode with
//! a detail sub-mode, and (with the mutations work package) a memory-delete
//! confirm sub-mode. The payload is host-assembled (`coco-journey`); this state
//! only tracks selection + mode.

use coco_types::{
    JourneyDialogPayload, JourneyNodeBodyWire, JourneyNodeWire, JourneyStatsWire,
    TimelineBucketWire,
};

#[derive(Debug, Clone)]
pub struct JourneyState {
    /// Pre-computed timeline rows (host does not re-bucket on resize in v1).
    pub buckets: Vec<TimelineBucketWire>,
    /// List rows, ascending `last_activity_ms`.
    pub nodes: Vec<JourneyNodeWire>,
    pub stats: JourneyStatsWire,
    pub selected: usize,
    pub mode: JourneyMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JourneyMode {
    List,
    /// `Enter`: full description + telemetry + event history for the selection.
    Detail,
    /// Memory deletion confirm (irreversible). Skill retire/restore is
    /// immediate, so only memory delete needs a confirm. Defaults to No.
    DeleteMemoryConfirm {
        yes_selected: bool,
    },
}

impl JourneyState {
    pub fn from_wire(payload: JourneyDialogPayload) -> Self {
        Self {
            buckets: payload.buckets,
            nodes: payload.nodes,
            stats: payload.stats,
            selected: 0,
            mode: JourneyMode::List,
        }
    }

    /// Rebuild from a refreshed payload, preserving the selection by node
    /// identity when possible (falls back to a clamped index).
    pub fn refresh_from_wire(&mut self, payload: JourneyDialogPayload) {
        let prev_key = self.selected_node().map(node_identity);
        self.buckets = payload.buckets;
        self.nodes = payload.nodes;
        self.stats = payload.stats;
        self.selected = prev_key
            .and_then(|key| self.nodes.iter().position(|n| node_identity(n) == key))
            .unwrap_or_else(|| self.selected.min(self.nodes.len().saturating_sub(1)));
        // A refresh drops any open detail / confirm sub-mode.
        self.mode = JourneyMode::List;
    }

    pub fn selected_node(&self) -> Option<&JourneyNodeWire> {
        self.nodes.get(self.selected)
    }

    /// Clamp-only navigation (no wrap), matching the other overlays.
    pub fn nav(&mut self, delta: i32) {
        if self.nodes.is_empty() {
            self.selected = 0;
            return;
        }
        let max = self.nodes.len() - 1;
        let next = (self.selected as i32 + delta).clamp(0, max as i32);
        self.selected = next as usize;
    }
}

/// Stable identity for selection preservation across a refresh: the backing
/// path (skills) or filename (memories).
fn node_identity(node: &JourneyNodeWire) -> String {
    match &node.body {
        JourneyNodeBodyWire::AgentSkill { path, .. }
        | JourneyNodeBodyWire::UserSkill { path, .. } => path.clone(),
        JourneyNodeBodyWire::Memory { filename } => filename.clone(),
    }
}

#[cfg(test)]
#[path = "journey.test.rs"]
mod tests;
