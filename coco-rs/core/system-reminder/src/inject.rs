//! Convert [`SystemReminder`] outputs into coco-rs [`Message`] entries.
//!
//! Two-layer design:
//!
//! 1. [`create_injected_messages`] — pure, coco-types-free. Takes `SystemReminder`s
//!    and produces [`InjectedMessage`]s with XML wrapping already applied.
//!    Easy to unit-test without depending on message history machinery.
//! 2. [`inject_reminders`] — engine-facing. Runs step 1, then maps each
//!    [`InjectedMessage`] to a [`Message`] and appends to `history`.
//!
//! The combined pipeline covers
//! `getAttachmentMessages` → `normalizeAttachmentForAPI` →
//! `wrapMessagesInSystemReminder`.
//!
//! Phase A handles [`ReminderOutput::Text`] — the 95 % case for reminders that
//! produce a single user message. Multi-message reminders
//! ([`ReminderOutput::Messages`] / [`ReminderOutput::ModelAttachment`]) are
//! converted too, but only text blocks (no synthetic `tool_use` / `tool_result`
//! pairs) land in `history` because `AttachmentMessage` is user-only. Phase B
//! will add the assistant-block path when a generator needs it.

use coco_messages::AssistantMessage;
use coco_messages::AttachmentMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_types::SkillDiscoveryPayload;
use coco_types::render_skill_discovery_prompt;
use serde_json::Value;
use tracing::debug;
use uuid::Uuid;

use crate::types::ContentBlock;
use crate::types::MessageRole;
use crate::types::ReminderOutput;
use crate::types::SystemReminder;
use crate::xml::wrap_with_tag;

/// A reminder converted for injection — carries `AttachmentKind` so downstream
/// sinks (engine, UI) can classify and filter uniformly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectedMessage {
    /// Single user-visible text block (already XML-wrapped).
    UserText {
        kind: coco_types::AttachmentKind,
        content: String,
        is_meta: bool,
    },
    /// Multi-block user message (text + `tool_result` blocks).
    UserBlocks {
        kind: coco_types::AttachmentKind,
        blocks: Vec<InjectedBlock>,
        is_meta: bool,
    },
    SkillDiscovery {
        payload: SkillDiscoveryPayload,
        content: String,
        is_meta: bool,
    },
    /// Multi-block assistant message (text + `tool_use` blocks).
    AssistantBlocks {
        kind: coco_types::AttachmentKind,
        blocks: Vec<InjectedBlock>,
        is_meta: bool,
    },
}

/// A single content block within a multi-block [`InjectedMessage`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectedBlock {
    Text(String),
    /// Image block — base64 + media type, routed to `UserContentPart::File`
    /// via `FilePart::image_base64` at injection time so vendor-specific
    /// image encoding stays inside `coco-messages` / `coco-inference`.
    Image {
        media_type: String,
        data_base64: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
}

/// Two-sink normalization result: model-visible messages injected into the
/// API call vs. display-only messages the UI may surface without sending
/// to the model.
///
/// Mirrors cocode-rs `NormalizedMessages` (reference impl). Callers that
/// only need the API-visible subset can use [`create_injected_messages`];
/// callers that also want the display-only stream (TUI transcript, log
/// viewer, telemetry) call [`normalize_injected_messages`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NormalizedMessages {
    /// Messages whose content is sent in the next API request.
    pub model_visible: Vec<InjectedMessage>,
    /// Messages hidden from the API but retained for UI / transcript /
    /// telemetry. Populated by silent reminders.
    pub display_only: Vec<InjectedMessage>,
}

impl NormalizedMessages {
    pub fn is_empty(&self) -> bool {
        self.model_visible.is_empty() && self.display_only.is_empty()
    }

    pub fn total_len(&self) -> usize {
        self.model_visible.len() + self.display_only.len()
    }
}

/// Convert a batch of reminders into `InjectedMessage`s (model-visible only).
///
/// Silent reminders (explicit `is_silent` flag OR silent output variant OR
/// empty content) are dropped. Use [`normalize_injected_messages`] if you
/// need silent messages routed to a separate display sink instead of
/// discarded.
///
/// - `Text` → single `UserText` with the output wrapped by the reminder's
///   [`crate::AttachmentType::xml_tag`].
/// - `Messages(vec)` → one `UserBlocks` / `AssistantBlocks` per entry; each
///   text block is wrapped per the parent reminder's tag (each text block
///   is individually wrapped).
/// - `ModelAttachment { payload }` → single `UserText` whose content is
///   the pretty-printed JSON, wrapped in the reminder's tag.
pub fn create_injected_messages(reminders: Vec<SystemReminder>) -> Vec<InjectedMessage> {
    normalize_injected_messages(reminders).model_visible
}

/// Convert a batch of reminders into normalized model-visible + display-only
/// sinks.
///
/// Silent variants (`Silent`, `SilentText`, `SilentMessages`,
/// `SilentAttachment`) + any reminder with `is_silent = true` are routed to
/// `display_only`; everything else is routed to `model_visible`. The same
/// XML wrapping applies in both cases so UI consumers see the exact tagging
/// the model would have seen if the reminder were visible.
pub fn normalize_injected_messages(reminders: Vec<SystemReminder>) -> NormalizedMessages {
    let mut out = NormalizedMessages::default();
    for r in reminders {
        let is_silent = r.is_effectively_silent();
        if is_silent {
            debug!(kind = %r.attachment_type, "reminder routed to display_only");
        }
        let tag = r.xml_tag();
        let is_meta = r.is_meta;
        let kind: coco_types::AttachmentKind = r.attachment_type.into();
        let sink = if is_silent {
            &mut out.display_only
        } else {
            &mut out.model_visible
        };

        match r.output {
            ReminderOutput::Text(content) => {
                if content.is_empty() {
                    continue;
                }
                sink.push(InjectedMessage::UserText {
                    kind,
                    content: wrap_with_tag(&content, tag),
                    is_meta,
                });
            }
            ReminderOutput::Messages(msgs) => {
                for msg in msgs {
                    let blocks = msg
                        .blocks
                        .into_iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => {
                                // Wrap each text block per the parent reminder's tag.
                                InjectedBlock::Text(wrap_with_tag(&text, tag))
                            }
                            ContentBlock::Image {
                                media_type,
                                data_base64,
                            } => InjectedBlock::Image {
                                media_type,
                                data_base64,
                            },
                            ContentBlock::ToolUse { id, name, input } => {
                                InjectedBlock::ToolUse { id, name, input }
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                            } => InjectedBlock::ToolResult {
                                tool_use_id,
                                content,
                            },
                        })
                        .collect();
                    match msg.role {
                        MessageRole::User => sink.push(InjectedMessage::UserBlocks {
                            kind,
                            blocks,
                            is_meta: msg.is_meta,
                        }),
                        MessageRole::Assistant => sink.push(InjectedMessage::AssistantBlocks {
                            kind,
                            blocks,
                            is_meta: msg.is_meta,
                        }),
                    }
                }
            }
            ReminderOutput::ModelAttachment { payload }
            | ReminderOutput::SilentAttachment { payload } => {
                if payload.is_null() {
                    continue;
                }
                let text =
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
                sink.push(InjectedMessage::UserText {
                    kind,
                    content: wrap_with_tag(&text, tag),
                    is_meta,
                });
            }
            ReminderOutput::SkillDiscovery(payload) => {
                if payload.skills.is_empty() {
                    continue;
                }
                sink.push(InjectedMessage::SkillDiscovery {
                    content: wrap_with_tag(&render_skill_discovery_prompt(&payload), tag),
                    payload,
                    is_meta,
                });
            }
        }
    }
    out
}

/// Materialized reminder messages produced by [`inject_reminders`].
///
/// Holds the model-visible `Message`s that the caller should push
/// into `MessageHistory` (typically via
/// `coco_query::history_sync::history_push_and_emit` so each entry
/// emits a `MessageAppended` event and the TUI / SDK observers stay
/// coherent — I-1 authority), plus the silent / display-only
/// reminders that never reach the model but may surface to UI or
/// telemetry.
#[derive(Debug, Clone, Default)]
pub struct InjectedReminderBatch {
    /// Reminders that must land in `MessageHistory`. Order preserved.
    /// Caller is responsible for pushing each one through an
    /// event-emitting helper so observers track the new entries.
    pub model_visible: Vec<Message>,
    /// Reminders that **must not** reach the model. Returned for UI /
    /// telemetry sinks; never written to history.
    pub display_only: Vec<InjectedMessage>,
}
impl InjectedReminderBatch {
    pub fn is_empty(&self) -> bool {
        self.model_visible.is_empty() && self.display_only.is_empty()
    }
}

/// Build model-visible reminder messages and split out the silent
/// subset. Returns owned `Message`s instead of mutating a history
/// vector so callers go through the event-emitting helper
/// (`coco_query::history_sync::history_push_and_emit`) and every
/// reminder appended to `MessageHistory` is observable on the wire.
///
/// - Simple text reminders → [`Message::Attachment`] (matches the existing
///   `PlanModeReminder::reminder_message` shape in `app/query`).
/// - Multi-block user reminders → [`Message::Attachment`] with API body.
/// - Multi-block assistant reminders → [`Message::Assistant`] carrying the
///   raw blocks. No model/usage metadata is attached — these are synthetic.
/// - Silent reminders → returned on `display_only`. **Never**
///   suggested for history push (they must not reach the model).
pub fn inject_reminders(reminders: Vec<SystemReminder>) -> InjectedReminderBatch {
    let normalized = normalize_injected_messages(reminders);
    let mut model_visible: Vec<Message> = Vec::with_capacity(normalized.model_visible.len());
    for msg in normalized.model_visible {
        match msg {
            InjectedMessage::UserText {
                kind,
                content,
                is_meta: _,
            } => {
                model_visible.push(Message::Attachment(AttachmentMessage::api(
                    kind,
                    LlmMessage::user_text(content),
                )));
            }
            InjectedMessage::UserBlocks {
                kind,
                blocks,
                is_meta: _,
            } => {
                // Multi-block reminder content lives in Message::Attachment
                // with Api body. The kind governs API + UI filtering; no
                // separate is_meta flag needed.
                let llm = user_llm_from_blocks(blocks);
                model_visible.push(Message::Attachment(coco_messages::AttachmentMessage::api(
                    kind, llm,
                )));
            }
            InjectedMessage::SkillDiscovery {
                payload,
                content,
                is_meta: _,
            } => {
                model_visible.push(Message::Attachment(
                    AttachmentMessage::skill_discovery_with_content(payload, content),
                ));
            }
            InjectedMessage::AssistantBlocks {
                kind: _,
                blocks,
                is_meta: _,
            } => {
                let llm = assistant_llm_from_blocks(blocks);
                model_visible.push(Message::Assistant(AssistantMessage {
                    message: llm,
                    uuid: Uuid::new_v4(),
                    model: String::new(),
                    stop_reason: None,
                    usage: None,
                    cost_usd: None,
                    request_id: None,
                    api_error: None,
                }));
            }
        }
    }
    InjectedReminderBatch {
        model_visible,
        display_only: normalized.display_only,
    }
}

fn user_llm_from_blocks(blocks: Vec<InjectedBlock>) -> LlmMessage {
    use coco_messages::FileContent;
    use coco_messages::TextContent;
    use coco_messages::UserContent;
    let mut content: Vec<UserContent> = Vec::with_capacity(blocks.len());
    for b in blocks {
        match b {
            InjectedBlock::Text(text) => {
                content.push(UserContent::Text(TextContent {
                    text,
                    provider_metadata: None,
                }));
            }
            InjectedBlock::Image {
                media_type,
                data_base64,
            } => {
                // Queued-command image pastes alongside the wrapped text —
                // vercel-ai-v4 carries them through `UserContentPart::File`
                // with a base64 payload.
                content.push(UserContent::File(FileContent::from_base64(
                    data_base64,
                    media_type,
                )));
            }
            // vercel-ai-v4 routes tool_result / tool_use through the `Tool` /
            // `Assistant` message variants, not `User`. Drop these defensively
            // — Phase B generators that need them will produce `Messages`
            // with `MessageRole::Assistant` blocks, or the inject layer will
            // grow a dedicated tool-result path.
            InjectedBlock::ToolResult { .. } | InjectedBlock::ToolUse { .. } => {
                tracing::warn!(
                    "dropping non-text block in UserBlocks reminder (coco-rs routes tool blocks through assistant/tool messages)"
                );
            }
        }
    }
    LlmMessage::User {
        content,
        provider_options: None,
    }
}

fn assistant_llm_from_blocks(blocks: Vec<InjectedBlock>) -> LlmMessage {
    use coco_messages::AssistantContent;
    use coco_messages::TextContent;
    use coco_messages::ToolCallContent;
    let mut content: Vec<AssistantContent> = Vec::with_capacity(blocks.len());
    for b in blocks {
        match b {
            InjectedBlock::Text(text) => {
                content.push(AssistantContent::Text(TextContent {
                    text,
                    provider_metadata: None,
                }));
            }
            InjectedBlock::ToolUse { id, name, input } => {
                content.push(AssistantContent::ToolCall(ToolCallContent {
                    tool_call_id: id,
                    tool_name: name,
                    input,
                    provider_executed: None,
                    invalid: false,
                    invalid_reason: None,
                    provider_metadata: None,
                }));
            }
            InjectedBlock::ToolResult { .. } | InjectedBlock::Image { .. } => {
                // Assistant content has no Image variant, and tool_result
                // belongs on a Tool message — drop with a warning if a
                // generator ever emits one of these in an assistant block.
                tracing::warn!(
                    "dropping unsupported block in AssistantBlocks reminder (image / tool_result not assistant-routable)"
                );
            }
        }
    }
    LlmMessage::Assistant {
        content,
        provider_options: None,
    }
}

#[cfg(test)]
#[path = "inject.test.rs"]
mod tests;
