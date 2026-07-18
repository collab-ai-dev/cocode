//! Shared suggestion popup widget for autocomplete + the slash command
//! palette.
//!
//! Renders an inline, borderless list in the fixed slot directly below
//! the input area (the viewport's bottom slot, in place of the status bar).
//! Each row is a single text line: a `▸` selection marker, then — only
//! when at least one row in the list carries a kind icon — a kind-icon
//! column (`*` agents, `+` files / paths, `◇` MCP resources, `#` symbols,
//! `↻` sessions), then a fixed-width name column and a single-line
//! description. A palette whose rows have no icons (the slash-command /
//! skill list) drops the icon column entirely, so a `/cmd` label lines up
//! under the `/` the user typed in the composer. Selected rows use the
//! theme's primary color (bold); unselected rows are rendered with
//! `text_dim`. Agent rows pick up the agent's configured color
//! (Red/Blue/Green/…) when present.

use coco_types::AgentColorName;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::i18n::t;
use crate::presentation::layout::truncate_to_width;
use coco_tui_ui::style::UiStyles;

/// A suggestion item for the popup.
#[derive(Debug, Clone)]
pub struct SuggestionItem {
    /// Char positions in `label` that the fuzzy match hit, for per-character
    /// highlighting. Empty when the producer has no match information (a
    /// context-free listing, or a source whose matcher does not report them).
    ///
    /// Char — not byte — positions, so a matcher's own indices over a
    /// non-ASCII label need no conversion, and the renderer's truncation
    /// filter is a plain count comparison. `i32` to match
    /// `coco_file_search::FileSuggestion::match_indices`, which feeds it
    /// directly; the renderer ignores out-of-range values rather than trusting
    /// matchers it does not own.
    pub highlight_indices: Vec<i32>,
    /// Display text — for slash commands this already includes the
    /// leading `/`; for agents in the unified popup it already includes
    /// the `" (agent)"` suffix (see `super::unified::seed_agent_items`).
    /// The widget renders it verbatim.
    pub label: String,
    /// Optional single-line description; whitespace runs are collapsed
    /// to a single space before truncation.
    pub description: Option<String>,
    /// Optional kind-specific metadata. `None` for slash commands and
    /// context-free items.
    pub metadata: Option<SuggestionMeta>,
}

/// Per-kind metadata carried alongside a suggestion. The renderer uses
/// the discriminant to pick an icon prefix and color, and the insertion
/// path uses it to format the splice (directory `/` suffix, agent name
/// stripping, MCP `server:uri` form).
#[derive(Debug, Clone)]
pub enum SuggestionMeta {
    /// Path completion (file or directory). `is_directory` lets the
    /// insertion path append `/` and keep the popup open for drilling.
    Path { is_directory: bool },
    /// Agent suggestion. Carries the optional badge color so the
    /// renderer can tint the row.
    Agent { color: Option<AgentColorName> },
    /// MCP resource — `server` carries the binding name, `uri` the
    /// resource path. Insertion produces `@<server>:<uri>`.
    McpResource { server: String, uri: String },
    /// Workspace symbol row from a typed symbol source.
    Symbol,
    /// Saved session row for `/resume`.
    Session,
}

impl SuggestionMeta {
    /// Single-character icon glyph rendered before the label:
    ///
    /// - agents      → `*`
    /// - mcp         → `◇`
    /// - file / path → `+`
    ///
    /// Returns a space when the metadata doesn't request an icon
    /// (slash commands; symbol search results that intentionally
    /// render undecorated).
    pub fn icon(&self) -> char {
        match self {
            Self::Agent { .. } => '*',
            Self::McpResource { .. } => '◇',
            Self::Path { .. } => '+',
            Self::Symbol => '#',
            Self::Session => '↻',
        }
    }
}

/// Suggestion popup widget.
///
/// Callers pass the fixed slot reserved by their layout. The widget clears the
/// whole slot and renders up to `max_visible` rows inside it so changing result
/// counts do not move the input composer.
pub struct SuggestionPopup<'a> {
    items: &'a [SuggestionItem],
    selected: usize,
    styles: UiStyles<'a>,
    max_visible: usize,
}

impl<'a> SuggestionPopup<'a> {
    /// Default cap on visible rows. Callers that drive their own row
    /// reservation (e.g. the TUI's vertical layout) should override via
    /// `max_visible` so the widget can't overflow the slot. Matches codex's
    /// `MAX_POPUP_ROWS` so the popup's vertical footprint is consistent.
    pub const DEFAULT_MAX_VISIBLE: u16 = 8;

    pub fn new(items: &'a [SuggestionItem], styles: UiStyles<'a>) -> Self {
        Self {
            items,
            selected: 0,
            styles,
            max_visible: Self::DEFAULT_MAX_VISIBLE as usize,
        }
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }
}

/// Width of the leading marker when the kind-icon column is shown:
/// `▸ X ` (selection marker + space + icon + space). Used when at least
/// one row carries an icon, so every label aligns past the icon column.
const MARKER_WIDTH_WITH_ICON: usize = 4;
/// Width of the leading marker when NO row carries a kind icon (the pure
/// slash-command / skill palette): `▸ ` (selection marker + space). The
/// always-blank icon column is dropped so a `/cmd` label lines up under
/// the `/` the user typed after the composer's 2-col `❯ ` indicator.
const MARKER_WIDTH_NO_ICON: usize = 2;
/// Trailing padding between the name column and the description so the
/// description never abuts the longest name in the list.
const NAME_COLUMN_PADDING: usize = 2;
/// Cap on the name column as a percentage of the popup's total width.
const NAME_COLUMN_CAP_PCT: usize = 40;
/// Floor on the name column when items are extremely short so the
/// description still has a stable starting column.
const NAME_COLUMN_FLOOR: usize = 10;

impl Widget for SuggestionPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let popup_width = area.width;
        if popup_width == 0 || area.height == 0 {
            return;
        }
        // An empty list mid-session (overshot filter, async search pending)
        // keeps the slot and shows a dim placeholder — collapsing the slot
        // would move the bottom-aligned composer (see `popup_row_budget`).
        if self.items.is_empty() {
            Clear.render(area, buf);
            let text = truncate_to_width(
                &format!("  {}", t!("suggestion_popup.no_matches")),
                popup_width as usize,
            );
            let line = Line::from(Span::styled(text, self.styles.dim_style()));
            Paragraph::new(line).render(area, buf);
            return;
        }

        let slot = self.max_visible.min(area.height as usize);
        if slot == 0 {
            return;
        }
        let total_items = self.items.len();
        // When more items match than fit the slot, sacrifice the bottom row for
        // an overflow indicator (position + scroll affordance) so the user can
        // tell the list is scrollable rather than silently truncated.
        let overflow = total_items > slot;
        let reserve_hint = overflow && slot >= 2;
        let visible_count = if reserve_hint {
            slot - 1
        } else {
            slot.min(total_items)
        };
        if visible_count == 0 {
            return;
        }
        let popup_area = area;

        Clear.render(popup_area, buf);

        // Fixed name column = longest label + padding, capped at 40% of
        // popup width and floored so very-short items still leave room
        // for the description.
        let max_label_width = self
            .items
            .iter()
            .map(|item| UnicodeWidthStr::width(item.label.as_str()))
            .max()
            .unwrap_or(0);
        let column_cap =
            ((popup_width as usize) * NAME_COLUMN_CAP_PCT / 100).max(NAME_COLUMN_FLOOR);
        let name_col_width = (max_label_width + NAME_COLUMN_PADDING)
            .min(column_cap)
            .max(NAME_COLUMN_FLOOR.min(column_cap));

        // Reserve the kind-icon column only when some row actually carries an
        // icon. A pure slash-command / skill palette (all metadata `None`)
        // drops it so labels line up under the composer's typed `/`.
        let show_icon_column = self.items.iter().any(|item| item.metadata.is_some());

        // Center the selected row in the visible window so the user
        // sees context above and below as they navigate.
        let total = total_items;
        let half = visible_count / 2;
        let max_start = total.saturating_sub(visible_count);
        let start = self.selected.saturating_sub(half).min(max_start);
        let end = (start + visible_count).min(total);

        let mut lines: Vec<Line> = Vec::with_capacity(visible_count + usize::from(reserve_hint));
        for (i, item) in self.items[start..end].iter().enumerate() {
            let actual_idx = start + i;
            let is_selected = actual_idx == self.selected;
            lines.push(build_row(
                item,
                is_selected,
                name_col_width,
                popup_width as usize,
                show_icon_column,
                self.styles,
            ));
        }
        if reserve_hint {
            lines.push(build_overflow_hint(
                self.selected,
                total,
                popup_width as usize,
                self.styles,
            ));
        }

        Paragraph::new(lines).render(popup_area, buf);
    }
}

fn build_row(
    item: &SuggestionItem,
    is_selected: bool,
    name_col_width: usize,
    popup_width: usize,
    show_icon_column: bool,
    styles: UiStyles<'_>,
) -> Line<'static> {
    let marker_width = if show_icon_column {
        MARKER_WIDTH_WITH_ICON
    } else {
        MARKER_WIDTH_NO_ICON
    };
    let selection_marker = if is_selected { '▸' } else { ' ' };
    let marker_text = if show_icon_column {
        let kind_icon = item
            .metadata
            .as_ref()
            .map(SuggestionMeta::icon)
            .unwrap_or(' ');
        format!("{selection_marker} {kind_icon} ")
    } else {
        format!("{selection_marker} ")
    };

    let label_color = match item.metadata.as_ref() {
        Some(SuggestionMeta::Agent { color: Some(c) }) => Some(agent_color_to_ratatui(*c)),
        _ => None,
    };
    let label_style = match (is_selected, label_color) {
        (true, Some(c)) => Style::default().fg(c).bold(),
        (true, None) => Style::default().fg(styles.primary()).bold(),
        (false, Some(c)) => Style::default().fg(c),
        (false, None) => styles.dim_style(),
    };
    let marker_style = if is_selected {
        Style::default().fg(styles.primary()).bold()
    } else {
        styles.dim_style()
    };
    let desc_style = if is_selected {
        Style::default().fg(styles.text())
    } else {
        styles.dim_style()
    };

    let label_target = name_col_width.saturating_sub(NAME_COLUMN_PADDING);
    let label_text = if UnicodeWidthStr::width(item.label.as_str()) > label_target {
        truncate_to_width(&item.label, label_target)
    } else {
        item.label.clone()
    };
    let label_used = UnicodeWidthStr::width(label_text.as_str());
    let pad = " ".repeat(name_col_width.saturating_sub(label_used));

    let mut spans: Vec<Span<'static>> = vec![Span::styled(marker_text, marker_style)];
    spans.extend(highlighted_label_spans(
        &label_text,
        &item.highlight_indices,
        label_style,
        highlight_style(is_selected, styles),
    ));
    spans.push(Span::raw(pad));

    if let Some(desc) = item.description.as_ref() {
        let remaining = popup_width.saturating_sub(marker_width + name_col_width);
        if remaining > 0 {
            let normalized = normalize_whitespace(desc);
            let truncated = truncate_to_width(&normalized, remaining);
            spans.push(Span::styled(truncated, desc_style));
        }
    }

    Line::from(spans)
}

/// Style for the characters a fuzzy match hit.
///
/// Bold + primary on both selected and unselected rows: the highlight's job is
/// to show *why* a row matched, which is most useful on the rows the user has
/// not landed on yet. On the selected row the label is already primary+bold, so
/// the run is separated by dropping the surrounding text to plain instead.
fn highlight_style(is_selected: bool, styles: UiStyles<'_>) -> Style {
    if is_selected {
        Style::default().fg(styles.primary()).bold().underlined()
    } else {
        Style::default().fg(styles.primary()).bold()
    }
}

/// Split `label` into alternating matched / unmatched spans.
///
/// `indices` are char positions **into the untruncated label**, so they are
/// filtered against the truncated text rather than trusted: truncate first,
/// then drop the indices that fell off the end. Doing it the other way round
/// (shifting indices onto a shorter string) is how off-by-one highlight drift
/// gets in. Out-of-range and duplicate indices are ignored rather than
/// panicking — they come from matchers this widget does not control.
fn highlighted_label_spans(
    label: &str,
    indices: &[i32],
    base: Style,
    highlight: Style,
) -> Vec<Span<'static>> {
    if indices.is_empty() {
        return vec![Span::styled(label.to_string(), base)];
    }
    let matched: std::collections::HashSet<usize> = indices
        .iter()
        .filter_map(|index| usize::try_from(*index).ok())
        .collect();

    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    for (position, ch) in label.chars().enumerate() {
        let is_match = matched.contains(&position);
        if !run.is_empty() && is_match != run_matched {
            spans.push(Span::styled(
                std::mem::take(&mut run),
                if run_matched { highlight } else { base },
            ));
        }
        run_matched = is_match;
        run.push(ch);
    }
    if !run.is_empty() {
        spans.push(Span::styled(
            run,
            if run_matched { highlight } else { base },
        ));
    }
    spans
}

/// Dim trailing row shown when the result list is taller than the popup slot.
/// Surfaces the selected position (`pos/total`) and a scroll affordance so the
/// user knows the list continues beyond the visible window instead of being
/// silently truncated.
fn build_overflow_hint(
    selected: usize,
    total: usize,
    popup_width: usize,
    styles: UiStyles<'_>,
) -> Line<'static> {
    let text = format!("  {}/{}  ↑↓ more", selected + 1, total);
    let truncated = truncate_to_width(&text, popup_width);
    Line::from(Span::styled(truncated, styles.dim_style()))
}

/// Map `AgentColorName` onto ratatui terminal colors. Indexed colors keep
/// the agent badge readable across both light and dark themes; RGB is
/// deliberately avoided so the user's terminal palette decides the shade.
pub(crate) fn agent_color_to_ratatui(color: AgentColorName) -> Color {
    match color {
        AgentColorName::Red => Color::Red,
        AgentColorName::Blue => Color::Blue,
        AgentColorName::Green => Color::Green,
        AgentColorName::Yellow => Color::Yellow,
        AgentColorName::Purple => Color::Magenta,
        // ratatui has no `Orange` / `Pink` ANSI named variants. Fall
        // back to the closest perceptual ANSI 16-color match
        // (LightRed and Magenta) so theme remapping behaves the same
        // way as the other agent badges. `Color::Indexed` is blocked
        // by the project's `disallowed_methods` lint (terminals with
        // custom palettes render the indices unpredictably).
        AgentColorName::Orange => Color::LightRed,
        AgentColorName::Pink => Color::Magenta,
        AgentColorName::Cyan => Color::Cyan,
    }
}

/// Collapse runs of whitespace in a description down to a single space
/// so multi-line help text renders on one inline row.
fn normalize_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

#[cfg(test)]
#[path = "suggestion_popup.test.rs"]
mod tests;
