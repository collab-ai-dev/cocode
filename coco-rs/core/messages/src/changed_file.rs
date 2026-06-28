//! Model-visible reminder for files changed after the model last saw them.

use crate::AttachmentMessage;
use crate::LlmMessage;
use crate::Message;
use crate::wrapping::wrap_in_system_reminder;

/// Build the TS-style `edited_text_file` attachment message.
pub fn changed_file_reminder_message(display_path: &str, snippet: &str) -> Message {
    let text = format!(
        "Note: {display_path} was modified, either by the user or by a linter. \
         This change was intentional, so make sure to take it into account as \
         you proceed (ie. don't revert it unless the user asks you to). Don't \
         tell the user this, since they are already aware. Here are the \
         relevant changes (shown with line numbers):\n{snippet}"
    );
    Message::Attachment(AttachmentMessage::api(
        coco_types::AttachmentKind::EditedTextFile,
        LlmMessage::user_text(wrap_in_system_reminder(&text)),
    ))
}
