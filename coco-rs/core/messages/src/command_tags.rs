//! Slash-command transcript messages — echo and result formatting
//! for slash commands persisted in the transcript.
//!
//! A slash command's echo + result render tool-style (`❯ /cmd` + `⎿ output`).
//! Both are carried on `Message::User` envelopes that are
//! `is_visible_in_transcript_only` — they show in the transcript but never
//! reach the model. Slash commands are user↔tool interactions, not
//! conversation, so this is the only mode coco-rs produces here.
//!
//! ## Display modes
//!
//! Three display behaviors are available through distinct, orthogonal mechanisms
//! (a dedicated enum was unnecessary — see commit history). If you ever need
//! one, this is the map:
//!
//! | display mode | meaning                       | mechanism |
//! |--------------|-------------------------------|-----------|
//! | `'skip'`     | no transcript message         | return `CommandResult::Skip` (don't call this builder) |
//! | `'system'`   | user-visible, model-invisible | **this builder** (the default) |
//! | `'user'`     | also enters model context     | feed the model via `CommandResult::Prompt` / `InjectPrompt` (the prompt-expansion path); or, for a literal model-visible echo, call [`slash_user_message`] with `transcript_only = false` (exercised by `command_tags.test.rs`) |
//!
//! The content embeds the same XML tags (`<command-name>`, `<local-command-stdout>`,
//! etc.) so a single tag-aware renderer strips them back to `❯`/`⎿` rows.

use coco_types::messages::LlmMessage;
use coco_types::messages::Message;
use coco_types::messages::MessageOrigin;
use coco_types::messages::SystemContextUsageMessage;
use coco_types::messages::SystemMessage;
use coco_types::messages::UserMessage;
use uuid::Uuid;

/// `<command-name>` — wraps the slash name in the echo.
pub const COMMAND_NAME_TAG: &str = "command-name";
/// `<command-message>` — the bare name, no leading slash.
pub const COMMAND_MESSAGE_TAG: &str = "command-message";
/// `<command-args>` — the argument string.
pub const COMMAND_ARGS_TAG: &str = "command-args";
/// `<local-command-stdout>` — wraps a command's result text.
pub const LOCAL_COMMAND_STDOUT_TAG: &str = "local-command-stdout";
/// `<local-command-stderr>` — wraps a command's error text.
pub const LOCAL_COMMAND_STDERR_TAG: &str = "local-command-stderr";
/// Placeholder when a command produced no output.
pub const NO_CONTENT_MESSAGE: &str = "(no content)";

/// Build the command-input echo body. The trailing indentation in the
/// template keeps persisted transcripts consistent.
pub fn format_command_input(name: &str, args: &str) -> String {
    format!(
        "<{COMMAND_NAME_TAG}>/{name}</{COMMAND_NAME_TAG}>\n            <{COMMAND_MESSAGE_TAG}>{name}</{COMMAND_MESSAGE_TAG}>\n            <{COMMAND_ARGS_TAG}>{args}</{COMMAND_ARGS_TAG}>"
    )
}

/// Wrap result text as a `<local-command-stdout>` body (empty → `(no content)`).
pub fn format_local_command_stdout(value: &str) -> String {
    let body = if value.is_empty() {
        NO_CONTENT_MESSAGE
    } else {
        value
    };
    format!("<{LOCAL_COMMAND_STDOUT_TAG}>{body}</{LOCAL_COMMAND_STDOUT_TAG}>")
}

/// Wrap error text as a `<local-command-stderr>` body (empty → `(no content)`).
/// The renderer paints stderr bodies in the error color so slash-command
/// failures read like a tool result's `⎿ error` row.
pub fn format_local_command_stderr(value: &str) -> String {
    let body = if value.is_empty() {
        NO_CONTENT_MESSAGE
    } else {
        value
    };
    format!("<{LOCAL_COMMAND_STDERR_TAG}>{body}</{LOCAL_COMMAND_STDERR_TAG}>")
}

/// Extract the inner (trimmed) text of the first `<tag>…</tag>` if present.
pub fn extract_tag<'a>(content: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = content.find(&open)? + open.len();
    let end = content[start..].find(&close)? + start;
    Some(content[start..end].trim())
}

/// True if `content` is a slash-command echo body (carries `<command-name>`).
pub fn is_command_input(content: &str) -> bool {
    content.contains(&format!("<{COMMAND_NAME_TAG}>"))
}

/// True if `content` is a local-command result body (`<local-command-stdout>`
/// or `<local-command-stderr>`).
pub fn is_local_command_output(content: &str) -> bool {
    let s = content.trim_start();
    s.starts_with(&format!("<{LOCAL_COMMAND_STDOUT_TAG}>"))
        || s.starts_with(&format!("<{LOCAL_COMMAND_STDERR_TAG}>"))
}

/// Build the `❯ /cmd args` echo + `⎿ output` result for a slash command, as
/// transcript-only `Message::User`s (rendered for the user, never sent to
/// the model). This is coco-rs's only slash-feedback display mode — see the
/// module docs for how to restore the `skip` / `user` modes if ever needed.
pub fn build_slash_command_messages(
    name: &str,
    args: &str,
    output: &str,
    is_sensitive: bool,
) -> Vec<Message> {
    vec![
        slash_user_message(
            &format_command_input(name, &redact_args(args, is_sensitive)),
            /*transcript_only*/ true,
        ),
        slash_user_message(
            &format_local_command_stdout(output),
            /*transcript_only*/ true,
        ),
    ]
}

/// Build the `❯ /cmd args` echo + `⎿ error` result for a slash command that
/// failed (handler `Err`, missing handler, etc.), as transcript-only
/// `Message::User`s. Identical to [`build_slash_command_messages`] except the
/// result body is wrapped as `<local-command-stderr>`, so the renderer paints
/// it in the error color — the slash-command analogue of a tool's `⎿ error`.
pub fn build_slash_command_error_messages(name: &str, args: &str, error: &str) -> Vec<Message> {
    vec![
        slash_user_message(
            &format_command_input(name, args),
            /*transcript_only*/ true,
        ),
        slash_user_message(
            &format_local_command_stderr(error),
            /*transcript_only*/ true,
        ),
    ]
}

/// Build the `❯ /context` echo + the inline context-usage snapshot for the
/// `/context` slash command. The echo is a transcript-only `Message::User`
/// (never reaches the model); the snapshot is a `Message::System` the TUI
/// paints as a colored grid + grouped detail (inline, not a modal). Both
/// messages are model-invisible.
pub fn build_context_usage_messages(
    args: &str,
    result: coco_types::ContextUsageResult,
) -> Vec<Message> {
    vec![
        slash_user_message(
            &format_command_input("context", args),
            /*transcript_only*/ true,
        ),
        Message::System(SystemMessage::ContextUsage(SystemContextUsageMessage {
            uuid: Uuid::new_v4(),
            result,
        })),
    ]
}

/// Redact args for sensitive commands.
fn redact_args(args: &str, is_sensitive: bool) -> String {
    if is_sensitive && !args.trim().is_empty() {
        "***".to_string()
    } else {
        args.to_string()
    }
}

/// One `Message::User` carrying tag content, marked with
/// `MessageOrigin::SlashCommand`. `transcript_only = true` keeps it out of
/// the model's context (the slash-feedback default); pass `false` to make
/// the echo/result model-visible — the `display: 'user'` equivalent (see
/// module docs).
fn slash_user_message(content: &str, transcript_only: bool) -> Message {
    Message::User(UserMessage {
        message: LlmMessage::user_text(content),
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: transcript_only,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: Some(MessageOrigin::SlashCommand),
        parent_tool_use_id: None,
    })
}

#[cfg(test)]
#[path = "command_tags.test.rs"]
mod tests;
