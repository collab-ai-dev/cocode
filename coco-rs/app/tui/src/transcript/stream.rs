//! Source-backed renderer for the active assistant stream.
//!
//! Streaming deltas append raw source quickly, but repaint cadence can be much
//! higher than semantic changes. This controller asks `coco-tui-markdown` for a
//! conservative stable source prefix, appends newly stable regions to the
//! cached rows, and only re-renders the mutable tail.

use std::hash::Hash;
use std::hash::Hasher;
use std::time::Instant;

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::style::UiStyles;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;

use crate::transcript::render::assistant::ASSISTANT_DOT;
use crate::transcript::render::assistant::CommittedAssistantMarkdownOptions;
use crate::transcript::render::assistant::render_stream_stable_assistant_markdown_continuation_with_links;
use crate::transcript::render::assistant::render_stream_stable_assistant_markdown_with_links;
use crate::transcript::render::assistant_stream_lead_marker;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StreamRenderKey(u64);

impl StreamRenderKey {
    /// Key over every line-affecting input of the committed renderer that can
    /// vary at runtime — width, syntax enablement, theme. The source text is
    /// deliberately not part of the key (it gates *how* rows were rendered,
    /// not *what*); body indent and the streaming flag are constants of the
    /// committed assistant render by construction
    /// (`render_committed_assistant_markdown`).
    pub(crate) fn committed(
        styles: UiStyles<'_>,
        width: u16,
        syntax_highlighting: SyntaxHighlighting,
        hyperlinks_enabled: bool,
    ) -> Self {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        width.hash(&mut h);
        syntax_highlighting.hash(&mut h);
        hyperlinks_enabled.hash(&mut h);
        styles.theme_hash().hash(&mut h);
        Self(h.finish())
    }
}

/// The single record of in-flight assistant rows already inserted into native
/// scrollback (tui-v2 §6.7-10). Both the live-tail increment (`surface::stream`)
/// and the anchored finalize (`transcript::emission`) compute against THIS one
/// value — there is no second copy — so §6.7-5 ("rows enter scrollback exactly
/// once") holds by construction rather than by agreement between two structs.
///
/// Owned by `SurfaceStreamDriver`; the finalize reads it through
/// `SurfaceStreamDriver::commit`. It is advanced only by a successful stream
/// insert and cleared only when those rows actually leave scrollback (replay /
/// reset) or the finalize consumes them — never by a transient `streaming ==
/// None` frame (that benign clear is what re-committed already-present rows and
/// duplicated them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScrollbackStreamCommit {
    /// Source bytes whose rendered rows are already in scrollback. The finalize
    /// anchors the canonical assistant text with `text.starts_with(source_prefix)`;
    /// the live tail re-validates the same way — content identity, not length,
    /// so a coalesced turn boundary cannot re-attribute the prefix to a new turn.
    pub(crate) source_prefix: String,
    /// Number of rendered rows already in scrollback — the suffix start the
    /// finalize appends from and the increment start the live tail emits from.
    pub(crate) line_len: usize,
    /// Render key those rows were produced under; a mismatch means the rows are
    /// stale (theme / width / syntax changed) and the surface must replay.
    pub(crate) render_key: StreamRenderKey,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamRenderInput<'a> {
    pub(crate) source: &'a str,
    /// Identity of `source` (`StreamingState::visible_generation`):
    /// process-globally unique per visible-document state, so an equal value
    /// guarantees `source` is byte-identical to the previously processed one
    /// and the controller can skip its O(doc) prefix scan (the common
    /// no-reveal spinner frame).
    pub(crate) generation: u64,
    pub(crate) styles: UiStyles<'a>,
    pub(crate) width: u16,
    pub(crate) syntax_highlighting: SyntaxHighlighting,
    pub(crate) hyperlinks_enabled: bool,
}

/// One frame's view of the stream render state, borrowing the controller's
/// cached line vectors. `stable_lines` is the authoritative committed-renderer
/// output for the stable source prefix; `tail_lines` is the mutable-tail
/// render. Consumers clone exactly the slices they need instead of receiving
/// (and re-cloning) a rebuilt concatenation every frame.
#[derive(Debug)]
pub(crate) struct StreamRenderProjection<'a> {
    pub(crate) stable_lines: &'a [Line<'static>],
    pub(crate) stable_links: &'a [coco_tui_markdown::LinkSpan],
    pub(crate) tail_lines: &'a [Line<'static>],
    pub(crate) stable_source_len: usize,
    pub(crate) render_key: StreamRenderKey,
    /// Whether this frame was served from the generation cache (no O(doc)
    /// scan, no re-render). Surfaced into the `prepare_native_frame` perf
    /// stage log so production traces can verify the no-reveal fast path.
    pub(crate) cache_hit: bool,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct StreamRenderController {
    render_key: Option<StreamRenderKey>,
    /// Generation of the last processed `StreamRenderInput.source` — equal
    /// generation + equal render key means the cached projection is exact
    /// (the input identity is process-globally unique), so a no-reveal frame
    /// skips the `starts_with` memcmp and the `stable_prefix_end` scan.
    last_generation: Option<u64>,
    source: String,
    stable_prefix_tracker: coco_tui_markdown::StablePrefixTracker,
    stable_prefix_end: usize,
    stable_lines: Vec<Line<'static>>,
    stable_links: Vec<coco_tui_markdown::LinkSpan>,
    tail_source_start: usize,
    tail_source: String,
    tail_lines: Vec<Line<'static>>,
    #[cfg(test)]
    stable_rendered_source_bytes: usize,
}

impl StreamRenderController {
    pub(crate) fn render_projection(
        &mut self,
        input: StreamRenderInput<'_>,
    ) -> StreamRenderProjection<'_> {
        if input.source.is_empty() {
            self.clear();
            return StreamRenderProjection {
                stable_lines: &[],
                stable_links: &[],
                tail_lines: &[],
                stable_source_len: 0,
                render_key: StreamRenderKey::default(),
                cache_hit: false,
            };
        }

        let render_key = StreamRenderKey::committed(
            input.styles,
            input.width,
            input.syntax_highlighting,
            input.hyperlinks_enabled,
        );
        let cache_hit =
            self.last_generation == Some(input.generation) && self.render_key == Some(render_key);
        if !cache_hit {
            let render_reset =
                self.render_key != Some(render_key) || !input.source.starts_with(&self.source);
            let stable_end = if render_reset {
                self.reset_for_key(render_key, input.source);
                self.stable_prefix_tracker.push(input.source)
            } else {
                let appended = &input.source[self.source.len()..];
                self.source.push_str(appended);
                self.stable_prefix_tracker.push(appended)
            };

            if stable_end > self.stable_prefix_end {
                let stable_start = self.stable_prefix_end;
                let requires_full_render = self.stable_prefix_tracker.requires_document_context();
                let (rendered, replace) = if requires_full_render {
                    (
                        render_committed_stable_region(
                            &self.source[..stable_end],
                            input,
                            StableRegionPosition::Initial,
                        ),
                        true,
                    )
                } else {
                    let position = if stable_start == 0 {
                        StableRegionPosition::Initial
                    } else {
                        StableRegionPosition::Continuation
                    };
                    (
                        render_committed_stable_region(
                            &self.source[stable_start..stable_end],
                            input,
                            position,
                        ),
                        false,
                    )
                };
                let marker_only =
                    stable_start == 0 && stable_render_is_marker_only(&rendered.lines);
                if !marker_only {
                    if replace {
                        self.stable_lines = rendered.lines;
                        self.stable_links = rendered.links;
                    } else {
                        if stable_start > 0 && !rendered.lines.is_empty() {
                            self.stable_lines.push(Line::default());
                        }
                        let line_offset = self.stable_lines.len();
                        self.stable_lines.extend(rendered.lines);
                        self.stable_links
                            .extend(rendered.links.into_iter().map(|mut link| {
                                link.line = link.line.saturating_add(line_offset);
                                link
                            }));
                    }
                    #[cfg(test)]
                    {
                        self.stable_rendered_source_bytes += if requires_full_render {
                            stable_end
                        } else {
                            stable_end - stable_start
                        };
                    }
                    self.stable_prefix_end = stable_end;
                }
            }

            let tail_source = &self.source[self.stable_prefix_end..];
            if self.tail_source_start != self.stable_prefix_end || self.tail_source != tail_source {
                self.tail_source_start = self.stable_prefix_end;
                self.tail_source.clear();
                self.tail_source.push_str(tail_source);
                self.tail_lines = render_mutable_tail_region(
                    &self.tail_source,
                    input,
                    self.stable_lines.is_empty(),
                );
            }
            self.last_generation = Some(input.generation);
        }

        StreamRenderProjection {
            stable_lines: &self.stable_lines,
            stable_links: &self.stable_links,
            tail_lines: &self.tail_lines,
            stable_source_len: self.stable_prefix_end,
            render_key,
            cache_hit,
        }
    }

    fn reset_for_key(&mut self, render_key: StreamRenderKey, source: &str) {
        self.render_key = Some(render_key);
        self.source.clear();
        self.source.push_str(source);
        self.stable_prefix_tracker = coco_tui_markdown::StablePrefixTracker::default();
        self.stable_prefix_end = 0;
        self.stable_lines.clear();
        self.stable_links.clear();
        self.tail_source_start = 0;
        self.tail_source.clear();
        self.tail_lines.clear();
        #[cfg(test)]
        {
            self.stable_rendered_source_bytes = 0;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.render_key = None;
        self.last_generation = None;
        self.source.clear();
        self.stable_prefix_tracker = coco_tui_markdown::StablePrefixTracker::default();
        self.stable_prefix_end = 0;
        self.stable_lines.clear();
        self.stable_links.clear();
        self.tail_source_start = 0;
        self.tail_source.clear();
        self.tail_lines.clear();
        #[cfg(test)]
        {
            self.stable_rendered_source_bytes = 0;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StableRegionPosition {
    Initial,
    Continuation,
}

fn stable_render_is_marker_only(lines: &[Line<'_>]) -> bool {
    lines.len() == 1
        && lines[0].spans.len() == 1
        && lines[0].spans[0].content.as_ref() == ASSISTANT_DOT
}

fn render_committed_stable_region(
    source: &str,
    input: StreamRenderInput<'_>,
    position: StableRegionPosition,
) -> coco_tui_markdown::MarkdownRender {
    if source.is_empty() {
        return coco_tui_markdown::MarkdownRender {
            lines: Vec::new(),
            links: Vec::new(),
        };
    }
    let started = Instant::now();
    // Memo-bypassed (the controller caches `stable_lines`); row-identical to the
    // committed finalize render, which is what makes the mid-stream→finalize
    // handoff sound (tui-v2 §6.2).
    let options = CommittedAssistantMarkdownOptions {
        styles: input.styles,
        width: input.width,
        syntax_highlighting: input.syntax_highlighting,
    };
    let rendered = match position {
        StableRegionPosition::Initial => render_stream_stable_assistant_markdown_with_links(
            source,
            options,
            input.hyperlinks_enabled,
        ),
        StableRegionPosition::Continuation => {
            render_stream_stable_assistant_markdown_continuation_with_links(
                source,
                options,
                input.hyperlinks_enabled,
            )
        }
    };
    let elapsed = started.elapsed();
    tracing::debug!(
        target: "tui::streaming",
        region = "stable_append",
        position = ?position,
        source_bytes = source.len(),
        lines = rendered.lines.len(),
        elapsed_us = elapsed.as_micros(),
        width = input.width,
        "render streaming markdown region",
    );
    rendered
}

fn render_mutable_tail_region(
    source: &str,
    input: StreamRenderInput<'_>,
    include_marker: bool,
) -> Vec<Line<'static>> {
    if source.is_empty() {
        return Vec::new();
    }
    let opts = coco_tui_markdown::MarkdownOptions::new(
        input.styles,
        input.width,
        input.syntax_highlighting,
    )
    .streaming();
    let marker = include_marker.then(|| assistant_stream_lead_marker(input.styles));
    let started = Instant::now();
    let lines = coco_tui_markdown::render_markdown(source, opts, marker.as_ref());
    let elapsed = started.elapsed();
    tracing::trace!(
        target: "tui::streaming",
        region = "mutable_tail",
        source_bytes = source.len(),
        lines = lines.len(),
        elapsed_us = elapsed.as_micros(),
        width = input.width,
        "render streaming markdown region",
    );
    lines
}

pub(crate) fn streaming_cursor_line(styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::raw("▌").fg(styles.accent()))
}

#[cfg(test)]
#[path = "stream.test.rs"]
mod tests;
