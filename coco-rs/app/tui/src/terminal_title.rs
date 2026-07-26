//! Terminal window/tab title composition.
//!
//! The title is the only part of coco visible when its terminal is not the
//! focused one, which makes it a different surface from the status bar rather
//! than a smaller copy of it. It answers one question — *does this tab want
//! me?* — so the run state leads and everything else is context for it.
//!
//! Writing is rate-limited by equality, not by time: the driver keeps the last
//! applied title and re-writes only on change. Terminals redraw a tab label on
//! every OSC 2, and a title that repaints on every frame flickers in the tab bar
//! of every terminal that animates the change.

use coco_config::settings::TerminalTitleItem;
use coco_tui_ui::terminal_title::SetTerminalTitleResult;
use coco_tui_ui::terminal_title::clear_terminal_title;
use coco_tui_ui::terminal_title::set_terminal_title;
use coco_types::ModelRole;

use crate::presentation::context_usage::render_context_usage;
use crate::state::AppState;

/// Marker on the run state when the session is blocked on the user. A leading
/// glyph rather than a word: it survives the truncation a narrow tab applies.
const ATTENTION_MARKER: &str = "\u{25cf} ";

/// Owns the "what is currently on the terminal title" fact.
///
/// Single owner on purpose — a second cache would let two writers disagree
/// about whether the title is coco's to clear, and clearing someone else's
/// title is not recoverable.
#[derive(Debug, Default)]
pub(crate) struct TerminalTitleDriver {
    applied: Option<String>,
}

impl TerminalTitleDriver {
    /// Recompute the title from state and write it if it changed.
    pub(crate) fn refresh(&mut self, state: &AppState) {
        let items = &state.ui.display_settings.terminal_title;
        let desired = (!items.is_empty())
            .then(|| title_text(state, items))
            .flatten();
        if desired == self.applied {
            return;
        }
        let Some(title) = desired else {
            self.clear();
            return;
        };
        match set_terminal_title(&title) {
            Ok(SetTerminalTitleResult::Applied) => self.applied = Some(title),
            // Sanitization left nothing visible — clear rather than write an
            // empty title, so a hostile-only title cannot blank the tab label.
            Ok(SetTerminalTitleResult::NoVisibleContent) => self.clear(),
            Err(err) => tracing::debug!(
                target: "coco_tui::terminal_title",
                error = %err,
                "failed to set terminal title",
            ),
        }
    }

    /// Drop the title coco wrote. No-op when coco never wrote one — the
    /// terminal's own title is not ours to erase.
    pub(crate) fn clear(&mut self) {
        if self.applied.is_none() {
            return;
        }
        if let Err(err) = clear_terminal_title() {
            tracing::debug!(
                target: "coco_tui::terminal_title",
                error = %err,
                "failed to clear terminal title",
            );
        }
        self.applied = None;
    }
}

/// Render the configured items, skipping the ones with nothing to say.
///
/// `None` when every item is unavailable: a title of separators is worse than
/// no title at all.
fn title_text(state: &AppState, items: &[TerminalTitleItem]) -> Option<String> {
    let parts: Vec<String> = items
        .iter()
        .filter_map(|item| title_item_text(state, *item))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn title_item_text(state: &AppState, item: TerminalTitleItem) -> Option<String> {
    match item {
        TerminalTitleItem::AppName => Some(coco_config::constants::PRODUCT_NAME.to_string()),
        TerminalTitleItem::Project => project_name(state),
        TerminalTitleItem::Cwd => state.session.working_dir.clone(),
        TerminalTitleItem::RunState => Some(run_state_text(state)),
        TerminalTitleItem::Model => model_name(state),
        TerminalTitleItem::GitBranch => state.session.git_branch.clone(),
        TerminalTitleItem::ContextUsed => {
            render_context_usage(state).map(|usage| format!("ctx {}%", usage.percent))
        }
    }
}

fn project_name(state: &AppState) -> Option<String> {
    let dir = state.session.working_dir.as_deref()?;
    Some(
        dir.rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .unwrap_or(dir)
            .to_string(),
    )
}

/// Bare model id, not `provider/model`: a tab label has no room for the
/// provider, and the model alone is what distinguishes two coco tabs.
fn model_name(state: &AppState) -> Option<String> {
    let binding_id = state
        .session
        .model_by_role
        .get(&ModelRole::Main)
        .map(|binding| binding.model_id.clone())
        .unwrap_or_else(|| state.session.model.clone());
    (!binding_id.is_empty()).then_some(binding_id)
}

/// The state the title exists to communicate.
///
/// "Waiting" outranks "working" because a queued permission prompt is the one
/// state where the session cannot make progress without the user, and it is the
/// state a backgrounded tab most needs to advertise.
fn run_state_text(state: &AppState) -> String {
    if state.ui.interaction.active_prompt.is_some() {
        return format!(
            "{ATTENTION_MARKER}{}",
            crate::i18n::t!("title.awaiting_input")
        );
    }
    if state.ui.ephemeral.turn_active() || state.session.is_compacting {
        return crate::i18n::t!("title.working").to_string();
    }
    crate::i18n::t!("title.idle").to_string()
}

#[cfg(test)]
#[path = "terminal_title.test.rs"]
mod tests;
