//! Pure derivation of `RenderedCell`s from engine `Message`s.
//!
//! Hygiene rule: lives in `coco-tui`, not `coco-messages`. The adapter
//! is one-directional (`Message` → cells) and does not mutate the
//! source message. No theme / viewport / hover state is consulted —
//! that lives in the renderer at draw time.
//!
//! The renderer pipeline consumes `&[RenderedCell]` end-to-end
//! (transcript::render, surface controller/viewport). Engine
//! `MessageHistory` is the only source of truth.
//!
//! See `engine-tui-unified-transcript-plan.md` §2 (Layer Ownership) and
//! `engine-tui-phase3d-renderer-migration-plan.md` §4.

use std::sync::Arc;

use coco_messages::AssistantContent;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::UserContent;
use uuid::Uuid;

use super::cells::CellKind;
use super::cells::RenderedCell;
use super::cells::SystemCellKind;

/// Derive zero or more cells from a single engine `Message`.
///
/// Most variants yield exactly one cell. `Message::Assistant` may
/// yield multiple cells when its content interleaves text / thinking /
/// tool_use blocks. `Message::Tombstone` yields zero (filtered).
pub fn message_to_cells(msg: Arc<Message>) -> Vec<RenderedCell> {
    if matches!(msg.as_ref(), Message::Attachment(_)) && !msg.visibility().ui {
        return Vec::new();
    }

    match &*msg {
        Message::User(user) => {
            let text = extract_user_text(&user.message);
            vec![cell(user.uuid, CellKind::UserText { text }, msg.clone())]
        }
        Message::Assistant(asst) => {
            assistant_cells(asst.uuid, &asst.message, &asst.model, msg.clone())
        }
        Message::System(sm) => {
            let uuid = *sm.uuid();
            vec![cell(
                uuid,
                CellKind::System(SystemCellKind::from(sm)),
                msg.clone(),
            )]
        }
        Message::ToolResult(tr) => {
            // `tool_use_id` is the canonical call_id field on
            // `ToolResultMessage`; surfacing it on the cell lets the
            // projection pair tool-use ↔ tool-result rows by id.
            vec![cell(
                tr.uuid,
                CellKind::ToolResult {
                    call_id: tr.tool_use_id.clone(),
                },
                msg.clone(),
            )]
        }
        Message::Attachment(a) => vec![cell(a.uuid, CellKind::Attachment, msg.clone())],
        Message::Progress(_) => Vec::new(),
        Message::Tombstone(_) => Vec::new(),
    }
}

fn cell(message_uuid: Uuid, kind: CellKind, source: Arc<Message>) -> RenderedCell {
    RenderedCell {
        message_uuid,
        kind,
        source,
    }
}

fn extract_user_text(msg: &LlmMessage) -> String {
    let LlmMessage::User { content, .. } = msg else {
        return String::new();
    };
    let mut buf = String::new();
    for part in content {
        if let UserContent::Text(t) = part {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(&t.text);
        }
    }
    buf
}

fn assistant_cells(
    uuid: Uuid,
    msg: &LlmMessage,
    model: &str,
    source: Arc<Message>,
) -> Vec<RenderedCell> {
    let LlmMessage::Assistant { content, .. } = msg else {
        return Vec::new();
    };
    let mut out: Vec<RenderedCell> = Vec::new();
    let mut reasoning_run: Vec<String> = Vec::new();
    let mut first_visible_reasoning_run = true;
    let flush_reasoning_run =
        |out: &mut Vec<RenderedCell>,
         reasoning_run: &mut Vec<String>,
         first_visible_reasoning_run: &mut bool| {
            if reasoning_run.is_empty() {
                return;
            }
            let visible = reasoning_run
                .iter()
                .filter(|text| !text.is_empty())
                .cloned()
                .collect::<Vec<_>>();
            reasoning_run.clear();
            if visible.is_empty() {
                out.push(cell(
                    uuid,
                    CellKind::AssistantRedactedThinking,
                    source.clone(),
                ));
            } else {
                let metadata_anchor = *first_visible_reasoning_run;
                *first_visible_reasoning_run = false;
                out.push(cell(
                    uuid,
                    CellKind::AssistantThinking {
                        text: visible.join("\n\n"),
                        metadata_anchor,
                    },
                    source.clone(),
                ));
            }
        };
    for part in content {
        if let AssistantContent::Reasoning(r) = part {
            reasoning_run.push(r.text.clone());
            continue;
        }
        let kind = match part {
            AssistantContent::Text(t) if !t.text.is_empty() => {
                flush_reasoning_run(
                    &mut out,
                    &mut reasoning_run,
                    &mut first_visible_reasoning_run,
                );
                CellKind::AssistantText {
                    text: t.text.clone(),
                    model: model.to_string(),
                }
            }
            AssistantContent::ToolCall(tc) => {
                flush_reasoning_run(
                    &mut out,
                    &mut reasoning_run,
                    &mut first_visible_reasoning_run,
                );
                // Overlay-driven tools (plan approval, plan-mode entry, question
                // dialog) render their own surface; suppress the `● ToolName(…)`
                // invocation header so only the result/plan shows. The result
                // cell orphan-renders (see `presentation::transcript` projection),
                // matching TS `userFacingName() == ""`.
                if crate::tool_display::tool_is_overlay_driven(&tc.tool_name) {
                    continue;
                }
                CellKind::ToolUse {
                    call_id: tc.tool_call_id.clone(),
                    tool_name: tc.tool_name.clone(),
                }
            }
            _ => continue,
        };
        out.push(cell(uuid, kind, source.clone()));
    }
    flush_reasoning_run(
        &mut out,
        &mut reasoning_run,
        &mut first_visible_reasoning_run,
    );
    out
}

/// Extract the raw input `Value` of the tool call `call_id` from the
/// assistant message that issued it. Returns the structured value so renderers
/// can pull typed fields — e.g. `old_string`/`new_string` for a diff or
/// `command` for a shell invocation — and [`tool_call_header_preview`] can
/// flatten it to the one-line header. `None` when the message isn't the issuing
/// assistant turn or the call isn't found.
pub(crate) fn extract_tool_call_input(msg: &Message, call_id: &str) -> Option<serde_json::Value> {
    let Message::Assistant(asst) = msg else {
        return None;
    };
    let LlmMessage::Assistant { content, .. } = &asst.message else {
        return None;
    };
    content.iter().find_map(|part| match part {
        AssistantContent::ToolCall(tc) if tc.tool_call_id == call_id => Some(tc.input.clone()),
        _ => None,
    })
}

/// Header-friendly one-line preview of a tool call's input. An object input
/// (the production shape) goes through the per-tool field picker so the header
/// reads `Edit(Cargo.toml)` / `Bash(ls -la)` rather than a raw JSON blob; a bare
/// string is shown unwrapped. Shared by every tool-invocation header (inline
/// chat, expanded cell, transcript reader) so they never diverge.
pub(crate) fn tool_call_header_preview(msg: &Message, call_id: &str, tool_name: &str) -> String {
    tool_call_header_preview_model(msg, call_id, tool_name)
        .plain_text()
        .to_string()
}

pub(crate) fn tool_call_header_preview_model(
    msg: &Message,
    call_id: &str,
    tool_name: &str,
) -> crate::tool_display::ToolInputPreview {
    match extract_tool_call_input(msg, call_id) {
        Some(serde_json::Value::String(s)) => crate::tool_display::ToolInputPreview::Plain(s),
        Some(other) => crate::tool_display::tool_input_semantic_preview(tool_name, &other),
        None => crate::tool_display::ToolInputPreview::Plain(String::new()),
    }
}

/// Project model-facing output and optional typed display data from a
/// `Message::ToolResult`.
/// Pure data accessor — consumed by `render_tool::try_render` to
/// build the result row. Concatenates the `ToolResultOutput` variants
/// to text (JSON parts serialise to their string representation).
pub(crate) fn tool_result_output(msg: &Message) -> Option<ToolResultProjection<'_>> {
    use coco_messages::ToolContent;
    use coco_messages::ToolResultContentPart;
    use coco_messages::ToolResultOutput;

    let Message::ToolResult(tr) = msg else {
        return None;
    };
    let LlmMessage::Tool { content, .. } = &tr.message else {
        return None;
    };
    let part = content.iter().find_map(|p| match p {
        ToolContent::ToolResult(part) => Some(part),
        _ => None,
    })?;
    let tool_name = part.tool_name.clone();
    let output = match &part.output {
        ToolResultOutput::Text { value, .. } => value.clone(),
        ToolResultOutput::Json { value, .. } => value.to_string(),
        ToolResultOutput::Content { value, .. } => value
            .iter()
            .filter_map(|p| match p {
                ToolResultContentPart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        ToolResultOutput::ErrorText { value, .. } => value.clone(),
        ToolResultOutput::ErrorJson { value, .. } => value.to_string(),
        ToolResultOutput::ExecutionDenied { reason, .. } => reason.clone().unwrap_or_default(),
    };
    Some(ToolResultProjection {
        tool_name,
        output,
        display_data: tr.display_data.as_ref(),
    })
}

pub(crate) struct ToolResultProjection<'a> {
    pub(crate) tool_name: String,
    pub(crate) output: String,
    pub(crate) display_data: Option<&'a coco_types::ToolDisplayData>,
}

/// Map a raw id string to a `Uuid`. Returns the parsed UUID when the
/// string is a valid UUID; otherwise derives a deterministic v5 UUID
/// from the bytes so test fixtures that use synthetic ids
/// (`"msg-1"` / `"tool-call-1"`) land on stable cell UUIDs and
/// downstream lookups (rewind picker, transcript anchor) can fall
/// back to the same mapping.
///
/// **Test-only.** Production code paths now key on parsed `Uuid` from
/// the wire (e.g. `Cell.message_uuid`); the
/// fallback v5 derivation exists purely so test fixtures can compare
/// against the same uuids the cell-push helpers produce.
#[cfg(test)]
pub(crate) fn id_to_uuid(id: &str) -> Uuid {
    Uuid::parse_str(id).unwrap_or_else(|_| Uuid::new_v5(&Uuid::NAMESPACE_OID, id.as_bytes()))
}

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Helpers for tests that need to construct `RenderedCell`s without
    //! going through the engine `MessageHistory`.

    use std::sync::Arc;

    use coco_messages::AssistantContent;
    use coco_messages::TextContent;
    use coco_messages::create_assistant_message;
    use coco_messages::create_user_message_with_uuid;
    use coco_types::TokenUsage;
    use uuid::Uuid;

    use super::message_to_cells;
    use crate::transcript::cells::RenderedCell;

    /// One-cell `RenderedCell` for a user text turn keyed by `uuid`.
    pub fn user_text_cell(uuid: Uuid, text: &str) -> RenderedCell {
        let msg = create_user_message_with_uuid(uuid, text);
        message_to_cells(Arc::new(msg))
            .into_iter()
            .next()
            .expect("user message yields a cell")
    }

    /// Single-cell `RenderedCell` for a plain-text assistant turn.
    pub fn assistant_text_cell(text: &str) -> RenderedCell {
        let msg = create_assistant_message(
            vec![AssistantContent::Text(TextContent::new(text))],
            "test-model",
            TokenUsage::default(),
        );
        message_to_cells(Arc::new(msg))
            .into_iter()
            .next()
            .expect("assistant message yields a cell")
    }

    /// Single-cell `RenderedCell` for a `SystemMessage::Informational`
    /// with the meta-preview marker set.
    pub fn info_cell(title: &str, message: &str) -> RenderedCell {
        let msg = coco_messages::create_info_message(title, message);
        message_to_cells(Arc::new(msg))
            .into_iter()
            .next()
            .expect("info message yields a cell")
    }

    /// Single-cell `RenderedCell` for a `/context` usage snapshot
    /// (`SystemMessage::ContextUsage`) — first-class content, not a meta row.
    pub fn context_usage_cell() -> RenderedCell {
        let result = coco_types::ContextUsageResult {
            total_tokens: 1_000,
            max_tokens: 200_000,
            raw_max_tokens: 200_000,
            percentage: 0.5,
            model: "test/model".to_string(),
            categories: Vec::new(),
            is_auto_compact_enabled: false,
            auto_compact_threshold: None,
            message_breakdown: None,
            memory_files: Vec::new(),
            mcp_tools: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            suggestions: Vec::new(),
        };
        let msg = coco_messages::Message::System(coco_messages::SystemMessage::ContextUsage(
            coco_messages::SystemContextUsageMessage {
                uuid: Uuid::new_v4(),
                result,
            },
        ));
        message_to_cells(Arc::new(msg))
            .into_iter()
            .next()
            .expect("context usage message yields a cell")
    }

    /// Synthetic thinking-cell for tests that exercise the assistant
    /// thinking renderer. The owned engine message carries the
    /// reasoning text so renderers can rehydrate metadata via
    /// `cell.source` if needed.
    pub fn assistant_thinking_cell(text: &str) -> RenderedCell {
        use coco_messages::ReasoningContent;
        let msg = create_assistant_message(
            vec![AssistantContent::Reasoning(ReasoningContent::new(text))],
            "test-model",
            TokenUsage::default(),
        );
        // Take the first (and only) cell — thinking content yields a
        // single `AssistantThinking` cell.
        message_to_cells(Arc::new(msg))
            .into_iter()
            .next()
            .expect("thinking message yields a cell")
    }

    /// Synthetic thinking cell paired with its reasoning metadata.
    /// Returns `(cell, ReasoningMetadata)` so tests can stash the
    /// metadata in `SessionState.reasoning_metadata` keyed by the
    /// returned cell's `message_uuid` to exercise the renderer's
    /// "Thinking · <duration> · <tokens>" header path.
    pub fn assistant_thinking_cell_with_metadata(
        text: &str,
        duration_ms: i64,
        reasoning_tokens: i64,
    ) -> (RenderedCell, crate::state::session::ReasoningMetadata) {
        let cell = assistant_thinking_cell(text);
        let meta = crate::state::session::ReasoningMetadata {
            duration_ms: Some(duration_ms),
            reasoning_tokens,
        };
        (cell, meta)
    }

    /// Override the message uuid on a freshly-constructed cell so tests
    /// can correlate stable IDs across asserts. The wrapped `Message`
    /// is left intact — that's the engine-authoritative copy and
    /// renderers read `cell.message_uuid` separately.
    pub fn with_uuid(mut cell: RenderedCell, uuid: Uuid) -> RenderedCell {
        cell.message_uuid = uuid;
        cell
    }

    /// Assistant `ToolUse` cell. `input` is rendered as JSON-encoded
    /// args (matching engine wire shape). Tests typically pair this
    /// with [`tool_result_cell`] using the same `call_id`.
    pub fn tool_use_cell(call_id: &str, tool_name: &str, input: serde_json::Value) -> RenderedCell {
        use coco_messages::ToolCallContent;
        let msg = create_assistant_message(
            vec![AssistantContent::ToolCall(ToolCallContent::new(
                call_id, tool_name, input,
            ))],
            "test-model",
            TokenUsage::default(),
        );
        message_to_cells(Arc::new(msg))
            .into_iter()
            .find(|c| matches!(c.kind, crate::transcript::cells::CellKind::ToolUse { .. }))
            .expect("tool-use yields a cell")
    }

    /// Tool result cell — text output, non-error.
    pub fn tool_result_cell(call_id: &str, tool_name: &str, output: &str) -> RenderedCell {
        use coco_messages::create_tool_result_message;
        use coco_types::ToolId;
        let msg = create_tool_result_message(
            call_id,
            tool_name,
            ToolId::Custom("test".into()),
            output,
            /*is_error*/ false,
        );
        message_to_cells(Arc::new(msg))
            .into_iter()
            .next()
            .expect("tool-result yields a cell")
    }

    // ── Push helpers ─────────────────────────────────────────────────
    //
    // Fixture-friendly wrappers that push a synthesized engine message
    // straight into `SessionState::transcript`. The production write
    // path (`MessageAppended` → `TranscriptView::on_message_appended`)
    // is the only writer — these helpers reuse it so renderer tests
    // see exactly what the live session would.

    use crate::state::session::SessionState;
    use coco_messages::Message;

    #[allow(dead_code)]
    fn push(state: &mut SessionState, msg: Message) {
        state.transcript.on_message_appended(Arc::new(msg));
    }

    /// Push a user text message using a deterministic UUID derived
    /// from `id`. Returns the synthesized cell uuid so callers can
    /// build `TranscriptCellId::message` anchors.
    #[allow(dead_code)]
    pub fn push_user_text(state: &mut SessionState, id: &str, text: &str) -> Uuid {
        let uuid = super::id_to_uuid(id);
        push(
            state,
            coco_messages::create_user_message_with_uuid(uuid, text),
        );
        uuid
    }

    /// Push an assistant text response. The cell uuid is auto-generated
    /// and returned — callers rarely need it but a stable handle is
    /// occasionally useful for anchor lookups.
    #[allow(dead_code)]
    pub fn push_assistant_text(state: &mut SessionState, text: &str) -> Uuid {
        use coco_messages::AssistantContent;
        use coco_messages::TextContent;
        use coco_messages::create_assistant_message;
        let msg = create_assistant_message(
            vec![AssistantContent::Text(TextContent::new(text))],
            "test-model",
            coco_types::TokenUsage::default(),
        );
        let uuid = match &msg {
            Message::Assistant(a) => a.uuid,
            _ => unreachable!("create_assistant_message yields Assistant"),
        };
        push(state, msg);
        uuid
    }

    /// Push an assistant `Thinking` cell with reasoning metadata.
    /// Mirrors the production path: the cell derives from `Message`
    /// (no embedded metadata); duration + reasoning tokens land in
    /// `SessionState.reasoning_metadata` keyed by the cell uuid.
    #[allow(dead_code)]
    pub fn push_assistant_thinking(
        state: &mut SessionState,
        text: &str,
        duration_ms: i64,
        reasoning_tokens: i64,
    ) -> Uuid {
        use coco_messages::AssistantContent;
        use coco_messages::ReasoningContent;
        use coco_messages::create_assistant_message;
        let msg = create_assistant_message(
            vec![AssistantContent::Reasoning(ReasoningContent::new(text))],
            "test-model",
            coco_types::TokenUsage::default(),
        );
        let uuid = match &msg {
            Message::Assistant(a) => a.uuid,
            _ => unreachable!("create_assistant_message yields Assistant"),
        };
        push(state, msg);
        state.insert_reasoning_metadata(
            uuid,
            crate::state::session::ReasoningMetadata {
                duration_ms: Some(duration_ms),
                reasoning_tokens,
            },
        );
        uuid
    }

    /// Push an assistant tool-call invocation. `input_preview` is
    /// encoded as a JSON string so [`tool_call_header_preview`] renders it
    /// unwrapped.
    #[allow(dead_code)]
    pub fn push_tool_use(
        state: &mut SessionState,
        call_id: &str,
        tool_name: &str,
        input_preview: &str,
    ) {
        use coco_messages::AssistantContent;
        use coco_messages::ToolCallContent;
        use coco_messages::create_assistant_message;
        let input = serde_json::Value::String(input_preview.to_string());
        let msg = create_assistant_message(
            vec![AssistantContent::ToolCall(ToolCallContent::new(
                call_id, tool_name, input,
            ))],
            "test-model",
            coco_types::TokenUsage::default(),
        );
        push(state, msg);
    }

    /// Like [`push_tool_use`] but stores a structured object input (the
    /// production shape) instead of a bare JSON string, so renderers that read
    /// typed fields — diffs from `old_string`/`new_string`, the `$ command`
    /// line — can be exercised in snapshot tests.
    #[allow(dead_code)]
    pub fn push_tool_use_input(
        state: &mut SessionState,
        call_id: &str,
        tool_name: &str,
        input: serde_json::Value,
    ) {
        use coco_messages::AssistantContent;
        use coco_messages::ToolCallContent;
        use coco_messages::create_assistant_message;
        let msg = create_assistant_message(
            vec![AssistantContent::ToolCall(ToolCallContent::new(
                call_id, tool_name, input,
            ))],
            "test-model",
            coco_types::TokenUsage::default(),
        );
        push(state, msg);
    }

    /// Push a tool result. `is_error` toggles success vs error path.
    #[allow(dead_code)]
    pub fn push_tool_result(
        state: &mut SessionState,
        call_id: &str,
        tool_name: &str,
        output: &str,
        is_error: bool,
    ) {
        use coco_messages::create_tool_result_message;
        use coco_types::ToolId;
        let msg = create_tool_result_message(
            call_id,
            tool_name,
            ToolId::Custom("test".into()),
            output,
            is_error,
        );
        push(state, msg);
    }

    /// Push a `SystemMessage::Informational` cell with `Info` level.
    /// `title` may be empty.
    #[allow(dead_code)]
    pub fn push_info(state: &mut SessionState, title: &str, message: &str) {
        let msg = coco_messages::create_info_message(title, message);
        push(state, msg);
    }
}

#[cfg(test)]
#[path = "derive.test.rs"]
mod tests;
