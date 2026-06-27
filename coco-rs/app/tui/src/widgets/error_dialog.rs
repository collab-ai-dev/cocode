//! Error dialog — renders the body text that `surface_content` wraps in
//! a centered error modal.
//!
//! Unlike a toast, an error state blocks input until dismissed — used for
//! `TurnEnded(Failed)` and non-retryable `Error` events so the user must
//! acknowledge them.
//!
//! Exposed as a small library (not a `ratatui::Widget`) because the
//! state framework already owns the block + border + centering layout;
//! we only need to format the body string.

use coco_types::ErrorParams;

use crate::i18n::t;

/// Format a rich error body for the error modal. Includes category,
/// retryability hint, and a footer telling the user how to dismiss.
pub fn format_error_body(message: &str, category: Option<&str>, retryable: bool) -> String {
    let mut body = String::new();
    body.push_str(message.trim());
    body.push_str("\n\n");
    if let Some(cat) = category
        && !cat.is_empty()
    {
        body.push_str(&t!("error_dialog.category", cat = cat));
    }
    body.push_str(&if retryable {
        t!("error_dialog.retryable").to_string()
    } else {
        t!("error_dialog.non_retryable").to_string()
    });
    body.push_str(&t!("error_dialog.dismiss"));
    body
}

/// Body for an `Error` notification. Uses the event's `category` and
/// `retryable` fields.
pub fn error_body(params: &ErrorParams) -> String {
    format_error_body(
        &params.message,
        params.category.as_deref(),
        params.retryable,
    )
}

#[cfg(test)]
#[path = "error_dialog.test.rs"]
mod tests;
