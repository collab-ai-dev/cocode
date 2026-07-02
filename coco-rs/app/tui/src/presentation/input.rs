//! Presentation view models for composer-adjacent input surfaces.

use crate::state::AppState;
use crate::widgets::suggestion_popup::SuggestionItem;

/// Borrowed view of the active suggestion popup — built twice per frame
/// (sizing + paint pass), so it must not deep-clone the item list.
#[derive(Debug, Clone, Copy)]
pub(crate) struct InlinePopupView<'a> {
    pub(crate) items: &'a [SuggestionItem],
    pub(crate) selected: usize,
}

/// `Some` while the completion session is visibly in progress — including
/// frames where the filtered list is momentarily empty (overshot slash
/// filter, async result gap), PROVIDED the session has already shown rows
/// (`CompletionState::had_items`). The viewport keeps the popup slot reserved
/// on those frames (the widget renders a dim "no matches" row), so the
/// composer does not collapse-and-reopen mid-typing. A session that has never
/// matched anything stays `None` — heuristic trigger false-positives (bash
/// tokens containing `/`, prose `@word`, a `/usr/...` path typed as a
/// message) must not materialize a placeholder panel.
pub(crate) fn inline_popup_view(state: &AppState) -> Option<InlinePopupView<'_>> {
    if state.ui.interaction.active_prompt.is_some() {
        return None;
    }
    let popup = state.ui.interaction.popup.as_ref()?;
    let suggestions = state.ui.completion.active.as_ref()?;
    if !popup_matches_suggestions(popup.kind(), suggestions.kind)
        || (suggestions.items.is_empty() && !state.ui.completion.had_items)
    {
        return None;
    }
    Some(InlinePopupView {
        items: &suggestions.items,
        selected: suggestions.selected,
    })
}

fn popup_matches_suggestions(
    popup: crate::state::SuggestionKind,
    suggestions: crate::state::SuggestionKind,
) -> bool {
    popup == suggestions
        || matches!(
            (popup, suggestions),
            (
                crate::state::SuggestionKind::At,
                crate::state::SuggestionKind::Path
                    | crate::state::SuggestionKind::BashPath
                    | crate::state::SuggestionKind::Directory
                    | crate::state::SuggestionKind::CustomTitle
            )
        )
}

#[cfg(test)]
#[path = "input.test.rs"]
mod tests;
