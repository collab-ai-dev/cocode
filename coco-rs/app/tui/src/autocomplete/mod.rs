//! Autocomplete systems for input suggestions.
//!
//! Three trigger families:
//! - `/` (slash command) — synchronous, ranked from session command list
//! - `@` (unified mention) — agents (sync) + file paths (async) + MCP
//!   resources (planned). See [`unified`] for the merge order.
//! - `@#` (LSP symbol) — debounced async (100 ms), cancel-on-change.
//!
//! Removed in this refactor: the legacy `@agent-<name>` sub-prefix. Agents
//! fall out of the unified `@` pool.

pub mod agent_search;
pub mod file_search;
pub mod skill_search;
pub mod slash;
pub mod symbol_search;
pub mod trigger;
pub mod unified;

pub use agent_search::AgentInfo;
pub use file_search::FileSearchEvent;
pub use file_search::FileSearchManager;
pub use skill_search::SkillInfo;
pub use skill_search::SkillSearchEvent;
pub use skill_search::SkillSearchManager;
pub use symbol_search::SymbolSearchEvent;
pub use symbol_search::SymbolSearchManager;
pub use trigger::apply_async_result;
pub use trigger::apply_async_result_for_key;
pub use trigger::refresh_suggestions;

use crate::widgets::suggestion_popup::SuggestionItem;

/// Shared suggestion state for any autocomplete type.
#[derive(Debug, Clone)]
pub struct SuggestionState {
    /// The query that produced these suggestions.
    pub query: String,
    /// Start position in input where the trigger was detected.
    pub start_pos: i32,
    /// Matching suggestions.
    pub items: Vec<SuggestionItem>,
    /// Currently selected index.
    pub selected: i32,
}

impl SuggestionState {
    /// Create a new suggestion state.
    pub fn new(query: String, start_pos: i32, items: Vec<SuggestionItem>) -> Self {
        Self {
            query,
            start_pos,
            items,
            selected: 0,
        }
    }

    /// Move selection up.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down.
    pub fn select_next(&mut self) {
        if self.selected < self.items.len() as i32 - 1 {
            self.selected += 1;
        }
    }

    /// Get the currently selected item.
    pub fn selected_item(&self) -> Option<&SuggestionItem> {
        self.items.get(self.selected as usize)
    }

    /// Whether there are any suggestions.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}
