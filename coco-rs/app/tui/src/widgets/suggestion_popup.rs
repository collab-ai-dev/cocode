//! Shared suggestion popup widget for autocomplete + the slash command
//! palette.
//!
//! Renders an inline, borderless list directly above the input area.
//! Each row is a single text line: a leading kind icon (`▸` selected
//! marker, `*` for agents, `+` for files / paths, `◇` for MCP resources,
//! blank for slash commands), then a fixed-width name column, then a
//! single-line description. Selected rows use the theme's primary color
//! (bold); unselected rows are rendered with `text_dim`. Agent rows pick
//! up the agent's configured color (Red/Blue/Green/…) when present.

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

use crate::presentation::layout::truncate_to_width;
use coco_tui_ui::style::UiStyles;

/// A suggestion item for the popup.
#[derive(Debug, Clone)]
pub struct SuggestionItem {
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

/// Width of the leading marker. Layout: `▸ X ` when selected with kind
/// icon, `  X ` when unselected — three columns for the marker + icon +
/// trailing space.
const MARKER_WIDTH: usize = 4;
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
        if self.items.is_empty() {
            return;
        }
        let popup_width = area.width;
        if popup_width == 0 || area.height == 0 {
            return;
        }

        let visible_count = self
            .items
            .len()
            .min(self.max_visible)
            .min(area.height as usize);
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

        // Center the selected row in the visible window so the user
        // sees context above and below as they navigate.
        let total = self.items.len();
        let half = visible_count / 2;
        let max_start = total.saturating_sub(visible_count);
        let start = self.selected.saturating_sub(half).min(max_start);
        let end = (start + visible_count).min(self.items.len());

        let mut lines: Vec<Line> = Vec::with_capacity(end - start);
        for (i, item) in self.items[start..end].iter().enumerate() {
            let actual_idx = start + i;
            let is_selected = actual_idx == self.selected;
            lines.push(build_row(
                item,
                is_selected,
                name_col_width,
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
    styles: UiStyles<'_>,
) -> Line<'static> {
    let selection_marker = if is_selected { '▸' } else { ' ' };
    let kind_icon = item
        .metadata
        .as_ref()
        .map(SuggestionMeta::icon)
        .unwrap_or(' ');
    let marker_text = format!("{selection_marker} {kind_icon} ");

    let label_color = match item.metadata.as_ref() {
        Some(SuggestionMeta::Agent { color: Some(c) }) => Some(agent_color_to_ratatui(*c)),
        _ => None,
    };
    let label_style = match (is_selected, label_color) {
        (true, Some(c)) => Style::default().fg(c).bold(),
        (true, None) => Style::default().fg(styles.primary()).bold(),
        (false, Some(c)) => Style::default().fg(c),
        (false, None) => Style::default().fg(styles.dim()),
    };
    let marker_style = if is_selected {
        Style::default().fg(styles.primary()).bold()
    } else {
        Style::default().fg(styles.dim())
    };
    let desc_style = if is_selected {
        Style::default().fg(styles.text())
    } else {
        Style::default().fg(styles.dim())
    };

    let label_target = name_col_width.saturating_sub(NAME_COLUMN_PADDING);
    let label_text = if UnicodeWidthStr::width(item.label.as_str()) > label_target {
        truncate_to_width(&item.label, label_target)
    } else {
        item.label.clone()
    };
    let label_used = UnicodeWidthStr::width(label_text.as_str());
    let pad = " ".repeat(name_col_width.saturating_sub(label_used));

    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(marker_text, marker_style),
        Span::styled(label_text, label_style),
        Span::raw(pad),
    ];

    if let Some(desc) = item.description.as_ref() {
        let remaining = popup_width.saturating_sub(MARKER_WIDTH + name_col_width);
        if remaining > 0 {
            let normalized = normalize_whitespace(desc);
            let truncated = truncate_to_width(&normalized, remaining);
            spans.push(Span::styled(truncated, desc_style));
        }
    }

    Line::from(spans)
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
