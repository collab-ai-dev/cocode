//! Help state presentation.
//!
//! Reads from [`crate::keymap::KEYMAP`] (the structured source of truth)
//! and renders one row per entry, grouped by `KeymapGroup`. For entries
//! bound to a configurable `KeybindingAction`, the displayed combo comes
//! from the user's active resolver (so a rebind in `~/.coco/keybindings.json`
//! shows the rebound key, not the default). For built-in verbs and
//! prompt prefixes the combo from the keymap entry is used as-is.

use ratatui::prelude::Color;

use crate::i18n::t;
use crate::keybinding_bridge::KeybindingContext as TuiContext;
use crate::keymap::GROUP_ORDER;
use crate::keymap::KeymapBinding;
use crate::keymap::KeymapEntry;
use crate::keymap::entries_for_group;
use crate::state::AppState;
use coco_tui_ui::style::UiStyles;

const SHORTCUT_COLUMN_WIDTH: usize = 18;

pub(crate) fn help_content(state: &AppState, styles: UiStyles<'_>) -> (String, String, Color) {
    let mut sections = Vec::new();
    for &group in GROUP_ORDER {
        let mut group_lines = Vec::new();
        for entry in entries_for_group(group) {
            group_lines.push(render_row(state, entry));
        }
        if group_lines.is_empty() {
            continue;
        }
        let title = t!(group.title_key()).to_string();
        sections.push(format!("{title}\n{}", group_lines.join("\n")));
    }
    let body = sections.join("\n\n");
    (t!("help.title").to_string(), body, styles.primary())
}

fn render_row(state: &AppState, entry: &KeymapEntry) -> String {
    let shortcut = match &entry.binding {
        KeymapBinding::Action { action } => state
            .ui
            .kb_handle
            .display_for(action, TuiContext::Chat)
            .unwrap_or_else(|| entry.combo.to_string()),
        // Built-in readline verbs and prompt-prefix markers are
        // hard-coded to the keymap entry's combo display (they're not
        // user-rebindable).
        KeymapBinding::Verb { .. } | KeymapBinding::Marker => entry.combo.to_string(),
    };
    format!("{shortcut:<SHORTCUT_COLUMN_WIDTH$} {}", entry.description())
}

#[cfg(test)]
#[path = "help.test.rs"]
mod tests;
