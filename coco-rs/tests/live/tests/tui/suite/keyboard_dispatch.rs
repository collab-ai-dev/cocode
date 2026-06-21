//! End-to-end keystroke routing.
//!
//! Drives the same path `App::handle_event` exercises in production —
//! `crossterm::KeyEvent` → `keybinding_bridge::map_key` →
//! `update::handle_command` → `UserCommand` on the wire. Verifies the
//! TUI actually reaches the engine via real keystrokes (not just by
//! calling `submit()` directly).
//!
//! Exercised here:
//! - Typing "hi" (two `Char` keys) builds the input buffer.
//! - `Enter` flushes the buffer through `TuiCommand::SubmitInput`,
//!   which emits `UserCommand::SubmitInput` to the agent driver;
//!   the engine echoes a `Message::User` back via `MessageAppended`.
//! - `Shift+Tab` cycles `permission_mode` and emits
//!   `UserCommand::SetPermissionMode` (the gateway used by the
//!   permission-cycling overlay flow).

use std::time::Duration;

use anyhow::Result;
use coco_types::PermissionMode;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    // Start in `Default` so Shift+Tab has somewhere to cycle to without
    // the bypass-capability gate kicking in.
    let mut harness = TuiHarness::builder()
        .with_permission_mode(PermissionMode::Default)
        .with_replies([Reply::text("ack")])
        .build()
        .await?;

    // Type "hi" via two character keystrokes — exercises the
    // `keybinding_bridge::map_key` → `TuiCommand::InsertChar` path.
    let h_changed = harness
        .press_key(KeyCode::Char('h'), KeyModifiers::NONE)
        .await;
    let i_changed = harness
        .press_key(KeyCode::Char('i'), KeyModifiers::NONE)
        .await;
    assert!(
        h_changed && i_changed,
        "keyboard_dispatch: char keystrokes should mark state dirty"
    );
    assert_eq!(
        harness.state.ui.input.text(),
        "hi",
        "keyboard_dispatch: typed input not buffered, got {:?}",
        harness.state.ui.input.text()
    );

    // Enter flushes the buffer through SubmitInput.
    let enter_changed = harness.press_key(KeyCode::Enter, KeyModifiers::NONE).await;
    assert!(
        enter_changed,
        "keyboard_dispatch: Enter should produce a state change"
    );
    assert_eq!(
        harness.state.ui.input.text(),
        "",
        "keyboard_dispatch: input buffer should be drained after Enter"
    );

    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(ok, "keyboard_dispatch: SessionResult flagged is_error");

    // The "hi" prompt should have reached the engine and produced an
    // assistant reply. This proves Enter routed end-to-end.
    let saw_user = harness
        .text_cells_in_order()
        .iter()
        .any(|(role, text)| *role == "user" && *text == "hi");
    assert!(saw_user, "keyboard_dispatch: user `hi` not in transcript");
    assert!(
        harness.assistant_text_contains("ack"),
        "keyboard_dispatch: assistant reply not in transcript"
    );

    // Shift+Tab cycles permission mode. The engine driver discards
    // SetPermissionMode (we only handle SubmitInput) but the TUI's
    // local state should still flip — the cycle is computed locally
    // before the command is sent.
    let starting_mode = harness.state.session.permission_mode;
    let _ = harness
        .press_key(KeyCode::BackTab, KeyModifiers::SHIFT)
        .await;
    assert_ne!(
        harness.state.session.permission_mode, starting_mode,
        "keyboard_dispatch: Shift+Tab should advance permission_mode \
         (still {:?})",
        harness.state.session.permission_mode
    );

    harness.shutdown().await;
    Ok(())
}
