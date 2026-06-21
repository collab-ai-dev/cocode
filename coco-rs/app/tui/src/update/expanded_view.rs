//! `app:toggleTodos` handler — cycle the right-rail expanded view.
//!
//! Three-state cycle when there are running teammates, two-state otherwise:
//!
//! * has running teammates: `None → Tasks → Teammates → None`
//! * else: `None ↔ Tasks`
//!
//! The Teammates branch is skipped when `subagents` is empty so the
//! cycle doesn't waste a keystroke landing on an empty pane.

use coco_types::ExpandedView;

use crate::state::AppState;
use crate::state::session::SubagentStatus;

/// Cycle the right-rail expanded view per the rules described above.
pub(super) fn cycle(state: &mut AppState) {
    let has_running_teammates = state
        .session
        .subagents
        .iter()
        .any(|s| matches!(s.status, SubagentStatus::Running));

    state.session.expanded_view = next(state.session.expanded_view, has_running_teammates);
}

/// Pure cycle function exposed for testing.
pub(super) fn next(current: ExpandedView, has_running_teammates: bool) -> ExpandedView {
    if has_running_teammates {
        match current {
            ExpandedView::None => ExpandedView::Tasks,
            ExpandedView::Tasks => ExpandedView::Teammates,
            ExpandedView::Teammates => ExpandedView::None,
        }
    } else {
        match current {
            ExpandedView::Tasks => ExpandedView::None,
            // `Teammates → None` even when no teammates are running so
            // a stale Teammates state (e.g. teammate finished while
            // panel was open) collapses cleanly on first press.
            ExpandedView::None | ExpandedView::Teammates => ExpandedView::Tasks,
        }
    }
}

#[cfg(test)]
#[path = "expanded_view.test.rs"]
mod tests;
