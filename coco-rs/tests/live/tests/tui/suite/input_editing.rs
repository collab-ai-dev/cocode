//! Mid-edit keyboard flow: type, backspace, type more, then Enter.
//! Drives the same `KeyEvent → keybinding_bridge → TuiCommand` chain
//! `keyboard_dispatch` covers, but stresses the *editing* commands —
//! `DeleteBackward` (Backspace) and `DeleteWordBackward` (Ctrl+Backspace)
//! — and proves the buffer mutations are visible to a downstream Enter.
//! Verifies:
//!
//! - Each keystroke produces a state change (true return).
//! - The visible input buffer evolves char-by-char as expected.
//! - Ctrl+Backspace nukes a whole word, not just one char.
//! - Enter submits the *edited* text (not the originally-typed text)
//!   and the engine sees the post-edit string.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::KeyCode;
use crossterm::event::KeyModifiers;

use crate::tui::harness::TuiHarness;
use crate::tui::scripted_model::Reply;

pub async fn run() -> Result<()> {
    let mut harness = TuiHarness::builder()
        .with_replies([Reply::text("got it: hello")])
        .build()
        .await?;

    // Step 1: type "hellp" character-by-character.
    for c in ['h', 'e', 'l', 'l', 'p'] {
        let changed = harness
            .press_key(KeyCode::Char(c), KeyModifiers::NONE)
            .await;
        assert!(
            changed,
            "input_editing: typing `{c}` should mark state dirty",
        );
    }
    assert_eq!(
        harness.state.ui.input.text(),
        "hellp",
        "input_editing: buffer after typing `hellp` was {:?}",
        harness.state.ui.input.text(),
    );

    // Step 2: Backspace twice — drop the typo + the final `l`.
    for _ in 0..2 {
        let changed = harness
            .press_key(KeyCode::Backspace, KeyModifiers::NONE)
            .await;
        assert!(changed, "input_editing: Backspace should mark state dirty",);
    }
    assert_eq!(
        harness.state.ui.input.text(),
        "hel",
        "input_editing: buffer after 2× Backspace was {:?}",
        harness.state.ui.input.text(),
    );

    // Step 3: type the rest — `l`, `o`.
    for c in ['l', 'o'] {
        harness
            .press_key(KeyCode::Char(c), KeyModifiers::NONE)
            .await;
    }
    assert_eq!(
        harness.state.ui.input.text(),
        "hello",
        "input_editing: buffer after `lo` append was {:?}",
        harness.state.ui.input.text(),
    );

    // Step 4: prove word-delete works — append a junk word then nuke it.
    harness
        .press_key(KeyCode::Char(' '), KeyModifiers::NONE)
        .await;
    for c in ['x', 'y', 'z'] {
        harness
            .press_key(KeyCode::Char(c), KeyModifiers::NONE)
            .await;
    }
    assert_eq!(
        harness.state.ui.input.text(),
        "hello xyz",
        "input_editing: pre-word-delete buffer was {:?}",
        harness.state.ui.input.text(),
    );
    let changed = harness
        .press_key(KeyCode::Backspace, KeyModifiers::CONTROL)
        .await;
    assert!(
        changed,
        "input_editing: Ctrl+Backspace should mark state dirty",
    );
    assert!(
        // word-delete may keep the trailing space depending on cursor logic;
        // either "hello" or "hello " is acceptable — the load-bearing check
        // is that "xyz" is gone.
        !harness.state.ui.input.text().contains("xyz")
            && harness.state.ui.input.text().starts_with("hello"),
        "input_editing: Ctrl+Backspace did not delete the word `xyz` \
         (buffer={:?})",
        harness.state.ui.input.text(),
    );
    // Trim residual whitespace to make the Enter step deterministic.
    while harness.state.ui.input.text().ends_with(' ') {
        harness
            .press_key(KeyCode::Backspace, KeyModifiers::NONE)
            .await;
    }
    assert_eq!(
        harness.state.ui.input.text(),
        "hello",
        "input_editing: post-cleanup buffer should be `hello`, got {:?}",
        harness.state.ui.input.text(),
    );

    // Step 5: Enter — flushes the edited buffer through SubmitInput.
    let enter_changed = harness.press_key(KeyCode::Enter, KeyModifiers::NONE).await;
    assert!(
        enter_changed,
        "input_editing: Enter should produce a state change"
    );
    assert_eq!(
        harness.state.ui.input.text(),
        "",
        "input_editing: input buffer should be drained after Enter",
    );

    let ok = harness.pump_until_idle(Duration::from_secs(15)).await?;
    assert!(ok, "input_editing: SessionResult flagged is_error");

    // Engine received the *edited* text — `hello`, not `hellp` or `hello xyz`.
    let cells = harness.text_cells_in_order();
    let saw_edited = cells
        .iter()
        .any(|(role, text)| *role == "user" && *text == "hello");
    assert!(
        saw_edited,
        "input_editing: edited user prompt `hello` not in transcript (got: {cells:?})",
    );
    assert!(
        harness.assistant_text_contains("got it"),
        "input_editing: scripted assistant reply not in transcript",
    );
    assert_eq!(
        harness.model.call_count(),
        1,
        "input_editing: expected 1 LLM call for the single submission, got {}",
        harness.model.call_count(),
    );

    harness.shutdown().await;
    Ok(())
}
