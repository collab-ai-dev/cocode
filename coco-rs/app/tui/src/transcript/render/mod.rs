//! Finalized transcript rendering for native history emission — and the
//! single home of every per-cell renderer (tui-v2 §6.4 "render.rs —
//! the only renderer"; §6.7-2 "no streaming-only renderer exists").
//!
//! Submodules: [`cells_renderer`] (the `&[RenderedCell]` → `Vec<Line>`
//! projection, ex-`widgets::chat::ChatWidget`), the per-category cell
//! renderers ([`assistant`] / [`user`] / [`system`] / [`tool`] /
//! [`tool_result`]), and this module's history/replay renderer +
//! [`HistoryReplayCache`]. `widgets/` composes frames; it depends on
//! `transcript::render`, never the other way around.
//!
//! Phase 3d (§4): consumes the engine-authoritative `&[RenderedCell]`
//! slice from `session.transcript.cells()`. The "messages omitted"
//! counter in [`HistoryReplayLines`] still names messages because
//! truncation occurs at engine-message (not cell) boundaries — a
//! single `Message::Assistant` with text + thinking + tool_use blocks
//! contributes one increment, never three.
pub(crate) mod assistant;
mod cells_renderer;
mod system;
mod tool;
pub(crate) mod tool_result;
mod user;

#[cfg(any(test, feature = "testing"))]
pub(crate) use assistant::clear_committed_markdown_memo_for_tests;
pub(crate) use cells_renderer::CellsRenderer;
pub(crate) use cells_renderer::assistant_stream_lead_marker;
pub(crate) use cells_renderer::attachment_summary_text;
pub(crate) use cells_renderer::compact_file_reference_chip_path;
pub(crate) use cells_renderer::in_flight_tool_lines;
pub(crate) use cells_renderer::mention_summary_lines;
pub(crate) use cells_renderer::nested_memory_chip_path;
// Shared helpers the per-category renderers reach via `super::…` (they were
// items of the old `widgets::chat` parent module).
use cells_renderer::TOOL_OUTPUT_PREVIEW_ROWS;
use cells_renderer::output_result_line;
use cells_renderer::result_line;
use cells_renderer::single_line_capped;
use cells_renderer::transcript_safe_line;

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use coco_messages::Message;
use coco_messages::SystemMessage;
use coco_messages::SystemMessageLevel;
use coco_types::CompactTrigger;
use coco_types::TokenUsage;
use ratatui::text::Line;
use sha2::Digest;
use sha2::Sha256;

use crate::keybinding_resolver::KeybindingHandle;
use crate::state::session::ReasoningMetadata;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;
use crate::transcript::cells::SystemCellKind;
use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::engine::history_insert::HistoryRows;
use coco_tui_ui::engine::history_insert::render_history_rows;
use coco_tui_ui::style::UiStyles;

pub(crate) const DEFAULT_MAX_REFLOW_ROWS: usize = 9_000;
const DEFAULT_REPLAY_CACHE_ENTRIES: usize = 32;
const DEFAULT_REPLAY_CACHE_BYTES: usize = 2 * 1024 * 1024;
const REPLAY_CACHE_MIN_CELLS: usize = 32;
const REPLAY_CACHE_MIN_CONTENT_BYTES: usize = 8 * 1024;
const REPLAY_CACHE_ADMIT_MIN_ELAPSED: Duration = Duration::from_micros(250);
const REPLAY_CACHE_ADMIT_MIN_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryReplayCachePolicy {
    pub(crate) enabled: bool,
    pub(crate) max_entries: usize,
    pub(crate) max_estimated_bytes: usize,
    pub(crate) min_cells: usize,
    pub(crate) min_content_bytes: usize,
    pub(crate) admit_min_render_elapsed: Duration,
    pub(crate) admit_min_result_bytes: usize,
}

impl Default for HistoryReplayCachePolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_entries: DEFAULT_REPLAY_CACHE_ENTRIES,
            max_estimated_bytes: DEFAULT_REPLAY_CACHE_BYTES,
            min_cells: REPLAY_CACHE_MIN_CELLS,
            min_content_bytes: REPLAY_CACHE_MIN_CONTENT_BYTES,
            admit_min_render_elapsed: REPLAY_CACHE_ADMIT_MIN_ELAPSED,
            admit_min_result_bytes: REPLAY_CACHE_ADMIT_MIN_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct HistoryLineRenderOptions<'a> {
    pub(crate) styles: UiStyles<'a>,
    pub(crate) width: u16,
    pub(crate) syntax_highlighting: SyntaxHighlighting,
    pub(crate) show_system_reminders: bool,
    pub(crate) show_thinking: bool,
    /// Session working directory for relative memory-chip paths (`None` ⇒ absolute).
    pub(crate) cwd: Option<&'a str>,
    pub(crate) kb_handle: Option<&'a KeybindingHandle>,
    pub(crate) replay_cache_policy: HistoryReplayCachePolicy,
    /// TUI-side side-cache for reasoning metadata keyed by assistant
    /// message UUID. `None` ⇒ thinking cells render without the
    /// `· <duration> · <tokens>` badge (live append before
    /// `TurnCompleted` arrives).
    pub(crate) reasoning_metadata: Option<&'a HashMap<uuid::Uuid, ReasoningMetadata>>,
    /// TUI-side side-cache for completed-subagent run summaries keyed by
    /// the spawning Agent tool_use_id. `None` ⇒ Agent tool-result cells
    /// render without the `✓ Explore · N tools · …` summary row.
    pub(crate) subagent_summaries:
        Option<&'a HashMap<String, crate::state::session::SubagentRunSummary>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryReplayLines {
    pub(crate) lines: Arc<[Line<'static>]>,
    pub(crate) rows: Arc<HistoryRows>,
    pub(crate) omitted_messages: usize,
    pub(crate) stats: HistoryReplayRenderStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct HistoryReplayRenderStats {
    pub(crate) finalized_render_calls: usize,
    pub(crate) cells_rendered: usize,
    pub(crate) cache_hit: bool,
    pub(crate) cacheable: bool,
    pub(crate) cache_lookup: HistoryReplayCacheLookup,
    pub(crate) cache_skip_reason: Option<HistoryReplayCacheSkipReason>,
    pub(crate) key_build_elapsed_us: u128,
    pub(crate) cache_entries: usize,
    pub(crate) cache_estimated_bytes: usize,
    pub(crate) cell_content_estimated_bytes: usize,
    pub(crate) replay_estimated_bytes: usize,
    pub(crate) cache_evictions: usize,
    pub(crate) cache_admitted: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum HistoryReplayCacheLookup {
    #[default]
    Skipped,
    Hit,
    Miss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryReplayCacheSkipReason {
    BelowReplayThreshold,
    UnsupportedCell,
    AdmissionTooSmall,
    CacheDisabled,
    EntryTooLarge,
}

#[derive(Debug, Clone)]
struct HistoryReplayCacheEntry {
    lines: Arc<[Line<'static>]>,
    rows: Arc<HistoryRows>,
    omitted_messages: usize,
    bytes: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryReplayCache {
    entries: HashMap<HistoryReplayCacheKey, HistoryReplayCacheEntry>,
    order: VecDeque<HistoryReplayCacheKey>,
    max_entries: usize,
    max_bytes: usize,
    bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct HistoryReplayCacheKey {
    max_rows: usize,
    width: u16,
    syntax_enabled: bool,
    show_thinking: bool,
    show_system_reminders: bool,
    theme_hash: u64,
    cell_count: usize,
    content_digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HistoryReplayCacheInsertOutcome {
    inserted: bool,
    estimated_bytes: usize,
    evictions: usize,
    skip_reason: Option<HistoryReplayCacheSkipReason>,
}

impl Default for HistoryReplayCache {
    fn default() -> Self {
        Self::with_limits(DEFAULT_REPLAY_CACHE_ENTRIES, DEFAULT_REPLAY_CACHE_BYTES)
    }
}

impl HistoryReplayCache {
    pub(crate) fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            max_entries,
            max_bytes,
            bytes: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    fn get(&self, key: HistoryReplayCacheKey) -> Option<&HistoryReplayCacheEntry> {
        self.entries.get(&key)
    }

    fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn estimated_bytes(&self) -> usize {
        self.bytes
    }

    fn apply_policy(&mut self, policy: HistoryReplayCachePolicy) {
        self.max_entries = policy.max_entries;
        self.max_bytes = if policy.enabled {
            policy.max_estimated_bytes
        } else {
            0
        };
        self.evict_over_limit();
    }

    fn insert(
        &mut self,
        key: HistoryReplayCacheKey,
        lines: Arc<[Line<'static>]>,
        rows: Arc<HistoryRows>,
        omitted_messages: usize,
    ) -> HistoryReplayCacheInsertOutcome {
        let estimated_bytes = estimate_replay_entry_bytes(&lines, &rows);
        if self.max_entries == 0 || self.max_bytes == 0 {
            return HistoryReplayCacheInsertOutcome {
                inserted: false,
                estimated_bytes,
                evictions: 0,
                skip_reason: Some(HistoryReplayCacheSkipReason::CacheDisabled),
            };
        }

        if estimated_bytes > self.max_bytes {
            return HistoryReplayCacheInsertOutcome {
                inserted: false,
                estimated_bytes,
                evictions: 0,
                skip_reason: Some(HistoryReplayCacheSkipReason::EntryTooLarge),
            };
        }

        if let Some(previous) = self.entries.remove(&key) {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
            self.order.retain(|existing| existing != &key);
        }

        self.order.push_back(key);
        self.bytes = self.bytes.saturating_add(estimated_bytes);
        self.entries.insert(
            key,
            HistoryReplayCacheEntry {
                lines,
                rows,
                omitted_messages,
                bytes: estimated_bytes,
            },
        );
        let evictions = self.evict_over_limit();
        HistoryReplayCacheInsertOutcome {
            inserted: true,
            estimated_bytes,
            evictions,
            skip_reason: None,
        }
    }

    fn evict_over_limit(&mut self) -> usize {
        let mut evictions = 0;
        while self.entries.len() > self.max_entries || self.bytes > self.max_bytes {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
                evictions += 1;
            }
        }
        evictions
    }
}

pub(crate) fn render_finalized_history_lines(
    cells: &[RenderedCell],
    options: HistoryLineRenderOptions<'_>,
) -> Vec<Line<'static>> {
    let mut chat = CellsRenderer::new(cells, options.styles)
        .show_system_reminders(options.show_system_reminders)
        .show_thinking(options.show_thinking)
        .width(options.width)
        .syntax_highlighting(options.syntax_highlighting)
        .native_history_presentation()
        .cwd(options.cwd);
    if let Some(kb_handle) = options.kb_handle {
        chat = chat.kb_handle(kb_handle);
    }
    if let Some(meta) = options.reasoning_metadata {
        chat = chat.reasoning_metadata(meta);
    }
    if let Some(summaries) = options.subagent_summaries {
        chat = chat.subagent_summaries(summaries);
    }
    chat.build_lines_owned()
}

pub(crate) fn render_replay_history_lines(
    cells: &[RenderedCell],
    options: HistoryLineRenderOptions<'_>,
    max_rows: usize,
) -> HistoryReplayLines {
    let mut stats = HistoryReplayRenderStats::default();
    let replay = render_replay_history_lines_uncached(cells, options, max_rows, &mut stats);
    let rows = Arc::new(render_history_rows(replay.lines.clone(), options.width));
    HistoryReplayLines {
        lines: Arc::from(replay.lines),
        rows,
        omitted_messages: replay.omitted_messages,
        stats,
    }
}

pub(crate) fn render_replay_history_lines_cached(
    cells: &[RenderedCell],
    options: HistoryLineRenderOptions<'_>,
    max_rows: usize,
    cache: &mut HistoryReplayCache,
) -> HistoryReplayLines {
    let policy = options.replay_cache_policy;
    cache.apply_policy(policy);
    let content_bytes = estimate_cell_content_bytes(cells);
    if !policy.enabled || policy.max_entries == 0 || policy.max_estimated_bytes == 0 {
        let mut replay = render_replay_history_lines(cells, options, max_rows);
        replay.stats.cacheable = false;
        replay.stats.cache_skip_reason = Some(HistoryReplayCacheSkipReason::CacheDisabled);
        replay.stats.cache_entries = cache.entry_count();
        replay.stats.cache_estimated_bytes = cache.estimated_bytes();
        replay.stats.cell_content_estimated_bytes = content_bytes;
        replay.stats.replay_estimated_bytes = replay.rows.estimated_bytes();
        return replay;
    }

    if cells.len() < policy.min_cells && content_bytes < policy.min_content_bytes {
        let mut replay = render_replay_history_lines(cells, options, max_rows);
        replay.stats.cacheable = false;
        replay.stats.cache_skip_reason = Some(HistoryReplayCacheSkipReason::BelowReplayThreshold);
        replay.stats.cache_entries = cache.entry_count();
        replay.stats.cache_estimated_bytes = cache.estimated_bytes();
        replay.stats.cell_content_estimated_bytes = content_bytes;
        replay.stats.replay_estimated_bytes = replay.rows.estimated_bytes();
        return replay;
    };

    let key_started = Instant::now();
    let Some(key) = replay_cache_key(cells, options, max_rows) else {
        let key_build_elapsed_us = key_started.elapsed().as_micros();
        let mut replay = render_replay_history_lines(cells, options, max_rows);
        replay.stats.cacheable = false;
        replay.stats.cache_skip_reason = Some(HistoryReplayCacheSkipReason::UnsupportedCell);
        replay.stats.key_build_elapsed_us = key_build_elapsed_us;
        replay.stats.cache_entries = cache.entry_count();
        replay.stats.cache_estimated_bytes = cache.estimated_bytes();
        replay.stats.cell_content_estimated_bytes = content_bytes;
        replay.stats.replay_estimated_bytes = replay.rows.estimated_bytes();
        return replay;
    };
    let key_build_elapsed_us = key_started.elapsed().as_micros();

    if let Some(hit) = cache.get(key) {
        return HistoryReplayLines {
            lines: hit.lines.clone(),
            rows: hit.rows.clone(),
            omitted_messages: hit.omitted_messages,
            stats: HistoryReplayRenderStats {
                cache_hit: true,
                cacheable: true,
                cache_lookup: HistoryReplayCacheLookup::Hit,
                key_build_elapsed_us,
                cache_entries: cache.entry_count(),
                cache_estimated_bytes: cache.estimated_bytes(),
                cell_content_estimated_bytes: content_bytes,
                replay_estimated_bytes: hit.bytes,
                ..HistoryReplayRenderStats::default()
            },
        };
    }

    let render_started = Instant::now();
    let mut replay = render_replay_history_lines(cells, options, max_rows);
    let render_elapsed = render_started.elapsed();
    let estimated_bytes = replay.rows.estimated_bytes();
    replay.stats.cacheable = true;
    replay.stats.cache_lookup = HistoryReplayCacheLookup::Miss;
    replay.stats.key_build_elapsed_us = key_build_elapsed_us;
    replay.stats.cell_content_estimated_bytes = content_bytes;
    replay.stats.replay_estimated_bytes = estimated_bytes;
    if render_elapsed >= policy.admit_min_render_elapsed
        || estimated_bytes >= policy.admit_min_result_bytes
    {
        let outcome = cache.insert(
            key,
            replay.lines.clone(),
            replay.rows.clone(),
            replay.omitted_messages,
        );
        replay.stats.replay_estimated_bytes = outcome.estimated_bytes;
        replay.stats.cache_admitted = outcome.inserted;
        replay.stats.cache_evictions = outcome.evictions;
        replay.stats.cache_skip_reason = outcome.skip_reason;
    } else {
        replay.stats.cache_skip_reason = Some(HistoryReplayCacheSkipReason::AdmissionTooSmall);
    }
    replay.stats.cache_entries = cache.entry_count();
    replay.stats.cache_estimated_bytes = cache.estimated_bytes();
    replay
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UncachedHistoryReplayLines {
    lines: Vec<Line<'static>>,
    omitted_messages: usize,
}

fn render_replay_history_lines_uncached(
    cells: &[RenderedCell],
    options: HistoryLineRenderOptions<'_>,
    max_rows: usize,
    stats: &mut HistoryReplayRenderStats,
) -> UncachedHistoryReplayLines {
    let all_lines = render_counted(cells, options, stats);
    if rendered_line_row_count(&all_lines, options.width) <= max_rows || cells.is_empty() {
        return UncachedHistoryReplayLines {
            lines: all_lines,
            omitted_messages: 0,
        };
    }

    // Truncate at engine-message UUID boundaries so the "N older messages
    // omitted" marker counts engine messages, not cells.
    //
    // Dropping more leading messages can only shrink the rendered suffix, so
    // "suffix + marker fits within max_rows" is monotonic in the number of
    // omitted messages. Binary-search the smallest omission that fits rather
    // than re-rendering every candidate suffix forward — the old linear walk
    // re-wrapped the whole remaining transcript on each step (O(messages ×
    // cells)); this is O(messages × cells × log messages) and renders the
    // chosen suffix at most a handful of times.
    let message_starts: Vec<usize> =
        crate::transcript::cells::engine_message_starts(cells).collect();
    let n = message_starts.len();

    let mut fits = |omitted: usize| -> bool {
        let start = message_starts[omitted];
        let mut lines = replay_truncation_marker(omitted);
        lines.extend(render_counted(&cells[start..], options, stats));
        rendered_line_row_count(&lines, options.width) <= max_rows
    };

    // Smallest `omitted` in `1..n` whose suffix fits; `n` ⇒ none fits.
    let mut lo = 1;
    let mut hi = n;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if fits(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }

    if lo < n {
        let start = message_starts[lo];
        let mut lines = replay_truncation_marker(lo);
        lines.extend(render_counted(&cells[start..], options, stats));
        UncachedHistoryReplayLines {
            lines,
            omitted_messages: lo,
        }
    } else {
        // Even keeping only the final message overflows the cap; emit just
        // the marker (matches the prior fallback behaviour).
        UncachedHistoryReplayLines {
            lines: replay_truncation_marker(n),
            omitted_messages: n,
        }
    }
}

fn render_counted(
    cells: &[RenderedCell],
    options: HistoryLineRenderOptions<'_>,
    stats: &mut HistoryReplayRenderStats,
) -> Vec<Line<'static>> {
    stats.finalized_render_calls += 1;
    stats.cells_rendered += cells.len();
    render_finalized_history_lines(cells, options)
}

fn rendered_line_row_count(lines: &[Line<'static>], width: u16) -> usize {
    // The same `Paragraph::line_count` + `Wrap { trim: false }` math
    // `render_history_rows` uses for its height — identical by construction
    // (including the `u16::MAX` clamp) — without rasterizing a throwaway
    // `Buffer`. This runs on full-transcript input once per binary-search
    // step of the capped-replay fit, where the Buffer pass was multi-ms of
    // pure allocation + style writes.
    ratatui::widgets::Paragraph::new(lines.to_vec())
        .wrap(ratatui::widgets::Wrap { trim: false })
        .line_count(width)
        .min(u16::MAX as usize)
}

fn replay_cache_key(
    cells: &[RenderedCell],
    options: HistoryLineRenderOptions<'_>,
    max_rows: usize,
) -> Option<HistoryReplayCacheKey> {
    let compact_boundary_shortcut = cells
        .iter()
        .any(is_compact_boundary_cell)
        .then(|| replay_compact_boundary_shortcut(options.kb_handle));
    let compact_boundary_shortcut = compact_boundary_shortcut.as_deref();
    let mut hasher = Sha256::new();
    // kb-resolved strings render into cell BODIES (the tool-result
    // `(<chord> to expand)` hint and the thinking toggle hint) — fold the
    // resolved chords in so a rebind invalidates instead of serving stale
    // hint text.
    hash_str(
        &mut hasher,
        &replay_compact_boundary_shortcut(options.kb_handle),
    );
    hash_str(
        &mut hasher,
        &replay_thinking_toggle_shortcut(options.kb_handle),
    );
    for cell in cells {
        hash_cacheable_cell(cell, options, compact_boundary_shortcut, &mut hasher)?;
    }
    let content_digest: [u8; 32] = hasher.finalize().into();
    Some(HistoryReplayCacheKey {
        max_rows,
        width: options.width,
        syntax_enabled: options.syntax_highlighting.is_enabled(),
        show_thinking: options.show_thinking,
        show_system_reminders: options.show_system_reminders,
        theme_hash: options.styles.theme_hash(),
        cell_count: cells.len(),
        content_digest,
    })
}

fn hash_cacheable_cell(
    cell: &RenderedCell,
    options: HistoryLineRenderOptions<'_>,
    compact_boundary_shortcut: Option<&str>,
    hasher: &mut Sha256,
) -> Option<()> {
    hash_uuid(hasher, cell.message_uuid);
    match &cell.kind {
        CellKind::UserText { text } => {
            hash_u8(hasher, 0);
            hash_str(hasher, text);
        }
        CellKind::AssistantText { text, model } => {
            hash_u8(hasher, 1);
            hash_str(hasher, text);
            hash_str(hasher, model);
        }
        CellKind::System(kind) => {
            hash_u8(hasher, 2);
            hash_system_cell(
                kind,
                cell.source.as_ref(),
                compact_boundary_shortcut,
                hasher,
            )?;
        }
        // Tool / thinking / attachment cells used to bail to the uncached
        // path, which made the replay cache dead for virtually every real
        // session (any transcript with a tool call missed on every
        // resize/theme replay). The replay render reads no live execution
        // state (`render_finalized_history_lines` never sets
        // `tool_executions`, so elapsed badges are absent by construction);
        // the only extra-source input is the reasoning side-cache, hashed
        // below.
        CellKind::AssistantThinking {
            text,
            metadata_anchor,
        } => {
            hash_u8(hasher, 3);
            hash_str(hasher, text);
            // Side-cache badge (`Thinking · <duration> · <tokens>`): absence
            // (live append before `TurnCompleted`) renders differently.
            hash_bool(hasher, *metadata_anchor);
            if !*metadata_anchor {
                hash_u8(hasher, 0);
            } else {
                match options
                    .reasoning_metadata
                    .and_then(|cache| cache.get(&cell.message_uuid))
                {
                    Some(meta) => {
                        hash_u8(hasher, 1);
                        hash_i64(hasher, meta.duration_ms.unwrap_or(-1));
                        hash_i64(hasher, meta.reasoning_tokens);
                    }
                    None => {
                        hash_u8(hasher, 0);
                        hash_i64(hasher, assistant_source_reasoning_tokens(cell).unwrap_or(0));
                    }
                }
            }
        }
        CellKind::AssistantRedactedThinking => hash_u8(hasher, 4),
        CellKind::ToolUse { tool_name, call_id } => {
            hash_u8(hasher, 5);
            hash_str(hasher, tool_name);
            hash_str(hasher, call_id);
            hash_message_source(hasher, cell.source.as_ref())?;
        }
        CellKind::ToolResult { call_id } => {
            hash_u8(hasher, 6);
            hash_message_source(hasher, cell.source.as_ref())?;
            // Fold the subagent run summary (keyed by the spawning
            // tool_use_id) into the key so a late-arriving `TaskCompleted`
            // re-renders the Agent tool-result cell instead of serving a
            // stale cached body without the `✓ Explore · …` row.
            match options
                .subagent_summaries
                .and_then(|cache| cache.get(call_id))
            {
                Some(s) => {
                    hash_u8(hasher, 1);
                    hash_i64(hasher, s.tool_count as i64);
                    hash_i64(hasher, s.duration_ms);
                    hash_i64(hasher, s.input_tokens);
                    hash_i64(hasher, s.output_tokens);
                    hash_i64(hasher, s.cache_read_tokens);
                    hash_i64(hasher, (s.cost_usd * 1_000_000.0) as i64);
                }
                None => hash_u8(hasher, 0),
            }
        }
        CellKind::Attachment => {
            hash_u8(hasher, 8);
            hash_message_source(hasher, cell.source.as_ref())?;
        }
    }
    Some(())
}

fn assistant_source_reasoning_tokens(cell: &RenderedCell) -> Option<i64> {
    let Message::Assistant(assistant) = cell.source.as_ref() else {
        return None;
    };
    let tokens = assistant.usage.as_ref()?.output_tokens.reasoning;
    (tokens > 0).then_some(tokens)
}

/// Hash the full serialized source message — conservative over-keying for
/// cells whose render derives many fields from the source (tool input
/// previews, result bodies, display data, attachment chips). Over-keying can
/// only cause a miss, never a wrong hit.
fn hash_message_source(hasher: &mut Sha256, source: &Message) -> Option<()> {
    let json = serde_json::to_vec(source).ok()?;
    hasher.update((json.len() as u64).to_le_bytes());
    hasher.update(&json);
    Some(())
}

fn replay_thinking_toggle_shortcut(kb_handle: Option<&KeybindingHandle>) -> String {
    kb_handle
        .and_then(|handle| {
            handle.display_for(
                &coco_keybindings::KeybindingAction::ChatThinkingToggle,
                crate::keybinding_bridge::KeybindingContext::Chat,
            )
        })
        .unwrap_or_else(|| "F2".to_string())
}

fn hash_system_cell(
    kind: &SystemCellKind,
    source: &Message,
    compact_boundary_shortcut: Option<&str>,
    hasher: &mut Sha256,
) -> Option<()> {
    let Message::System(system) = source else {
        return None;
    };
    match (kind, system) {
        (SystemCellKind::Informational, SystemMessage::Informational(m)) => {
            hash_u8(hasher, 0);
            hash_system_level(hasher, m.level);
            hash_str(hasher, &m.title);
            hash_str(hasher, &m.message);
        }
        (SystemCellKind::ApiError, SystemMessage::ApiError(m)) => {
            hash_u8(hasher, 1);
            hash_str(hasher, &m.error);
            hash_option_i32(hasher, m.status_code);
        }
        (SystemCellKind::MicrocompactBoundary, SystemMessage::MicrocompactBoundary(_)) => {
            hash_u8(hasher, 2);
        }
        (SystemCellKind::PermissionRetry, SystemMessage::PermissionRetry(m)) => {
            hash_u8(hasher, 3);
            hash_str(hasher, &m.tool_name);
            hash_str(hasher, &m.message);
        }
        (SystemCellKind::BridgeStatus, SystemMessage::BridgeStatus(m)) => {
            hash_u8(hasher, 4);
            hash_bool(hasher, m.connected);
            hash_option_str(hasher, m.message.as_deref());
        }
        (SystemCellKind::MemorySaved, SystemMessage::MemorySaved(m)) => {
            hash_u8(hasher, 5);
            hash_str_vec(hasher, &m.written_paths);
            hash_str(hasher, &m.verb);
        }
        (SystemCellKind::AwaySummary, SystemMessage::AwaySummary(m)) => {
            hash_u8(hasher, 6);
            hash_str(hasher, &m.summary);
        }
        (SystemCellKind::AgentsKilled, SystemMessage::AgentsKilled(m)) => {
            hash_u8(hasher, 7);
            hash_i32(hasher, m.count);
        }
        (SystemCellKind::ApiMetrics, SystemMessage::ApiMetrics(m)) => {
            hash_u8(hasher, 8);
            hash_token_usage(hasher, m.usage);
            hash_str(hasher, &m.model);
            hash_option_f64(hasher, m.cost_usd);
        }
        (SystemCellKind::StopHookSummary, SystemMessage::StopHookSummary(m)) => {
            hash_u8(hasher, 9);
            hash_str(hasher, &m.hook_name);
            hash_str(hasher, &m.outcome);
        }
        (SystemCellKind::TurnDuration, SystemMessage::TurnDuration(m)) => {
            hash_u8(hasher, 10);
            hash_i64(hasher, m.duration_ms);
        }
        (SystemCellKind::ScheduledTaskFire, SystemMessage::ScheduledTaskFire(m)) => {
            hash_u8(hasher, 11);
            hash_str(hasher, &m.task_id);
            hash_str(hasher, &m.schedule);
        }
        (SystemCellKind::UserInterruption { for_tool_use }, SystemMessage::UserInterruption(m))
            if *for_tool_use == m.for_tool_use =>
        {
            hash_u8(hasher, 12);
            hash_bool(hasher, m.for_tool_use);
        }
        (SystemCellKind::CompactBoundary, SystemMessage::CompactBoundary(m)) => {
            hash_u8(hasher, 13);
            hash_i64(hasher, m.tokens_before);
            hash_i64(hasher, m.tokens_after);
            hash_compact_trigger(hasher, m.trigger);
            hash_option_str(hasher, m.user_context.as_deref());
            hash_option_i32(hasher, m.messages_summarized);
            hash_str_vec(hasher, &m.pre_compact_discovered_tools);
            hash_bool(hasher, m.preserved_segment.is_some());
            if let Some(segment) = &m.preserved_segment {
                hash_uuid(hasher, segment.head_uuid);
                hash_uuid(hasher, segment.anchor_uuid);
                hash_uuid(hasher, segment.tail_uuid);
            }
            hash_str(hasher, compact_boundary_shortcut?);
        }
        (SystemCellKind::LocalCommand, SystemMessage::LocalCommand(_))
        | (SystemCellKind::ContextUsage, SystemMessage::ContextUsage(_)) => {
            // Body-heavy variants (command output / usage table): hash the
            // full serialized source instead of enumerating fields — a missed
            // field would serve stale rows, over-keying only misses.
            hash_u8(hasher, 14);
            hash_message_source(hasher, source)?;
        }
        // Kind/source mismatch: defensive — leave it on the uncached path.
        _ => return None,
    }
    Some(())
}

fn is_compact_boundary_cell(cell: &RenderedCell) -> bool {
    matches!(
        &cell.kind,
        CellKind::System(SystemCellKind::CompactBoundary)
    )
}

fn replay_compact_boundary_shortcut(kb_handle: Option<&KeybindingHandle>) -> String {
    kb_handle
        .and_then(|handle| {
            handle.display_for(
                &coco_keybindings::KeybindingAction::AppToggleTranscript,
                crate::keybinding_bridge::KeybindingContext::Chat,
            )
        })
        .unwrap_or_else(|| "ctrl+o".to_string())
}

fn hash_system_level(hasher: &mut Sha256, level: SystemMessageLevel) {
    hash_u8(
        hasher,
        match level {
            SystemMessageLevel::Info => 0,
            SystemMessageLevel::Warning => 1,
            SystemMessageLevel::Error => 2,
        },
    );
}

fn hash_compact_trigger(hasher: &mut Sha256, trigger: CompactTrigger) {
    hash_u8(
        hasher,
        match trigger {
            CompactTrigger::Manual => 0,
            CompactTrigger::Auto => 1,
            CompactTrigger::Reactive => 2,
            CompactTrigger::TimeBased => 3,
            CompactTrigger::SessionMemory => 4,
            CompactTrigger::ContextCollapse => 5,
        },
    );
}

fn hash_token_usage(hasher: &mut Sha256, usage: TokenUsage) {
    hash_i64(hasher, usage.input_tokens.total);
    hash_i64(hasher, usage.input_tokens.no_cache);
    hash_i64(hasher, usage.input_tokens.cache_read);
    hash_i64(hasher, usage.input_tokens.cache_write);
    hash_i64(hasher, usage.output_tokens.total);
    hash_i64(hasher, usage.output_tokens.text);
    hash_i64(hasher, usage.output_tokens.reasoning);
}

fn hash_uuid(hasher: &mut Sha256, uuid: uuid::Uuid) {
    hasher.update(uuid.as_bytes());
}

fn hash_str(hasher: &mut Sha256, value: &str) {
    hash_usize(hasher, value.len());
    hasher.update(value.as_bytes());
}

fn hash_option_str(hasher: &mut Sha256, value: Option<&str>) {
    hash_bool(hasher, value.is_some());
    if let Some(value) = value {
        hash_str(hasher, value);
    }
}

fn hash_str_vec(hasher: &mut Sha256, values: &[String]) {
    hash_usize(hasher, values.len());
    for value in values {
        hash_str(hasher, value);
    }
}

fn hash_option_i32(hasher: &mut Sha256, value: Option<i32>) {
    hash_bool(hasher, value.is_some());
    if let Some(value) = value {
        hash_i32(hasher, value);
    }
}

fn hash_option_f64(hasher: &mut Sha256, value: Option<f64>) {
    hash_bool(hasher, value.is_some());
    if let Some(value) = value {
        hasher.update(value.to_bits().to_le_bytes());
    }
}

fn hash_bool(hasher: &mut Sha256, value: bool) {
    hash_u8(hasher, u8::from(value));
}

fn hash_u8(hasher: &mut Sha256, value: u8) {
    hasher.update([value]);
}

fn hash_usize(hasher: &mut Sha256, value: usize) {
    hasher.update((value as u64).to_le_bytes());
}

fn hash_i32(hasher: &mut Sha256, value: i32) {
    hasher.update(value.to_le_bytes());
}

fn hash_i64(hasher: &mut Sha256, value: i64) {
    hasher.update(value.to_le_bytes());
}

fn estimate_replay_entry_bytes(lines: &[Line<'_>], rows: &HistoryRows) -> usize {
    estimate_lines_bytes(lines).saturating_add(rows.estimated_bytes())
}

pub(super) fn estimate_lines_bytes(lines: &[Line<'_>]) -> usize {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.len() + 16)
                .sum::<usize>()
                + 16
        })
        .sum()
}

fn estimate_cell_content_bytes(cells: &[RenderedCell]) -> usize {
    cells.iter().map(estimate_cell_bytes).sum()
}

fn estimate_cell_bytes(cell: &RenderedCell) -> usize {
    match &cell.kind {
        CellKind::UserText { text }
        | CellKind::AssistantText { text, .. }
        | CellKind::AssistantThinking { text, .. } => text.len(),
        CellKind::ToolUse { call_id, tool_name } => call_id.len() + tool_name.len(),
        CellKind::ToolResult { call_id } => call_id.len(),
        CellKind::System(kind) => estimate_system_cell_bytes(kind, cell.source.as_ref()),
        CellKind::AssistantRedactedThinking | CellKind::Attachment => 0,
    }
}

fn estimate_system_cell_bytes(kind: &SystemCellKind, source: &Message) -> usize {
    let Message::System(system) = source else {
        return 0;
    };
    match (kind, system) {
        (SystemCellKind::Informational, SystemMessage::Informational(m)) => {
            m.title.len() + m.message.len()
        }
        (SystemCellKind::ApiError, SystemMessage::ApiError(m)) => m.error.len(),
        (SystemCellKind::CompactBoundary, SystemMessage::CompactBoundary(m)) => {
            m.user_context.as_ref().map_or(0, String::len)
                + m.pre_compact_discovered_tools
                    .iter()
                    .map(String::len)
                    .sum::<usize>()
        }
        (SystemCellKind::LocalCommand, SystemMessage::LocalCommand(m)) => {
            m.command.len() + m.output.len()
        }
        (SystemCellKind::PermissionRetry, SystemMessage::PermissionRetry(m)) => {
            m.tool_name.len() + m.message.len()
        }
        (SystemCellKind::BridgeStatus, SystemMessage::BridgeStatus(m)) => {
            m.message.as_ref().map_or(0, String::len)
        }
        (SystemCellKind::MemorySaved, SystemMessage::MemorySaved(m)) => {
            m.verb.len() + m.written_paths.iter().map(String::len).sum::<usize>()
        }
        (SystemCellKind::AwaySummary, SystemMessage::AwaySummary(m)) => m.summary.len(),
        (SystemCellKind::ApiMetrics, SystemMessage::ApiMetrics(m)) => m.model.len(),
        (SystemCellKind::StopHookSummary, SystemMessage::StopHookSummary(m)) => {
            m.hook_name.len() + m.outcome.len()
        }
        (SystemCellKind::ScheduledTaskFire, SystemMessage::ScheduledTaskFire(m)) => {
            m.task_id.len() + m.schedule.len()
        }
        (SystemCellKind::MicrocompactBoundary, SystemMessage::MicrocompactBoundary(_))
        | (SystemCellKind::AgentsKilled, SystemMessage::AgentsKilled(_))
        | (SystemCellKind::TurnDuration, SystemMessage::TurnDuration(_))
        | (SystemCellKind::UserInterruption { .. }, SystemMessage::UserInterruption(_)) => 0,
        _ => 0,
    }
}

fn replay_truncation_marker(omitted_messages: usize) -> Vec<Line<'static>> {
    vec![
        Line::from(format!(
            "... {omitted_messages} older messages retained in transcript, not replayed"
        )),
        Line::from("    open transcript pager for full history"),
        Line::default(),
    ]
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
