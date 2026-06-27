//! Shared helpers for reminder-coverage tests across all three live
//! layers (CLI / SDK / TUI).
//!
//! The `coco-system-reminder` crate injects per-turn reminders into the
//! engine's message history as `Message::Attachment` entries. Each
//! reminder carries an `AttachmentKind` discriminant matching the TS
//! `Attachment.type` taxonomy.
//!
//! Tests assert reminder presence/content by:
//! 1. Pulling the post-turn message history out of the layer under test
//!    (`QueryResult.final_messages` for CLI / bare-engine paths;
//!    `runtime.history` snapshot for SDK + TUI which use `SessionRuntime`).
//! 2. Filtering for `Message::Attachment` and matching on `kind`.
//! 3. Optionally extracting the wrapped text body for substring checks.
//!
//! Helpers are layer-agnostic — they take a `&[Message]` slice and never
//! reach into harness internals.

#![allow(dead_code)] // not every runner uses every helper

use coco_messages::AttachmentBody;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::UserContent;
use coco_types::AttachmentKind;

/// Iterate every `Message::Attachment` in the slice, returning a pair
/// of `(kind, body_text)`. `body_text` is the concatenated text content
/// of the attachment's API body — empty string for non-text bodies.
pub fn iter_reminders<M: std::borrow::Borrow<Message>>(
    messages: &[M],
) -> Vec<(AttachmentKind, String)> {
    messages
        .iter()
        .filter_map(|m| match m.borrow() {
            Message::Attachment(a) => Some((a.kind, attachment_text(&a.body))),
            _ => None,
        })
        .collect()
}

/// All `AttachmentKind`s present, in turn order. Duplicates preserved.
pub fn injected_reminder_kinds<M: std::borrow::Borrow<Message>>(
    messages: &[M],
) -> Vec<AttachmentKind> {
    iter_reminders(messages)
        .into_iter()
        .map(|(k, _)| k)
        .collect()
}

/// All wrapped text bodies for attachments matching `kind`, in turn
/// order. Empty Vec if none.
pub fn reminder_bodies_for<M: std::borrow::Borrow<Message>>(
    messages: &[M],
    kind: AttachmentKind,
) -> Vec<String> {
    iter_reminders(messages)
        .into_iter()
        .filter(|(k, _)| *k == kind)
        .map(|(_, body)| body)
        .collect()
}

/// `true` iff at least one reminder of `kind` is present.
pub fn has_reminder<M: std::borrow::Borrow<Message>>(messages: &[M], kind: AttachmentKind) -> bool {
    iter_reminders(messages).iter().any(|(k, _)| *k == kind)
}

/// Panic with a helpful diagnostic if `kind` is missing — lists every
/// reminder that was present so the test failure is debuggable.
pub fn assert_reminder_present<M: std::borrow::Borrow<Message>>(
    messages: &[M],
    kind: AttachmentKind,
    ctx: &str,
) {
    if has_reminder(messages, kind) {
        return;
    }
    let kinds = injected_reminder_kinds(messages);
    panic!(
        "[{ctx}] expected reminder `{}` to be injected, but it was not. \
         Reminders observed: {:?}",
        kind.as_str(),
        kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
    );
}

/// Assert at least one reminder of `kind` exists AND its body contains
/// `needle`. Substring match (case-sensitive). Diagnostic shows up to
/// the first 400 chars of the offending body.
pub fn assert_reminder_contains<M: std::borrow::Borrow<Message>>(
    messages: &[M],
    kind: AttachmentKind,
    needle: &str,
    ctx: &str,
) {
    let bodies = reminder_bodies_for(messages, kind);
    if bodies.is_empty() {
        let kinds = injected_reminder_kinds(messages);
        panic!(
            "[{ctx}] expected reminder `{}` containing `{needle}`, but no \
             reminder of that kind was injected. All reminders: {:?}",
            kind.as_str(),
            kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>(),
        );
    }
    if bodies.iter().any(|b| b.contains(needle)) {
        return;
    }
    let first = bodies
        .first()
        .cloned()
        .unwrap_or_default()
        .chars()
        .take(400)
        .collect::<String>();
    panic!(
        "[{ctx}] reminder `{}` was injected but body did not contain `{needle}`. \
         First body (truncated to 400 chars): {first:?}",
        kind.as_str(),
    );
}

/// Concatenate every text fragment from an `AttachmentBody`. Returns
/// the empty string for non-API or non-text bodies.
fn attachment_text(body: &AttachmentBody) -> String {
    match body {
        AttachmentBody::Api(LlmMessage::User { content, .. }) => content
            .iter()
            .filter_map(|c| match c {
                UserContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        AttachmentBody::Api(LlmMessage::Assistant { content, .. }) => content
            .iter()
            .filter_map(|c| match c {
                coco_messages::AssistantContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        // Other body shapes (Silent / Unit / non-text Api variants) carry
        // no API-visible text.
        _ => String::new(),
    }
}

/// Pretty-print the reminder timeline for diagnostic prints (e.g. in
/// failure messages of more involved scenarios). Truncates each body
/// to keep output readable.
pub fn debug_reminder_timeline<M: std::borrow::Borrow<Message>>(messages: &[M]) -> String {
    iter_reminders(messages)
        .iter()
        .enumerate()
        .map(|(i, (kind, body))| {
            let snippet: String = body.chars().take(80).collect();
            format!("  [{i}] {} — {snippet:?}", kind.as_str())
        })
        .collect::<Vec<_>>()
        .join("\n")
}
