//! Assistant-side cell renderers — text (markdown), thinking
//! (collapsible), redacted thinking, tool-use invocation.
//!
//! Dispatches directly on `cell.kind` / `cell.source: Arc<Message>`.
//! All emitted lines are `Line<'static>` (owned spans).

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::hash::Hasher;

use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use super::CellsRenderer;
use crate::i18n::t;
use crate::presentation::thinking::ThinkingDisplay;
use crate::presentation::thinking::ThinkingRenderInput;
use crate::presentation::thinking::format_duration_seconds;
use crate::presentation::thinking::render_thinking_block;
use crate::tool_display::ToolNameTone;
use crate::tool_display::tool_name_tone;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use coco_tui_ui::constants;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;

/// Turn-boundary glyph at the start of each assistant text response.
/// Standardised on `⏺` which renders cleanly in modern Linux/macOS/Windows
/// Terminal fonts and keeps a consistent visual across platforms.
pub(crate) const ASSISTANT_DOT: &str = "⏺";

/// The shared turn-boundary marker for assistant text (finalized + streaming),
/// so both paths land the dot identically and the row cannot jump on finish.
pub(crate) fn assistant_lead_marker(color: ratatui::style::Color) -> coco_tui_markdown::LeadMarker {
    coco_tui_markdown::LeadMarker::new(ASSISTANT_DOT, color)
}

thread_local! {
    /// Content-addressed markdown memo for committed assistant text cells, keyed
    /// by a hash of every line-affecting render input (text + width + syntax +
    /// theme + body_indent + streaming). Keying on content rather than
    /// `message_uuid` is required for correctness: one assistant message derives
    /// into MULTIPLE `AssistantText` cells when its content interleaves blocks
    /// (Text / ToolCall / Text), and all of them share one `uuid` — a uuid-keyed
    /// map would make those sibling cells evict each other every frame, turning
    /// every render into a guaranteed miss.
    ///
    /// Reached by [`render_committed_assistant_markdown`], the committed
    /// assistant-text renderer shared by native finalized append and replay.
    /// It absorbs the repeated full-history suffix renders the replay binary
    /// search performs. Bounded by entry count AND estimated bytes with LRU
    /// eviction (width+theme live in the key, so resize drags / theme cycling
    /// multiply entries — a count-only cap let that pin tens of MB and its
    /// wholesale clear evicted every legitimate entry at once). Because
    /// entries are content-keyed a stale one can never be served — at worst a
    /// removed message's entry is dead weight until evicted. It deliberately
    /// does NOT mirror the `reasoning_metadata` prune lifecycle (that exists
    /// for correctness, not memory). Accepted residual risk: hits are served
    /// on the truncated 64-bit key without storing the full inputs, so a hash
    /// collision would serve wrong lines (~cap²/2⁶⁵ — negligible).
    static COMMITTED_MD_MEMO: RefCell<MdMemo> = RefCell::new(MdMemo::default());

    /// Single-slot memo for the in-flight (streaming) render. Every delta
    /// changes the content hash, so the shared map would gain one dead
    /// snapshot per delta and evict legitimately cached committed cells. The
    /// live tail is one monotonically growing document — remembering the last
    /// render is exactly enough to dedupe the measure-then-paint double call
    /// within a frame.
    static IN_FLIGHT_MD_MEMO: RefCell<Option<(u64, Vec<Line<'static>>)>> =
        const { RefCell::new(None) };
}

/// Memo caps: entry count and estimated payload bytes (mirrors
/// `HistoryReplayCache`'s byte-capped policy at a smaller scale).
const COMMITTED_MD_MEMO_CAP: usize = 4096;
const COMMITTED_MD_MEMO_MAX_BYTES: usize = 8 * 1024 * 1024;

/// LRU + byte-capped markdown memo (same shape as `HistoryReplayCache`,
/// without the admission policy — committed-cell renders are always worth
/// keeping).
#[derive(Default)]
struct MdMemo {
    map: HashMap<u64, Vec<Line<'static>>>,
    lru: std::collections::VecDeque<u64>,
    bytes: usize,
}

impl MdMemo {
    fn touch(&mut self, key: u64) {
        if let Some(pos) = self.lru.iter().position(|&k| k == key) {
            self.lru.remove(pos);
        }
        self.lru.push_back(key);
    }

    fn get(&mut self, key: u64) -> Option<Vec<Line<'static>>> {
        let hit = self.map.get(&key).cloned()?;
        self.touch(key);
        Some(hit)
    }

    fn put(&mut self, key: u64, value: Vec<Line<'static>>) {
        let value_bytes = super::estimate_lines_bytes(&value);
        if let Some(previous) = self.map.insert(key, value) {
            self.bytes = self
                .bytes
                .saturating_sub(super::estimate_lines_bytes(&previous));
        }
        self.bytes = self.bytes.saturating_add(value_bytes);
        self.touch(key);
        while self.lru.len() > COMMITTED_MD_MEMO_CAP || self.bytes > COMMITTED_MD_MEMO_MAX_BYTES {
            let Some(evicted) = self.lru.pop_front() else {
                break;
            };
            if let Some(entry) = self.map.remove(&evicted) {
                self.bytes = self
                    .bytes
                    .saturating_sub(super::estimate_lines_bytes(&entry));
            }
        }
    }

    /// Only the test/bench memo reset uses this; gate it with its caller.
    #[cfg(any(test, feature = "testing"))]
    fn clear(&mut self) {
        self.map.clear();
        self.lru.clear();
        self.bytes = 0;
    }
}

#[cfg(any(test, feature = "testing"))]
pub(crate) fn clear_committed_markdown_memo_for_tests() {
    COMMITTED_MD_MEMO.with(|m| m.borrow_mut().clear());
    IN_FLIGHT_MD_MEMO.with(|m| *m.borrow_mut() = None);
}

/// Drop the in-flight single-slot memo when a stream ends or the surface
/// resets, so the last response's `Vec<Line>` is not retained until the next
/// stream overwrites the slot. Content-keying already makes a stale entry
/// impossible to serve; this is memory hygiene, not correctness.
pub(crate) fn clear_in_flight_markdown_memo() {
    IN_FLIGHT_MD_MEMO.with(|m| *m.borrow_mut() = None);
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CommittedAssistantMarkdownOptions<'a> {
    pub(crate) styles: UiStyles<'a>,
    pub(crate) width: u16,
    pub(crate) syntax_highlighting: SyntaxHighlighting,
}

/// Which memo (if any) backs an assistant-markdown render, and whether mermaid
/// diagrams are laid out. The three modes produce the same rows for the same
/// source EXCEPT for the streaming mermaid-suppression — they differ only in
/// caching, which is why `Committed` and `StreamStable` are row-identical (the
/// soundness anchor for the mid-stream→finalize handoff).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    /// Finalized cells / replay — mermaid laid out, shared content memo (absorbs
    /// repeated replay/finalize renders of the same text).
    Committed,
    /// In-flight live tail — mermaid suppressed (re-laying per delta is the cost
    /// the streaming flag avoids), single-slot memo.
    InFlight,
    /// Mid-stream STABLE region — mermaid laid out (these rows enter native
    /// scrollback and must match the `Committed` finalize render), but
    /// memo-bypassed: the `StreamRenderController` already caches these lines,
    /// so routing them through the shared committed map would flood it with
    /// dead per-advance prefixes and force premature cap clears that evict
    /// legitimate committed-cell entries.
    StreamStable,
}

pub(crate) fn render_committed_assistant_markdown(
    source: &str,
    options: CommittedAssistantMarkdownOptions<'_>,
) -> Vec<Line<'static>> {
    render_assistant_markdown(source, options, RenderMode::Committed)
}

/// Same renderer for IN-FLIGHT assistant text (the live tail), backed by the
/// single-slot [`IN_FLIGHT_MD_MEMO`]. The markdown pass is marked streaming,
/// whose sole effect is suppressing mermaid diagram layout — re-laying a
/// diagram on every delta is exactly the cost that flag exists to avoid; the
/// diagram renders once when the text finalizes into a committed cell.
pub(crate) fn render_in_flight_assistant_markdown(
    source: &str,
    options: CommittedAssistantMarkdownOptions<'_>,
) -> Vec<Line<'static>> {
    render_assistant_markdown(source, options, RenderMode::InFlight)
}

/// Mid-stream STABLE region render: row-identical to the committed render (so the
/// scrollback rows match the eventual finalize) but bypassing the shared memo —
/// the `StreamRenderController` is the cache on this path.
pub(crate) fn render_stream_stable_assistant_markdown(
    source: &str,
    options: CommittedAssistantMarkdownOptions<'_>,
) -> Vec<Line<'static>> {
    render_assistant_markdown(source, options, RenderMode::StreamStable)
}

fn render_assistant_markdown(
    source: &str,
    options: CommittedAssistantMarkdownOptions<'_>,
    mode: RenderMode,
) -> Vec<Line<'static>> {
    let mut opts = coco_tui_markdown::MarkdownOptions::new(
        options.styles,
        options.width,
        options.syntax_highlighting,
    );
    if mode == RenderMode::InFlight {
        opts = opts.streaming();
    }
    let marker = assistant_lead_marker(options.styles.assistant_message());

    // Memo-bypass path: the stream controller owns this cache.
    if mode == RenderMode::StreamStable {
        return coco_tui_markdown::render_markdown(source, opts, Some(&marker));
    }

    let key = {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        source.hash(&mut h);
        opts.width.hash(&mut h);
        opts.syntax.is_enabled().hash(&mut h);
        opts.body_indent.hash(&mut h);
        opts.streaming.hash(&mut h);
        options.styles.theme_hash().hash(&mut h);
        h.finish()
    };
    let hit = if mode == RenderMode::InFlight {
        IN_FLIGHT_MD_MEMO.with(|m| {
            m.borrow()
                .as_ref()
                .and_then(|(cached_key, lines)| (*cached_key == key).then(|| lines.clone()))
        })
    } else {
        COMMITTED_MD_MEMO.with(|m| m.borrow_mut().get(key))
    };
    if let Some(hit) = hit {
        return hit;
    }
    let rendered = coco_tui_markdown::render_markdown(source, opts, Some(&marker));
    if mode == RenderMode::InFlight {
        IN_FLIGHT_MD_MEMO.with(|m| *m.borrow_mut() = Some((key, rendered.clone())));
    } else {
        COMMITTED_MD_MEMO.with(|m| m.borrow_mut().put(key, rendered.clone()));
    }
    rendered
}

pub(super) fn try_render(
    w: &CellsRenderer<'_>,
    cell: &RenderedCell,
    lines: &mut Vec<Line<'static>>,
) -> Option<()> {
    match &cell.kind {
        CellKind::AssistantText { text, .. } => {
            // Renders the body with a leading `BLACK_CIRCLE` turn marker on
            // the first line. The marker is a first-class renderer input
            // (`LeadMarker`); the renderer lands it at column 0 and keeps
            // wrapped prose at the body indent — no fragile first-span
            // string-matching here. Empty responses still get a marker-only
            // line. Memoized by content (see COMMITTED_MD_MEMO) so repeated
            // history replays / fallback rebuilds don't re-run pulldown + syntect.
            lines.extend(render_committed_assistant_markdown(
                text,
                CommittedAssistantMarkdownOptions {
                    styles: w.styles,
                    width: w.width,
                    syntax_highlighting: w.syntax_highlighting,
                },
            ));
            Some(())
        }
        CellKind::AssistantThinking {
            text,
            metadata_anchor,
        } => {
            let side_meta = metadata_anchor
                .then(|| {
                    w.reasoning_metadata
                        .and_then(|cache| cache.get(&cell.message_uuid))
                })
                .flatten();
            let source_reasoning_tokens = metadata_anchor
                .then(|| assistant_source_reasoning_tokens(cell))
                .flatten();
            lines.extend(render_thinking_block(
                ThinkingRenderInput {
                    content: text,
                    duration_ms: side_meta.and_then(|m| m.duration_ms),
                    reasoning_tokens: side_meta
                        .map(|m| m.reasoning_tokens)
                        .or(source_reasoning_tokens),
                    toggle_hint: Some(&w.thinking_toggle_hint()),
                    display: if w.show_thinking {
                        ThinkingDisplay::ExpandedFull
                    } else {
                        ThinkingDisplay::Collapsed
                    },
                },
                w.styles,
            ));
            Some(())
        }
        CellKind::AssistantRedactedThinking => {
            // ✻ (teardrop asterisk) signals "still thinking" — used for
            // the redacted/in-flight variant so users can tell at a glance
            // the block isn't finalized.
            lines.push(Line::from(
                Span::raw(t!("chat.redacted_thinking").to_string())
                    .fg(w.styles.thinking())
                    .dim()
                    .italic(),
            ));
            Some(())
        }
        CellKind::ToolUse { call_id, tool_name } => {
            let input_preview = crate::transcript::derive::tool_call_header_preview_model(
                &cell.source,
                call_id,
                tool_name,
            );
            let preview_spans = crate::tool_display::render_tool_input_preview_spans(
                &input_preview,
                w.styles,
                w.syntax_highlighting,
                constants::TOOL_DESCRIPTION_MAX_CHARS as usize,
            );
            // Elapsed time badge: `(250ms)` / `(1.2s)` / `(3m 4s)`
            // tail-aligned after the preview. Sourced from the
            // matching ToolExecution by call_id so running tools tick
            // forward via SpinnerTick redraws and completed tools
            // freeze at their final duration.
            let elapsed_badge = w
                .tool_executions
                .iter()
                .find(|t| t.call_id == *call_id)
                .map(|t| format!(" ({})", format_duration_seconds(t.elapsed())))
                .unwrap_or_default();
            // Width-1 `●` in the tool-type color so the marker aligns with the
            // assistant `⏺` and the result `└` gutter at column 2 — an emoji
            // (`🔧`) is width-2 and its cell width is font-dependent, so it
            // drifts the whole row out of the gutter.
            let tone = tool_tone_color(tool_name_tone(tool_name), w.styles);
            // Agent (subagent) headers lead with the subagent TYPE
            // (Explore / Plan / custom) rather than the generic "Agent" — the
            // type is the meaningful operation, matching how other tool
            // headers name what they do. Pulled from the call's
            // `subagent_type` input; falls back to the tool name when absent
            // (older transcripts, or any non-Agent tool).
            let header_name = if tool_name.as_str() == coco_types::ToolName::Agent.as_str() {
                crate::transcript::derive::extract_tool_call_input(&cell.source, call_id)
                    .as_ref()
                    .and_then(|input| input.get("subagent_type"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|ty| !ty.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| tool_name.clone())
            } else {
                tool_name.clone()
            };
            let mut spans = vec![
                Span::raw("● ").fg(tone),
                Span::raw(header_name).fg(tone).bold(),
            ];
            if !preview_spans.is_empty() {
                spans.push(Span::raw("(").fg(w.styles.text()));
                spans.extend(preview_spans);
                spans.push(Span::raw(")").fg(w.styles.text()));
            }
            spans.push(Span::raw(elapsed_badge).fg(w.styles.dim()).dim());
            lines.push(Line::from(spans));
            Some(())
        }
        _ => None,
    }
}

fn assistant_source_reasoning_tokens(cell: &RenderedCell) -> Option<i64> {
    let coco_messages::Message::Assistant(assistant) = cell.source.as_ref() else {
        return None;
    };
    let tokens = assistant.usage.as_ref()?.output_tokens.reasoning;
    (tokens > 0).then_some(tokens)
}

fn tool_tone_color(
    tone: ToolNameTone,
    styles: coco_tui_ui::style::UiStyles<'_>,
) -> ratatui::style::Color {
    match tone {
        ToolNameTone::ReadOnly => styles.success(),
        ToolNameTone::Shell => styles.primary(),
        ToolNameTone::Write => styles.warning(),
        ToolNameTone::Agent => styles.accent(),
        ToolNameTone::Plan => styles.plan(),
        ToolNameTone::Utility => styles.secondary(),
    }
}
