//! `crossterm::KeyEvent` → [`KeyCombo`] adapter.
//!
//! Feature-gated behind `crossterm`. The TUI enables this; library
//! callers that don't depend on crossterm are unaffected.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use crate::parser::KeyCombo;

/// Build a [`KeyCombo`] from a crossterm key event.
///
/// Returns `None` for events that aren't bindable shortcuts (e.g. raw
/// modifier-only events without a base key).
///
/// Behaviour:
///
/// * Named keys (Enter, Esc, Tab, Backspace, Delete, arrows, PageUp /
///   PageDown, Home, End) map to their canonical lowercase names.
/// * Single character codes lowercase the input.
/// * Function keys map to `f1` … `f12`.
/// * The Shift modifier is preserved on character keys; the caller can
///   tell `A` from `Shift+a` if needed.
pub fn from_crossterm(event: KeyEvent) -> Option<KeyCombo> {
    let key = match event.code {
        KeyCode::Esc => "escape".to_string(),
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => "tab".to_string(),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::Insert => "insert".to_string(),
        KeyCode::F(n) => format!("f{n}"),
        KeyCode::Char(c) => c.to_ascii_lowercase().to_string(),
        KeyCode::Null => return None,
        _ => return None,
    };

    let mods = event.modifiers;
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let shift = mods.contains(KeyModifiers::SHIFT);
    let mut alt = mods.contains(KeyModifiers::ALT);
    // `meta` (alt-equivalent in legacy terminals) is distinct from
    // `super` (cmd/win, only via kitty keyboard protocol). crossterm
    // exposes both, so we keep them separate.
    let mut meta = mods.contains(KeyModifiers::META);
    let super_key = mods.contains(KeyModifiers::SUPER);

    // Terminals (and Ink historically) set the Alt/Meta modifier on Esc
    // keystrokes. Strip both so a plain `escape` binding matches.
    if matches!(event.code, KeyCode::Esc) {
        alt = false;
        meta = false;
    }

    // BackTab is shift+tab from the user's perspective; surface that
    // explicitly so a `shift+tab` binding fires regardless of whether
    // the terminal also set the Shift modifier.
    let shift = matches!(event.code, KeyCode::BackTab) || shift;

    Some(KeyCombo {
        ctrl,
        shift,
        alt,
        meta,
        super_key,
        key,
    })
}

#[cfg(test)]
#[path = "adapter.test.rs"]
mod tests;
