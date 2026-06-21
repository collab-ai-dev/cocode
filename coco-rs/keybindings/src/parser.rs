//! Key string parser. Maps strings like `"ctrl+shift+a"`, `"cmd+k"`,
//! `"enter"` into a structured [`KeyCombo`] and chord strings like
//! `"ctrl+x ctrl+k"` into a multi-combo [`KeyChord`].
//!
//! ### Syntax
//!
//! - Single key: `"a"`, `"1"`, `"enter"`, `"f1"`, `"space"`.
//! - Modifiers joined by `+`: `"ctrl+a"`, `"ctrl+shift+p"`, `"cmd+k"`.
//! - Case-insensitive: `"Ctrl+A"` parses the same as `"ctrl+a"`.
//! - Aliases:
//!   - `control` ≡ `ctrl`
//!   - `option` ≡ `opt` ≡ `alt`
//!   - `cmd` ≡ `command` ≡ `super` ≡ `win` (mapped to the `super`
//!     field — distinct from `meta`)
//!   - `meta` is its own modifier (terminal alt-equivalent), kept
//!     distinct from `super` so e.g. macOS `cmd+c` renders as
//!     `cmd+c` not `opt+c`.
//! - Chord (multi-combo): combos separated by **whitespace**:
//!   `"ctrl+x ctrl+k"`. The single literal string `" "` (one space) is
//!   the space-key binding.
//!
//! Returns a typed [`ParseError`] (thiserror enum) so callers can match
//! on the failure mode rather than scrape strings.

use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

/// One key combination (modifiers + base key).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Meta — terminal alt-equivalent. Distinct from [`KeyCombo::super_key`];
    /// the matching layer collapses `alt` and `meta` for chord equality,
    /// but display and storage keep them separate.
    pub meta: bool,
    /// Super — cmd / win, only delivered by terminals using the kitty
    /// keyboard protocol. Distinct from [`KeyCombo::meta`] so e.g. a
    /// macOS `cmd+c` binding renders as `cmd+c` not `opt+c`.
    ///
    /// Named `super_key` (with the `r#`-equivalent rename via serde)
    /// because `super` is a Rust keyword.
    #[serde(rename = "super", default)]
    pub super_key: bool,
    /// The base key: either a single char (normalized lowercase) or a
    /// named key like `"enter"`, `"escape"`, `"f1"`.
    pub key: String,
}

/// A full chord — one or more combos pressed in sequence. Single-key
/// bindings are one-element chords.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyChord(pub Vec<KeyCombo>);

impl KeyChord {
    /// Whether this is a single-combo chord (the common case).
    pub fn is_single(&self) -> bool {
        self.0.len() == 1
    }
}

/// Parse failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("empty chord string")]
    EmptyChord,
    #[error("empty combo in chord")]
    EmptyCombo,
    #[error("empty token in combo `{combo}`")]
    EmptyToken { combo: String },
    #[error("combo `{combo}` has more than one non-modifier key")]
    MultipleBaseKeys { combo: String },
    #[error("combo `{combo}` has no base key")]
    MissingBaseKey { combo: String },
}

/// Parse a chord string like `"ctrl+x ctrl+k"` (whitespace-separated)
/// into one or more combos.
///
/// Special case: a single space character `" "` is the space-key
/// binding, not an empty chord.
pub fn parse_chord(input: &str) -> Result<KeyChord, ParseError> {
    if input == " " {
        return Ok(KeyChord(vec![named_combo("space")]));
    }
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyChord);
    }
    let combos: Vec<KeyCombo> = trimmed
        .split_whitespace()
        .map(parse_combo)
        .collect::<Result<_, _>>()?;
    if combos.is_empty() {
        return Err(ParseError::EmptyChord);
    }
    Ok(KeyChord(combos))
}

/// Parse a single combo (no whitespace; use [`parse_chord`] for chords).
pub fn parse_combo(input: &str) -> Result<KeyCombo, ParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ParseError::EmptyCombo);
    }
    let mut ctrl = false;
    let mut shift = false;
    let mut alt = false;
    let mut meta = false;
    let mut super_key = false;
    let mut key: Option<String> = None;

    for piece in trimmed.split('+') {
        let token = piece.trim().to_ascii_lowercase();
        if token.is_empty() {
            return Err(ParseError::EmptyToken {
                combo: trimmed.to_string(),
            });
        }
        match token.as_str() {
            "ctrl" | "control" => ctrl = true,
            "shift" => shift = true,
            "alt" | "option" | "opt" => alt = true,
            "meta" => meta = true,
            "cmd" | "command" | "super" | "win" => super_key = true,
            _ => {
                if key.is_some() {
                    return Err(ParseError::MultipleBaseKeys {
                        combo: trimmed.to_string(),
                    });
                }
                key = Some(normalize_key(&token));
            }
        }
    }

    let key = key.ok_or(ParseError::MissingBaseKey {
        combo: trimmed.to_string(),
    })?;
    Ok(KeyCombo {
        ctrl,
        shift,
        alt,
        meta,
        super_key,
        key,
    })
}

/// Normalize named keys to a canonical form (e.g. `"return"` →
/// `"enter"`, `"esc"` → `"escape"`).
fn normalize_key(raw: &str) -> String {
    match raw {
        "return" | "enter" => "enter".into(),
        "esc" | "escape" => "escape".into(),
        "del" | "delete" => "delete".into(),
        "bs" | "backspace" => "backspace".into(),
        "tab" => "tab".into(),
        "space" | " " => "space".into(),
        "pgup" => "pageup".into(),
        "pgdn" => "pagedown".into(),
        _ => raw.into(),
    }
}

/// Helper for special cases (the lone-space chord).
fn named_combo(name: &str) -> KeyCombo {
    KeyCombo {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
        super_key: false,
        key: normalize_key(name),
    }
}

#[cfg(test)]
#[path = "parser.test.rs"]
mod tests;
