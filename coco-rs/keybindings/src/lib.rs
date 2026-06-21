//! Keyboard shortcut management.
//!
//! Two complementary representations:
//!
//! * [`KeybindingsConfig`] / [`KeybindingBlock`] — the JSON shape from
//!   `~/.coco/keybindings.json`. Used by the loader and template generator.
//! * [`Keybinding`] (parsed) — the resolver's working unit: a typed
//!   `(KeyChord, Option<KeybindingAction>, KeybindingContext)` triple.
//!
//! Use [`KeybindingsConfig::parse_bindings`] to convert from the wire
//! shape to the resolver's parsed form, surfacing parse errors as a
//! separate channel.
//!
//! Closed enums for [`KeybindingAction`] (~98 variants) and
//! [`KeybindingContext`] (20 variants — 18 user-rebindable, 2 internal).
//! See the per-module docs for details.

pub mod action;
pub mod context;
pub mod defaults;
pub mod display;
pub mod parser;
pub mod reserved;
pub mod resolver;
pub mod template;
pub mod validator;

#[cfg(feature = "crossterm")]
pub mod adapter;

#[cfg(feature = "loader")]
pub mod loader;

pub use action::KeybindingAction;
pub use action::UnknownAction;
pub use action::UnknownActionReason;
pub use context::KeybindingContext;
pub use context::UnknownContext;
pub use display::DisplayPlatform;
pub use display::chord_to_display_string;
pub use display::chord_to_string;
pub use display::keystroke_to_display_string;
pub use display::keystroke_to_string;
pub use parser::KeyChord;
pub use parser::KeyCombo;
pub use parser::ParseError;
pub use parser::parse_chord;
pub use parser::parse_combo;
pub use reserved::ReservedShortcut;
pub use reserved::get_reserved_shortcuts;
pub use reserved::lookup_reserved;
pub use reserved::normalize_key_for_comparison;
pub use resolver::CHORD_TIMEOUT;
pub use resolver::ChordResolver;
pub use resolver::ResolveOutcome;
pub use template::generate_template;
pub use validator::Severity;
pub use validator::ValidationIssue;
pub use validator::ValidationKind;
pub use validator::format_issue;
pub use validator::format_issue_oneline;
pub use validator::validate;

#[cfg(feature = "crossterm")]
pub use adapter::from_crossterm;

#[cfg(feature = "loader")]
pub use loader::KeybindingsLoadResult;
#[cfg(feature = "loader")]
pub use loader::KeybindingsWatcher;
#[cfg(feature = "loader")]
pub use loader::default_keybindings_path;
#[cfg(feature = "loader")]
pub use loader::load_keybindings;

use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

/// A parsed keybinding — what the resolver consumes.
///
/// `action: None` represents a null unbind: when the chord matches,
/// the resolver returns [`ResolveOutcome::Unbound`] so the caller can
/// swallow the keystroke without falling through to lower-priority handlers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keybinding {
    pub chord: KeyChord,
    pub action: Option<KeybindingAction>,
    pub context: KeybindingContext,
}

impl Keybinding {
    /// Convenience constructor — parses the chord string and wraps
    /// the action.
    pub fn new(
        chord: &str,
        action: KeybindingAction,
        context: KeybindingContext,
    ) -> Result<Self, ParseError> {
        Ok(Self {
            chord: parse_chord(chord)?,
            action: Some(action),
            context,
        })
    }

    /// Convenience constructor for null-unbinds (`"action": null` in JSON).
    pub fn unbind(chord: &str, context: KeybindingContext) -> Result<Self, ParseError> {
        Ok(Self {
            chord: parse_chord(chord)?,
            action: None,
            context,
        })
    }
}

/// One block from `keybindings.json`: a context plus a chord-keyed map
/// of actions. `BTreeMap` so serialized output is deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeybindingBlock {
    pub context: KeybindingContext,
    /// Chord string (e.g. `"ctrl+x ctrl+k"`) → action, or `None` to
    /// unbind. Whitespace separates chord steps; comma is a literal key.
    pub bindings: BTreeMap<String, Option<KeybindingAction>>,
}

/// Top-level `keybindings.json` shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct KeybindingsConfig {
    /// `"$schema"` URL for editor validation. Optional.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// `"$docs"` URL for the user-facing docs. Optional.
    #[serde(rename = "$docs", default, skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
    /// Ordered blocks. Later blocks/entries override earlier ones at
    /// resolution time (last-wins).
    #[serde(default)]
    pub bindings: Vec<KeybindingBlock>,
}

impl KeybindingsConfig {
    /// Parse the config from JSON content. Strict — does not merge with
    /// defaults; that's the loader's job.
    pub fn from_json(content: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(content)
    }

    /// Serialize to pretty JSON with the documented two-space indent.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        let mut s = serde_json::to_string_pretty(self)?;
        s.push('\n');
        Ok(s)
    }

    /// Convert each block into parsed [`Keybinding`]s. Chord strings
    /// that fail to parse are skipped here — surface them via
    /// [`validator::validate`] instead.
    pub fn parse_bindings(&self) -> Vec<Keybinding> {
        let mut out = Vec::with_capacity(self.bindings.iter().map(|b| b.bindings.len()).sum());
        for block in &self.bindings {
            for (chord_str, action) in &block.bindings {
                if let Ok(chord) = parse_chord(chord_str) {
                    out.push(Keybinding {
                        chord,
                        action: action.clone(),
                        context: block.context,
                    });
                }
            }
        }
        out
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
