//! Keymap — structured source of truth for input-bar shortcuts, global
//! hotkeys, prompt prefixes, and vim normal-mode keys.
//!
//! Three consumers read from the same [`KEYMAP`] table:
//!
//! 1. **Keyboard help state** (`crate::presentation::help`) — renders the
//!    same data as an in-TUI state.
//! 2. **Built-in subagent retrieval** — [`export_markdown`] and
//!    [`export_json`] dump the table for a future "what keys does coco
//!    support?" tool. JSON resolves descriptions via the active locale so
//!    agents answer in the user's language.
//!
//! The keybinding dispatcher (`crate::keybinding_bridge::map_input_key`)
//! is a separate source of truth — its `match` arms perform the actual
//! routing. Each input-verb arm has a `// keymap_id = "..."` comment
//! pointing at the corresponding [`KeymapEntry`] so cross-references stay
//! traceable. The consistency test in `mod.test.rs` asserts every
//! `KeymapBinding::Verb` resolves to a known [`crate::events::TuiCommand`]
//! verb name, preventing typo drift between data and dispatch.

use coco_keybindings::KeybindingAction;
use serde::Serialize;

use crate::i18n::t;

mod entries;
pub use entries::KEYMAP;

/// Visual grouping for the help renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeymapGroup {
    /// Input bar — cursor movement & history navigation.
    InputCursor,
    /// Input bar — text editing (delete, kill ring, newline insertion).
    InputEdit,
    /// Input bar — submission, cancellation, exit.
    InputSubmit,
    /// Application-wide hotkeys (transcript, palette, fast mode).
    GlobalHotkey,
    /// Prompt and autocomplete prefix tokens (`!`, `@`, `#`, `/`).
    PromptPrefix,
    /// Vim Normal-mode keys (enabled by `/vim`).
    VimNormal,
}

impl KeymapGroup {
    /// i18n key for the user-facing group title.
    pub fn title_key(self) -> &'static str {
        match self {
            Self::InputCursor => "keymap.group.input_cursor",
            Self::InputEdit => "keymap.group.input_edit",
            Self::InputSubmit => "keymap.group.input_submit",
            Self::GlobalHotkey => "keymap.group.global",
            Self::PromptPrefix => "keymap.group.prompt_prefix",
            Self::VimNormal => "keymap.group.vim_normal",
        }
    }
}

/// What a key combo binds to.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeymapBinding {
    /// Input-verb level: a built-in readline-style verb handled by the
    /// TUI dispatcher (`keybinding_bridge::map_input_key`). The `id`
    /// names a `TuiCommand` variant (in `snake_case`) so cross-references
    /// from code comments stay grep-friendly. Not user-rebindable.
    Verb { id: &'static str },
    /// App-level configurable action; resolver-driven. User can rebind
    /// via `~/.coco/keybindings.json`.
    Action {
        #[serde(serialize_with = "serialize_action")]
        action: KeybindingAction,
    },
    /// Non-keystroke marker — e.g. typing `!` as the first character to
    /// enter bash mode, or the vim-mode operators that aren't single
    /// printable keystrokes the bridge would dispatch.
    Marker,
}

fn serialize_action<S: serde::Serializer>(
    action: &KeybindingAction,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&action.as_str())
}

/// One row in the keymap table.
#[derive(Debug, Serialize)]
pub struct KeymapEntry {
    /// Stable identifier in `domain:name` form (e.g. `input:cursor_home`).
    /// Used by JSON export consumers and cross-referenced from the
    /// dispatch code.
    pub id: &'static str,
    /// Primary key combo display (`Ctrl+A`, `Esc Esc`, `dd`).
    pub combo: &'static str,
    /// Alternate combos that map to the same action (`Home` for
    /// `Ctrl+A`). Empty when there are no alternates.
    pub alternates: &'static [&'static str],
    pub group: KeymapGroup,
    pub binding: KeymapBinding,
    /// i18n key resolved via `crate::i18n::t!`.
    pub description_key: &'static str,
}

impl KeymapEntry {
    /// Resolve the description against the active locale.
    pub fn description(&self) -> String {
        t!(self.description_key).to_string()
    }

    /// All combos for this entry (primary + alternates), useful when
    /// rendering "Ctrl+A / Home" together.
    #[cfg(test)]
    pub fn combos(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.combo).chain(self.alternates.iter().copied())
    }
}

/// Entries belonging to a particular group, in declaration order.
pub fn entries_for_group(group: KeymapGroup) -> impl Iterator<Item = &'static KeymapEntry> {
    KEYMAP.iter().filter(move |e| e.group == group)
}

/// All groups in canonical display order. Stable order is part of the
/// public contract — `/help` and the state both rely on it.
pub const GROUP_ORDER: &[KeymapGroup] = &[
    KeymapGroup::InputCursor,
    KeymapGroup::InputEdit,
    KeymapGroup::InputSubmit,
    KeymapGroup::GlobalHotkey,
    KeymapGroup::PromptPrefix,
    KeymapGroup::VimNormal,
];

// ──────────────────── Export — subagent retrieval ────────────────────

/// Serializable view of one entry with localized description baked in.
/// Used by the JSON / markdown export below; subagents and external
/// tools should consume the localized form so they can answer in the
/// user's language.
#[cfg(test)]
#[derive(Debug, Serialize)]
struct ResolvedEntry<'a> {
    id: &'a str,
    combo: &'a str,
    alternates: &'a [&'static str],
    group: KeymapGroup,
    binding: KeymapBinding,
    description: String,
}

#[cfg(test)]
impl<'a> From<&'a KeymapEntry> for ResolvedEntry<'a> {
    fn from(e: &'a KeymapEntry) -> Self {
        Self {
            id: e.id,
            combo: e.combo,
            alternates: e.alternates,
            group: e.group,
            binding: e.binding.clone(),
            description: e.description(),
        }
    }
}

/// Export the keymap as JSON for subagent retrieval. Descriptions are
/// resolved through the active locale; callers that need every locale
/// should iterate by setting `rust_i18n::set_locale` before each call.
///
/// No production consumer yet (the planned `/help`-export wiring never
/// landed); test-gated until one exists so it is not dead code in release
/// builds.
#[cfg(test)]
#[must_use]
pub fn export_json() -> String {
    let resolved: Vec<ResolvedEntry<'_>> = KEYMAP.iter().map(ResolvedEntry::from).collect();
    serde_json::to_string_pretty(&resolved).unwrap_or_else(|_| "[]".to_string())
}

/// Export the keymap as markdown, grouped by [`GROUP_ORDER`], for
/// subagents that prefer pre-rendered output.
///
/// No production consumer yet — see [`export_json`].
#[cfg(test)]
#[must_use]
pub fn export_markdown() -> String {
    let mut out = String::new();
    for &group in GROUP_ORDER {
        let mut group_entries = entries_for_group(group).peekable();
        if group_entries.peek().is_none() {
            continue;
        }
        out.push_str(&format!("**{}**\n\n", t!(group.title_key())));
        for entry in group_entries {
            let mut combo = entry.combo.to_string();
            for alt in entry.alternates {
                combo.push_str(" / ");
                combo.push_str(alt);
            }
            out.push_str(&format!("- `{combo}` — {}\n", entry.description()));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
