//! Grammar-accurate markdown rendering for the coco TUI.
//!
//! Parses CommonMark + GFM with `pulldown-cmark` and emits owned
//! `Vec<Line<'static>>` for the native-scrollback engine. Code fences are
//! syntax-highlighted with syntect, mapped onto coco's themeable palette (see
//! [`highlight`]). Colors come exclusively from [`UiStyles`]; the lead turn
//! marker is a first-class input (see [`LeadMarker`]) rather than a string the
//! caller post-patches.
//!
//! Output contract matches the prior renderer for prose: logical lines are
//! emitted with a `body_indent`-column left margin and are wrapped downstream at
//! paint time (`Paragraph::wrap`). Code fences are the exception: their guttered
//! body rows wrap internally so the frame stays within the configured width.

use coco_tui_ui::display::SyntaxHighlighting;
use coco_tui_ui::engine::history_links::HistoryLinkHint;
use coco_tui_ui::style::UiStyles;
use pulldown_cmark::Alignment;
use pulldown_cmark::BlockQuoteKind;
use pulldown_cmark::CodeBlockKind;
use pulldown_cmark::Event;
use pulldown_cmark::HeadingLevel;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

mod highlight;
mod stable;

pub use highlight::prewarm_highlighting;
pub use stable::StablePrefixTracker;
pub use stable::stable_prefix_end;

pub type LinkSpan = HistoryLinkHint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkdownRender {
    pub lines: Vec<Line<'static>>,
    pub links: Vec<LinkSpan>,
}

/// A turn-boundary marker placed at column 0 of the first rendered line (e.g.
/// the assistant `⏺` dot). The glyph plus a trailing space occupy exactly
/// `body_indent` columns so wrapped continuation lines stay aligned.
#[derive(Debug, Clone)]
pub struct LeadMarker {
    pub glyph: &'static str,
    pub style: Style,
}

impl LeadMarker {
    pub fn new(glyph: &'static str, color: Color) -> Self {
        Self {
            glyph,
            style: Style::default().fg(color),
        }
    }
}

/// Rendering options. `body_indent` replaces the old hard-coded two-space pad.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownOptions<'a> {
    pub styles: UiStyles<'a>,
    pub width: u16,
    pub syntax: SyntaxHighlighting,
    pub body_indent: u16,
    /// True while rendering an in-flight streaming buffer. A mid-stream fence is
    /// not yet closed, so its body is a moving target — laying out a `mermaid`
    /// diagram on every delta makes the block flicker/reflow as it grows. When
    /// set, `mermaid` fences keep their verbatim form and only render as a
    /// diagram once on the finalized (non-streaming) pass.
    pub streaming: bool,
}

impl<'a> MarkdownOptions<'a> {
    /// Defaults matching the legacy renderer (two-space body indent).
    pub fn new(styles: UiStyles<'a>, width: u16, syntax: SyntaxHighlighting) -> Self {
        Self {
            styles,
            width,
            syntax,
            body_indent: 2,
            streaming: false,
        }
    }

    /// Mark this render as an in-flight streaming pass (suppresses per-delta
    /// `mermaid` diagram layout — see [`MarkdownOptions::streaming`]).
    pub fn streaming(mut self) -> Self {
        self.streaming = true;
        self
    }
}

/// Render markdown `text` to owned ratatui lines.
///
/// When `marker` is `Some`, the first emitted line carries the marker glyph at
/// column 0; when `text` is empty a single marker-only line is produced so a
/// turn boundary is still visible.
pub fn render_markdown(
    text: &str,
    opts: MarkdownOptions<'_>,
    marker: Option<&LeadMarker>,
) -> Vec<Line<'static>> {
    render_markdown_with_mode(text, opts, marker, LinkPresentation::Fallback).lines
}

pub fn render_markdown_with_links(
    text: &str,
    opts: MarkdownOptions<'_>,
    marker: Option<&LeadMarker>,
) -> MarkdownRender {
    render_markdown_with_mode(text, opts, marker, LinkPresentation::Sidecar)
}

fn render_markdown_with_mode(
    text: &str,
    opts: MarkdownOptions<'_>,
    marker: Option<&LeadMarker>,
    link_presentation: LinkPresentation,
) -> MarkdownRender {
    let mut writer = Writer::new(opts, marker, link_presentation);
    let mut parser_opts = Options::empty();
    parser_opts.insert(Options::ENABLE_STRIKETHROUGH);
    parser_opts.insert(Options::ENABLE_TABLES);
    parser_opts.insert(Options::ENABLE_TASKLISTS);
    parser_opts.insert(Options::ENABLE_GFM);
    for event in Parser::new_ext(text, parser_opts) {
        writer.event(event);
    }
    writer.finish()
}

/// Highlight raw code outside a Markdown fence.
///
/// Tool-result renderers use this for file-content previews where wrapping the
/// content in a synthetic code fence would add borders and break on embedded
/// fence markers. Returns `None` when highlighting is disabled, unsupported, or
/// too expensive; callers should render plain text in that case.
pub fn highlight_code_lines(
    code: &str,
    lang: &str,
    styles: UiStyles<'_>,
    syntax: SyntaxHighlighting,
) -> Option<std::sync::Arc<Vec<Vec<Span<'static>>>>> {
    highlight::highlight_code(
        code,
        lang,
        styles,
        syntax,
        highlight::HighlightMode::Committed,
    )
}

// ─────────────────────────────────────────────────────────────────────────
// Writer
// ─────────────────────────────────────────────────────────────────────────

/// Inline-link render state captured between `Tag::Link` and `TagEnd::Link`.
struct LinkRender {
    dest_url: String,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkPresentation {
    /// Preserve the destination as visible text for renderers that cannot
    /// consume OSC 8 geometry (reader, modal, live viewport, unsupported tty).
    Fallback,
    /// Keep visible geometry label-only and emit a separate LinkSpan.
    Sidecar,
}

#[derive(Clone)]
struct LinkedSpan {
    span: Span<'static>,
    target: Option<String>,
}

impl LinkedSpan {
    fn plain(span: Span<'static>) -> Self {
        Self { span, target: None }
    }
}

/// One table cell holds styled inline content plus its link target sidecar.
type TableCell = Vec<LinkedSpan>;

struct TableBuilder {
    aligns: Vec<Alignment>,
    header: Vec<TableCell>,
    rows: Vec<Vec<TableCell>>,
    cur_row: Vec<TableCell>,
    cur_cell: TableCell,
    in_head: bool,
}

struct Writer<'a> {
    styles: UiStyles<'a>,
    width: u16,
    syntax: SyntaxHighlighting,
    body_indent: usize,
    /// Only read by the `#[cfg(feature = "mermaid")]` branch in
    /// `finish_code_block`; the default (no-mermaid) build never reads it.
    #[cfg_attr(not(feature = "mermaid"), allow(dead_code))]
    streaming: bool,

    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    span_links: Vec<Option<String>>,
    links: Vec<LinkSpan>,
    link_presentation: LinkPresentation,

    cur_style: Style,
    style_stack: Vec<Style>,

    list_stack: Vec<Option<u64>>,
    pending_marker: Option<Span<'static>>,
    /// Per-open-item `(marker_width, first_line_emitted)`. Continuation lines
    /// (after the item's first line) hang-indent under the item text by the
    /// marker width; the first line (bullet, number, or task checkbox) does not.
    item_hang: Vec<(usize, bool)>,
    quote_gutters: Vec<Style>,

    in_code: bool,
    code_lang: String,
    code_buf: String,

    table: Option<TableBuilder>,
    /// Active inline link: destination + display text used to suppress a
    /// duplicate fallback for autolinks.
    link: Option<LinkRender>,

    lead_marker: Option<Span<'static>>,
    /// Display width of `lead_marker` ("{glyph} "), used to align the first
    /// line's padding with continuation lines independent of `body_indent`.
    lead_marker_width: usize,
    first_line_emitted: bool,
    needs_gap: bool,
    empty_marker: Option<Span<'static>>,
}

impl<'a> Writer<'a> {
    fn new(
        opts: MarkdownOptions<'a>,
        marker: Option<&LeadMarker>,
        link_presentation: LinkPresentation,
    ) -> Self {
        let lead_marker = marker.map(|m| Span::styled(format!("{} ", m.glyph), m.style));
        let lead_marker_width = lead_marker
            .as_ref()
            .map_or(0, |s| UnicodeWidthStr::width(s.content.as_ref()));
        let empty_marker = marker.map(|m| Span::styled(m.glyph.to_string(), m.style));
        Self {
            styles: opts.styles,
            width: opts.width,
            syntax: opts.syntax,
            body_indent: opts.body_indent as usize,
            streaming: opts.streaming,
            lines: Vec::new(),
            spans: Vec::new(),
            span_links: Vec::new(),
            links: Vec::new(),
            link_presentation,
            cur_style: Style::default(),
            style_stack: Vec::new(),
            list_stack: Vec::new(),
            pending_marker: None,
            item_hang: Vec::new(),
            quote_gutters: Vec::new(),
            in_code: false,
            code_lang: String::new(),
            code_buf: String::new(),
            table: None,
            link: None,
            lead_marker,
            lead_marker_width,
            first_line_emitted: false,
            needs_gap: false,
            empty_marker,
        }
    }

    fn finish(mut self) -> MarkdownRender {
        // Flush any dangling inline content.
        if !self.spans.is_empty() {
            self.flush_line();
        }
        if self.lines.is_empty()
            && let Some(marker) = self.empty_marker.take()
        {
            self.lines.push(Line::from(vec![marker]));
        }
        MarkdownRender {
            lines: self.lines,
            links: self.links,
        }
    }

    fn list_depth(&self) -> usize {
        self.list_stack.len()
    }

    fn base_indent_cols(&self) -> usize {
        self.body_indent + self.list_depth().saturating_sub(1) * 2
    }

    /// Columns the block-level margin consumes before list hang indentation or
    /// pending markers. Rules and table grids use this simpler budget; code
    /// fences use `available_raw_cols` because they can appear after list text.
    fn left_margin_cols(&self) -> usize {
        self.base_indent_cols() + self.quote_gutters.len() * 2
    }

    fn leading_cols(&self) -> usize {
        let base = self.base_indent_cols();
        let hang = match self.item_hang.last() {
            Some(&(w, true)) => w,
            _ => 0,
        };
        let indent = base + hang;
        let first_prefix = if !self.first_line_emitted && self.lead_marker.is_some() {
            indent.max(self.lead_marker_width)
        } else {
            indent
        };
        let quote_cols = self.quote_gutters.len() * 2;
        let marker_cols = self
            .pending_marker
            .as_ref()
            .map_or(0, |marker| UnicodeWidthStr::width(marker.content.as_ref()));
        first_prefix + quote_cols + marker_cols
    }

    fn available_raw_cols(&self) -> usize {
        (self.width as usize).saturating_sub(self.leading_cols())
    }

    /// Leading spans for a freshly-finished line: lead marker (first line only)
    /// or indent spaces, blockquote gutters, then a pending list marker.
    fn leading(&mut self) -> Vec<Span<'static>> {
        let mut out: Vec<Span<'static>> = Vec::new();
        let base = self.base_indent_cols();
        // Continuation lines (after an item's first line) hang-indent under the
        // item text by the marker width; the first line carries the marker.
        let hang = match self.item_hang.last() {
            Some(&(w, true)) => w,
            _ => 0,
        };
        let indent = base + hang;
        if !self.first_line_emitted {
            self.first_line_emitted = true;
            if let Some(marker) = self.lead_marker.take() {
                out.push(marker);
                // Pad to `indent` from the marker's true display width, so a
                // width-2 glyph or a non-default body_indent still aligns the
                // first line with hang-indented continuation lines.
                let extra = indent.saturating_sub(self.lead_marker_width);
                if extra > 0 {
                    out.push(Span::raw(" ".repeat(extra)));
                }
            } else if indent > 0 {
                out.push(Span::raw(" ".repeat(indent)));
            }
        } else if indent > 0 {
            out.push(Span::raw(" ".repeat(indent)));
        }
        for gutter in &self.quote_gutters {
            out.push(Span::styled("│ ".to_string(), *gutter));
        }
        if let Some(marker) = self.pending_marker.take() {
            out.push(marker);
        }
        out
    }

    /// Finish the current logical line (content in `self.spans`).
    fn flush_line(&mut self) {
        let mut line_spans = self.leading();
        let leading_len = line_spans
            .iter()
            .map(|span| span.content.len())
            .sum::<usize>();
        let line_index = self.lines.len();
        let mut byte_offset = leading_len;
        let mut active: Option<(usize, String)> = None;
        for (span, target) in self.spans.iter().zip(&self.span_links) {
            let span_end = byte_offset.saturating_add(span.content.len());
            match (active.as_mut(), target) {
                (Some((_, active_target)), Some(target)) if active_target == target => {}
                (Some((start, active_target)), target) => {
                    self.links.push(LinkSpan {
                        line: line_index,
                        start_byte: *start,
                        end_byte: byte_offset,
                        target: std::mem::take(active_target),
                    });
                    active = target.clone().map(|target| (byte_offset, target));
                }
                (None, Some(target)) => active = Some((byte_offset, target.clone())),
                (None, None) => {}
            }
            byte_offset = span_end;
        }
        if let Some((start, target)) = active {
            self.links.push(LinkSpan {
                line: line_index,
                start_byte: start,
                end_byte: byte_offset,
                target,
            });
        }
        line_spans.append(&mut self.spans);
        self.span_links.clear();
        self.lines.push(Line::from(line_spans));
        // The current item has now emitted at least one line; later lines are
        // continuations and hang-indent under the item text.
        if let Some(last) = self.item_hang.last_mut() {
            last.1 = true;
        }
    }

    /// Emit a fully-formed line (used for borders / rules that bypass inline
    /// accumulation), honoring the first-line lead marker + base indent. Any
    /// dangling inline content (e.g. a tight list item's text immediately
    /// followed by a nested block) is flushed first so it is never dropped.
    fn emit_raw_line(&mut self, content: Vec<Span<'static>>) {
        if !self.spans.is_empty() {
            self.flush_line();
        }
        self.spans = content;
        self.span_links = vec![None; self.spans.len()];
        self.flush_line();
    }

    fn emit_linked_raw_line(&mut self, content: Vec<LinkedSpan>) {
        if !self.spans.is_empty() {
            self.flush_line();
        }
        self.span_links = content.iter().map(|span| span.target.clone()).collect();
        self.spans = content.into_iter().map(|span| span.span).collect();
        self.flush_line();
    }

    fn blank_line(&mut self) {
        self.lines.push(Line::from(String::new()));
    }

    /// Begin a block: flush any pending inline line (a tight list item's text
    /// before its nested block/sub-list), then insert a separating blank line
    /// when the previous block asked for one.
    fn block_gap(&mut self) {
        if !self.spans.is_empty() {
            self.flush_line();
        }
        if self.needs_gap && !self.lines.is_empty() {
            self.blank_line();
        }
        self.needs_gap = false;
    }

    fn push_style(&mut self, style: Style) {
        self.style_stack.push(self.cur_style);
        self.cur_style = self.cur_style.patch(style);
    }

    fn pop_style(&mut self) {
        if let Some(prev) = self.style_stack.pop() {
            self.cur_style = prev;
        }
    }

    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.on_text(&text),
            Event::Code(code) => self.on_inline_code(&code),
            // Math is not enabled; render literally so nothing is dropped.
            Event::InlineMath(s) | Event::DisplayMath(s) => self.on_text(&s),
            Event::Html(html) => {
                // pulldown emits one HtmlBlock chunk per Event::Html, usually
                // newline-terminated; treat the trailing newline as a line
                // break so multi-line raw HTML keeps its line structure.
                let had_newline = html.ends_with('\n');
                self.on_text(html.trim_end_matches('\n'));
                if had_newline {
                    self.flush_line();
                }
            }
            Event::InlineHtml(html) => self.on_text(html.trim_end_matches('\n')),
            // Footnotes are intentionally not enabled (no ENABLE_FOOTNOTES), so
            // pulldown-cmark never emits this; explicit no-op for exhaustiveness,
            // mirroring the Tag/TagEnd::FootnoteDefinition no-ops.
            Event::FootnoteReference(_) => {}
            Event::SoftBreak | Event::HardBreak => {
                // Preserve authored line structure (matches the prior renderer);
                // downstream `Paragraph::wrap` still reflows over-long lines.
                if self.in_code {
                    self.code_buf.push('\n');
                } else {
                    self.flush_line();
                }
            }
            Event::Rule => self.on_rule(),
            Event::TaskListMarker(checked) => self.on_task_marker(checked),
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => self.block_gap(),
            Tag::Heading { level, .. } => {
                self.block_gap();
                // TS renders headings with emphasis only — bold, and h1 also
                // italic + underline — with NO color, so they inherit the body
                // text color instead of a brand tint.
                let mut style = Style::default().add_modifier(Modifier::BOLD);
                if matches!(level, HeadingLevel::H1) {
                    style = style
                        .add_modifier(Modifier::ITALIC)
                        .add_modifier(Modifier::UNDERLINED);
                }
                self.push_style(style);
            }
            Tag::BlockQuote(kind) => {
                self.block_gap();
                self.start_blockquote(kind);
            }
            Tag::CodeBlock(kind) => {
                self.block_gap();
                self.in_code = true;
                self.code_buf.clear();
                self.code_lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        lang.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
            }
            Tag::List(start) => {
                // An empty parent item whose first child is a nested list still
                // holds its bullet in `pending_marker`; emit that bare bullet now
                // so the nested item's marker does not overwrite (drop) it.
                if self.pending_marker.is_some() && self.spans.is_empty() {
                    self.flush_line();
                }
                self.block_gap();
                self.list_stack.push(start);
            }
            Tag::Item => {
                let marker = match self.list_stack.last_mut() {
                    Some(Some(n)) => {
                        let label = format!("{n}. ");
                        *n += 1;
                        Span::styled(label, Style::default().fg(self.styles.text()))
                    }
                    _ => Span::styled("• ".to_string(), Style::default().fg(self.styles.text())),
                };
                self.item_hang
                    .push((UnicodeWidthStr::width(marker.content.as_ref()), false));
                self.pending_marker = Some(marker);
            }
            Tag::Table(aligns) => {
                self.block_gap();
                self.table = Some(TableBuilder {
                    aligns,
                    header: Vec::new(),
                    rows: Vec::new(),
                    cur_row: Vec::new(),
                    cur_cell: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.in_head = true;
                }
            }
            Tag::TableRow => {}
            Tag::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    t.cur_cell.clear();
                }
            }
            Tag::Emphasis => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(
                Style::default()
                    .fg(self.styles.strikethrough())
                    .add_modifier(Modifier::CROSSED_OUT),
            ),
            Tag::Link { dest_url, .. } => {
                self.link = Some(LinkRender {
                    dest_url: dest_url.to_string(),
                    text: String::new(),
                });
                self.push_style(
                    Style::default()
                        .fg(self.styles.hyperlink())
                        .add_modifier(Modifier::UNDERLINED),
                );
            }
            // Suppress image markup; alt text (Text events) renders inline.
            Tag::Image { .. } => self.push_style(Style::default()),
            // Not enabled (math/deflist/super-sub/footnote/html-block/metadata);
            // arms exist because pulldown enums are exhaustive.
            Tag::Superscript | Tag::Subscript => self.push_style(Style::default()),
            Tag::HtmlBlock
            | Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line();
                self.needs_gap = true;
            }
            TagEnd::Heading(_) => {
                self.pop_style();
                self.flush_line();
                self.needs_gap = true;
            }
            TagEnd::BlockQuote(_) => {
                self.quote_gutters.pop();
                self.needs_gap = true;
            }
            TagEnd::CodeBlock => self.finish_code_block(),
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.needs_gap = true;
            }
            TagEnd::Item => {
                // Flush any tight-list item content that did not close a block.
                if !self.spans.is_empty() || self.pending_marker.is_some() {
                    self.flush_line();
                }
                self.item_hang.pop();
            }
            TagEnd::Table => self.finish_table(),
            TagEnd::TableHead => {
                if let Some(t) = self.table.as_mut() {
                    t.header = std::mem::take(&mut t.cur_row);
                    t.in_head = false;
                }
            }
            TagEnd::TableRow => {
                if let Some(t) = self.table.as_mut()
                    && !t.in_head
                {
                    let row = std::mem::take(&mut t.cur_row);
                    t.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(t) = self.table.as_mut() {
                    let cell = std::mem::take(&mut t.cur_cell);
                    t.cur_row.push(cell);
                }
            }
            TagEnd::Link => {
                self.pop_style();
                self.finish_link();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Image
            | TagEnd::Superscript
            | TagEnd::Subscript => self.pop_style(),
            TagEnd::HtmlBlock
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => {}
        }
    }

    fn on_text(&mut self, text: &str) {
        if self.in_code {
            self.code_buf.push_str(text);
            return;
        }
        if text.is_empty() {
            return;
        }
        if let Some(link) = self.link.as_mut() {
            link.text.push_str(text);
        }
        // Table cells keep their styled spans so bold/italic/code/link styling
        // survives into the grid (the old path flattened to a plain string).
        let style = self.cur_style;
        let target = self.active_link_target();
        if let Some(t) = self.table.as_mut() {
            t.cur_cell.push(LinkedSpan {
                span: Span::styled(text.to_string(), style),
                target,
            });
            return;
        }
        self.spans.push(Span::styled(text.to_string(), style));
        self.span_links.push(self.active_link_target());
    }

    /// Keep link destinations out of visible prose and record them as geometry
    /// sidecars when the logical line flushes. Visible span text never carries
    /// the destination or terminal escapes, so wrapping geometry stays exact.
    fn finish_link(&mut self) {
        let Some(link) = self.link.take() else {
            return;
        };
        if self.link_presentation == LinkPresentation::Sidecar {
            return;
        }
        let destination = link.dest_url.trim();
        if destination.is_empty() {
            return;
        }
        let display_destination = destination.strip_prefix("mailto:").unwrap_or(destination);
        let display_text = link.text.trim();
        if display_text == display_destination {
            return;
        }
        let suffix = if display_text.is_empty() {
            display_destination.to_string()
        } else {
            format!(" ({display_destination})")
        };
        let span = LinkedSpan::plain(Span::styled(
            suffix,
            Style::default().fg(self.styles.hyperlink()),
        ));
        if let Some(table) = self.table.as_mut() {
            table.cur_cell.push(span);
        } else {
            self.spans.push(span.span);
            self.span_links.push(None);
        }
    }

    fn active_link_target(&self) -> Option<String> {
        match self.link_presentation {
            LinkPresentation::Fallback => None,
            LinkPresentation::Sidecar => self.link.as_ref().map(|link| link.dest_url.clone()),
        }
    }

    fn on_inline_code(&mut self, code: &str) {
        // Inline code uses the dedicated `code_inline` token (decoupled from
        // `accent`, which also drives chips/alerts) but preserves surrounding
        // inline modifiers (bold/italic/strikethrough/link) via patch.
        let style = self
            .cur_style
            .patch(Style::default().fg(self.styles.code_inline()));
        if let Some(link) = self.link.as_mut() {
            link.text.push_str(code);
        }
        let target = self.active_link_target();
        if let Some(t) = self.table.as_mut() {
            t.cur_cell.push(LinkedSpan {
                span: Span::styled(code.to_string(), style),
                target,
            });
            return;
        }
        self.spans.push(Span::styled(code.to_string(), style));
        self.span_links.push(self.active_link_target());
    }

    fn on_task_marker(&mut self, checked: bool) {
        // The checkbox IS the list marker for a task item — drop the bullet so
        // it does not render as a redundant "• ☐".
        self.pending_marker = None;
        let (glyph, color) = if checked {
            ("☑ ", self.styles.success())
        } else {
            ("☐ ", self.styles.dim())
        };
        self.spans
            .push(Span::styled(glyph.to_string(), Style::default().fg(color)));
        self.span_links.push(None);
    }

    fn on_rule(&mut self) {
        self.block_gap();
        let dashes = (self.width as usize)
            .saturating_sub(self.left_margin_cols())
            .clamp(1, 80);
        self.emit_raw_line(vec![Span::styled(
            "─".repeat(dashes),
            Style::default().fg(self.styles.hr()),
        )]);
        self.needs_gap = true;
    }

    fn start_blockquote(&mut self, kind: Option<BlockQuoteKind>) {
        let gutter_style = match kind {
            Some(k) => {
                let (label, color) = alert_label(k, self.styles);
                // Alert header line above the quoted body.
                self.emit_raw_line(vec![Span::styled(
                    label.to_string(),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )]);
                Style::default().fg(color)
            }
            None => Style::default().fg(self.styles.blockquote()),
        };
        self.quote_gutters.push(gutter_style);
    }

    fn finish_code_block(&mut self) {
        self.in_code = false;
        let code = std::mem::take(&mut self.code_buf);
        let lang = std::mem::take(&mut self.code_lang);

        // ```mermaid fences render as box-drawing cells when the `mermaid`
        // feature is on and the diagram is a supported, legible box-and-arrow
        // graph; otherwise fall through to the verbatim code-fence path.
        #[cfg(feature = "mermaid")]
        if !self.streaming
            && lang.eq_ignore_ascii_case("mermaid")
            && let Some(diagram) =
                coco_tui_mermaid::mermaid_to_lines(&code, self.styles, self.width)
        {
            for line in diagram {
                self.emit_raw_line(line.spans);
            }
            self.needs_gap = true;
            return;
        }

        let border_style = Style::default().fg(self.styles.border());
        let header_available = self.available_raw_cols();
        let header = if lang.is_empty() {
            "┌─".to_string()
        } else {
            format!("┌─ {lang}")
        };
        self.emit_raw_line(vec![Span::styled(
            coco_tui_ui::truncate::truncate_to_width(&header, header_available),
            border_style,
        )]);

        // Optional themeable background fill behind the fence body. Folded into
        // the gutter, body text, and highlighted spans. `None` (the default) is
        // a no-op.
        let bg = self.styles.code_bg();
        let mut gutter = Style::default().fg(self.styles.border());
        let mut body_style = Style::default().fg(self.styles.text());
        if let Some(c) = bg {
            gutter = gutter.bg(c);
            body_style = body_style.bg(c);
        }

        let mode = if self.streaming {
            // The in-flight tail re-renders the growing open fence every
            // revealed line: extend the prefix checkpoint, keep per-delta
            // snapshots out of the shared LRU.
            highlight::HighlightMode::Streaming
        } else {
            highlight::HighlightMode::Committed
        };
        let highlighted = highlight::highlight_code(&code, &lang, self.styles, self.syntax, mode);
        let code_lines: Vec<&str> = code.split('\n').collect();
        // Drop a trailing empty element from the final newline.
        let line_count = if code.ends_with('\n') {
            code_lines.len().saturating_sub(1)
        } else {
            code_lines.len()
        };
        for (i, code_line) in code_lines.iter().take(line_count).enumerate() {
            let code_spans = match highlighted.as_ref().and_then(|h| h.get(i)) {
                Some(hspans) if !hspans.is_empty() => match bg {
                    Some(c) => hspans
                        .iter()
                        .map(|s| Span::styled(s.content.clone(), s.style.bg(c)))
                        .collect(),
                    None => hspans.to_vec(),
                },
                _ => vec![Span::styled((*code_line).to_string(), body_style)],
            };
            for row in wrap_code_spans(&code_spans, self.available_raw_cols(), gutter) {
                self.emit_raw_line(row);
            }
        }
        let footer_available = self.available_raw_cols();
        self.emit_raw_line(vec![Span::styled(
            coco_tui_ui::truncate::truncate_to_width("└─", footer_available),
            border_style,
        )]);
        self.needs_gap = true;
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let col_count = table
            .header
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        if col_count == 0 {
            return;
        }
        // Budget for the sum of column *content* widths: total width minus the
        // left margin, the `col_count + 1` vertical borders, and the two padding
        // spaces per column.
        let budget = (self.width as usize)
            .saturating_sub(self.left_margin_cols() + 3 * col_count + 1)
            .max(col_count);
        let widths = column_widths(&table.header, &table.rows, col_count, budget);

        let border = Style::default().fg(self.styles.table_border());
        self.emit_raw_line(vec![Span::styled(
            table_rule(&widths, '┌', '┬', '┐'),
            border,
        )]);
        if !table.header.is_empty() {
            self.emit_table_row(&table.header, &widths, &table.aligns, true);
            self.emit_raw_line(vec![Span::styled(
                table_rule(&widths, '├', '┼', '┤'),
                border,
            )]);
        }
        for row in &table.rows {
            self.emit_table_row(row, &widths, &table.aligns, false);
        }
        self.emit_raw_line(vec![Span::styled(
            table_rule(&widths, '└', '┴', '┘'),
            border,
        )]);
        self.needs_gap = true;
    }

    fn emit_table_row(
        &mut self,
        cells: &[TableCell],
        widths: &[usize],
        aligns: &[Alignment],
        header: bool,
    ) {
        let border = Style::default().fg(self.styles.table_border());
        // Wrap each cell to its column width; the row is as tall as its tallest
        // cell and shorter cells pad with blank visual lines.
        let wrapped: Vec<Vec<Vec<LinkedSpan>>> = (0..widths.len())
            .map(|i| {
                let cell: &[LinkedSpan] = cells.get(i).map(Vec::as_slice).unwrap_or(&[]);
                let mut lines = wrap_styled_cell(cell, widths[i]);
                // Headers read bold but keep the terminal foreground (TS does not
                // brand-tint header cells).
                if header {
                    for line in &mut lines {
                        for span in line {
                            span.span.style = span.span.style.add_modifier(Modifier::BOLD);
                        }
                    }
                }
                lines
            })
            .collect();

        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for row_idx in 0..height {
            let mut spans = vec![LinkedSpan::plain(Span::styled("│".to_string(), border))];
            for (i, width) in widths.iter().enumerate() {
                let line = wrapped[i].get(row_idx).cloned().unwrap_or_default();
                let align = aligns.get(i).copied().unwrap_or(Alignment::None);
                spans.push(LinkedSpan::plain(Span::raw(" ")));
                spans.extend(pad_spans(line, *width, align));
                spans.push(LinkedSpan::plain(Span::raw(" ")));
                spans.push(LinkedSpan::plain(Span::styled("│".to_string(), border)));
            }
            self.emit_linked_raw_line(spans);
        }
    }
}

fn alert_label(kind: BlockQuoteKind, styles: UiStyles<'_>) -> (&'static str, Color) {
    match kind {
        BlockQuoteKind::Note => ("▲ NOTE", styles.primary()),
        BlockQuoteKind::Tip => ("▲ TIP", styles.success()),
        BlockQuoteKind::Important => ("▲ IMPORTANT", styles.accent()),
        BlockQuoteKind::Warning => ("▲ WARNING", styles.warning()),
        BlockQuoteKind::Caution => ("▲ CAUTION", styles.error()),
    }
}

fn wrap_code_spans(
    spans: &[Span<'static>],
    row_width: usize,
    gutter_style: Style,
) -> Vec<Vec<Span<'static>>> {
    let gutter = code_gutter(row_width, gutter_style);
    let content_width = row_width.saturating_sub(gutter.width);
    let mut rows = vec![vec![gutter.span.clone()]];
    if content_width == 0 {
        return rows;
    }

    let mut current_width = 0usize;
    for span in spans {
        let mut piece = String::new();
        for ch in span.content.chars() {
            if matches!(ch, '\n' | '\r') {
                continue;
            }
            let char_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if current_width > 0 && current_width + char_width > content_width {
                push_code_piece(rows.last_mut(), &mut piece, span.style);
                rows.push(vec![gutter.span.clone()]);
                current_width = 0;
            }
            if current_width == 0 && char_width > content_width {
                push_code_piece(rows.last_mut(), &mut piece, span.style);
                if let Some(row) = rows.last_mut() {
                    row.push(Span::styled(
                        coco_tui_ui::truncate::truncate_to_width(&ch.to_string(), content_width),
                        span.style,
                    ));
                }
                rows.push(vec![gutter.span.clone()]);
                current_width = 0;
                continue;
            }
            piece.push(ch);
            current_width += char_width;
        }
        push_code_piece(rows.last_mut(), &mut piece, span.style);
    }
    if rows.last().is_some_and(|row| row.len() == 1) && rows.len() > 1 {
        rows.pop();
    }
    rows
}

#[derive(Clone)]
struct CodeGutter {
    span: Span<'static>,
    width: usize,
}

fn code_gutter(row_width: usize, gutter_style: Style) -> CodeGutter {
    let content = match row_width {
        0 => String::new(),
        1 => "│".to_string(),
        _ => "│ ".to_string(),
    };
    let width = UnicodeWidthStr::width(content.as_str());
    CodeGutter {
        span: Span::styled(content, gutter_style),
        width,
    }
}

fn push_code_piece(row: Option<&mut Vec<Span<'static>>>, piece: &mut String, style: Style) {
    if piece.is_empty() {
        return;
    }
    if let Some(row) = row {
        row.push(Span::styled(std::mem::take(piece), style));
    }
}

fn table_rule(widths: &[usize], left: char, mid: char, right: char) -> String {
    let mut s = String::new();
    s.push(left);
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            s.push(mid);
        }
        s.push_str(&"─".repeat(w + 2));
    }
    s.push(right);
    s
}

/// Total display width of a cell's spans.
fn cell_width(cell: &[LinkedSpan]) -> usize {
    cell.iter()
        .map(|s| UnicodeWidthStr::width(s.span.content.as_ref()))
        .sum()
}

/// Width of the widest glyph in a cell. A column may hard-wrap a long word, but
/// it must never be narrower than a glyph or the glyph would be dropped.
fn widest_glyph_width(cell: &[LinkedSpan]) -> usize {
    cell.iter()
        .flat_map(|span| span.span.content.chars())
        .filter_map(UnicodeWidthChar::width)
        .max()
        .unwrap_or(0)
}

/// Distribute `budget` columns across `col_count` columns: prefer natural
/// (unwrapped) widths, and when they overflow, give every column its
/// widest-glyph floor first, then share the remainder proportionally to how
/// much more each column "wants" (natural − floor). No column is capped at a
/// fixed maximum, so wide cells wrap rather than silently truncate.
fn column_widths(
    header: &[TableCell],
    rows: &[Vec<TableCell>],
    col_count: usize,
    budget: usize,
) -> Vec<usize> {
    let mut natural = vec![0usize; col_count];
    let mut floor = vec![1usize; col_count];
    let mut consider = |cells: &[TableCell]| {
        for (i, cell) in cells.iter().enumerate().take(col_count) {
            natural[i] = natural[i].max(cell_width(cell));
            floor[i] = floor[i].max(widest_glyph_width(cell));
        }
    };
    consider(header);
    for row in rows {
        consider(row);
    }
    // A floor can never exceed the natural width, and every column is ≥ 1.
    for i in 0..col_count {
        natural[i] = natural[i].max(1);
        floor[i] = floor[i].clamp(1, natural[i]);
    }

    let total_natural: usize = natural.iter().sum();
    if total_natural <= budget {
        return natural;
    }

    let total_floor: usize = floor.iter().sum();
    if total_floor >= budget {
        // A grid cannot represent a glyph in fewer columns than the glyph
        // itself occupies. Preserve the per-column floors even when a very
        // narrow viewport cannot contain the complete rule; the terminal may
        // clip the far edge, but the renderer must never delete cell content.
        return floor;
    }

    let want: Vec<usize> = (0..col_count).map(|i| natural[i] - floor[i]).collect();
    let total_want: usize = want.iter().sum();
    let mut widths = floor.clone();
    if total_want > 0 {
        let remaining = budget - total_floor;
        for i in 0..col_count {
            let add = remaining * want[i] / total_want;
            widths[i] = (floor[i] + add).min(natural[i]);
        }
        // Hand any rounding leftover to columns that still want more, widest
        // want first, so the grid uses its full budget.
        let mut order: Vec<usize> = (0..col_count).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(want[i]));
        let mut leftover = budget.saturating_sub(widths.iter().sum());
        for &i in &order {
            if leftover == 0 {
                break;
            }
            let room = natural[i] - widths[i];
            let give = room.min(leftover);
            widths[i] += give;
            leftover -= give;
        }
    }
    widths
}

/// Wrap a styled cell to `width` columns at word boundaries, returning one span
/// vector per visual line. Word styling (bold/italic/code/link) is preserved;
/// words wider than the column are hard-broken by character so nothing is lost.
fn wrap_styled_cell(cell: &[LinkedSpan], width: usize) -> Vec<Vec<LinkedSpan>> {
    let width = width.max(1);
    let chars: Vec<(char, Style, Option<String>)> = cell
        .iter()
        .flat_map(|span| {
            let style = span.span.style;
            span.span
                .content
                .chars()
                .map(move |ch| (ch, style, span.target.clone()))
        })
        .collect();
    if chars.is_empty() {
        return vec![Vec::new()];
    }

    let mut rows: Vec<Vec<(char, Style, Option<String>)>> = Vec::new();
    let mut cur: Vec<(char, Style, Option<String>)> = Vec::new();
    let mut cur_w = 0usize;
    let char_w = |ch: char| UnicodeWidthChar::width(ch).unwrap_or(0);
    let trim_trailing = |row: &mut Vec<(char, Style, Option<String>)>| {
        while row.last().is_some_and(|(c, _, _)| *c == ' ') {
            row.pop();
        }
    };

    let mut i = 0;
    while i < chars.len() {
        if chars[i].0 == ' ' {
            let start = i;
            while i < chars.len() && chars[i].0 == ' ' {
                i += 1;
            }
            if cur_w == 0 {
                continue; // drop leading spaces on a wrapped row
            }
            let run = &chars[start..i];
            if cur_w + run.len() <= width {
                cur.extend_from_slice(run);
                cur_w += run.len();
            } else {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            continue;
        }

        let start = i;
        while i < chars.len() && chars[i].0 != ' ' {
            i += 1;
        }
        let word = &chars[start..i];
        let word_w: usize = word.iter().map(|(c, _, _)| char_w(*c)).sum();

        if cur_w + word_w <= width {
            cur.extend_from_slice(word);
            cur_w += word_w;
        } else if word_w <= width {
            trim_trailing(&mut cur);
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
            }
            cur_w = word_w;
            cur.extend_from_slice(word);
        } else {
            trim_trailing(&mut cur);
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            for (c, st, target) in word {
                let cw = char_w(*c);
                if cur_w > 0 && cur_w + cw > width {
                    rows.push(std::mem::take(&mut cur));
                    cur_w = 0;
                }
                // This only occurs for a direct width-1 call: table column
                // allocation keeps a glyph-width floor. Retain the glyph on
                // its own visual row instead of silently deleting it.
                if cur_w == 0 && cw > width {
                    cur.push((*c, *st, target.clone()));
                    cur_w = cw;
                    continue;
                }
                cur.push((*c, *st, target.clone()));
                cur_w += cw;
            }
        }
    }
    trim_trailing(&mut cur);
    rows.push(cur);

    rows.iter().map(|row| group_chars(row)).collect()
}

/// Coalesce a run of `(char, style)` into the minimal set of styled spans.
fn group_chars(row: &[(char, Style, Option<String>)]) -> Vec<LinkedSpan> {
    let mut spans: Vec<LinkedSpan> = Vec::new();
    let mut buf = String::new();
    let mut current: Option<(Style, Option<String>)> = None;
    for (ch, style, target) in row {
        if current.as_ref() == Some(&(*style, target.clone())) {
            buf.push(*ch);
        } else {
            if let Some((style, target)) = current.take() {
                spans.push(LinkedSpan {
                    span: Span::styled(std::mem::take(&mut buf), style),
                    target,
                });
            }
            buf.push(*ch);
            current = Some((*style, target.clone()));
        }
    }
    if let Some((style, target)) = current
        && !buf.is_empty()
    {
        spans.push(LinkedSpan {
            span: Span::styled(buf, style),
            target,
        });
    }
    spans
}

/// Pad an already-wrapped cell line to exactly `width` columns per `align`.
fn pad_spans(line: Vec<LinkedSpan>, width: usize, align: Alignment) -> Vec<LinkedSpan> {
    let content: usize = line
        .iter()
        .map(|s| UnicodeWidthStr::width(s.span.content.as_ref()))
        .sum();
    let pad = width.saturating_sub(content);
    let (left, right) = match align {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    let mut out = Vec::with_capacity(line.len() + 2);
    if left > 0 {
        out.push(LinkedSpan::plain(Span::raw(" ".repeat(left))));
    }
    out.extend(line);
    if right > 0 {
        out.push(LinkedSpan::plain(Span::raw(" ".repeat(right))));
    }
    out
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
