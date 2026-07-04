//! Model picker presentation.
//!
//! Rendered through the styled-`Line` modal path (see
//! `surface/modal.rs::render_modal_surface`): a provider-grouped, filterable
//! model list with a role axis (Tab) and a thinking-effort axis (Left/Right).
//! The active role tab, the focused model row, and the current-for-role model
//! each get a distinct visual cue; chrome (filter prompt / key hints) is dim.

use coco_types::ModelRole;
use ratatui::prelude::*;

use super::layout;
use super::picker;
use super::picker::PickerListView;
use super::picker::PickerRow;
use super::picker::SpanBgOpt;
use crate::i18n::t;
use crate::state::ModelEntry;
use crate::state::ModelPickerState;
use crate::state::ProviderUnavailableReason;
use coco_tui_ui::style::UiStyles;

/// Canonical role order — must mirror `update::show::next_role` so the
/// pill order matches Tab/Shift+Tab cycling.
const ROLE_ORDER: [ModelRole; 8] = [
    ModelRole::Main,
    ModelRole::Fast,
    ModelRole::Plan,
    ModelRole::Explore,
    ModelRole::Review,
    ModelRole::HookAgent,
    ModelRole::Memory,
    ModelRole::Subagent,
];

/// Non-list lines around the scrolling model list: role tabs, blank, filter,
/// blank, (list), blank, effort, blank, hints = 8. Used with the 2 border rows
/// to size the list window against the available modal height.
pub(crate) const CHROME_ROWS: usize = 8;

/// The popup title. Deliberately role-independent so the top border never
/// changes width as the active role cycles — the active role is shown by the
/// highlighted Role tab inside the box instead.
pub(crate) fn model_picker_title() -> String {
    t!("dialog.model_picker_title").to_string()
}

/// Styled body lines for the model picker, padded to `inner_width` columns and
/// windowing the model list to `list_visible` rows. The caller wraps these in
/// the bordered box + title.
pub(crate) fn model_picker_lines(
    m: &ModelPickerState,
    styles: UiStyles<'_>,
    inner_width: usize,
    available_list_rows: usize,
) -> Vec<Line<'static>> {
    // Size the list window to the UNFILTERED model count so the box height is
    // stable as the filter narrows results (no shrinking while typing), capped
    // by the caller's adaptive budget (`available_list_rows`) — a large catalog
    // scrolls inside the window rather than growing the box to fill the screen.
    let list_visible = unfiltered_row_count(m)
        .min(available_list_rows.max(1))
        .max(1);
    let view = build_view_model(m, list_visible);

    let mut lines = vec![
        picker::pad_line(render_role_tabs(m.role, styles), inner_width, None),
        picker::blank_line(inner_width),
        picker::pad_line(render_filter_line(m, styles), inner_width, None),
        picker::blank_line(inner_width),
    ];

    // Build the list region as EXACTLY `list_visible` rows (padding with blanks
    // when the filtered list is short) so the box height stays constant as the
    // filter narrows results — the box must not shrink while the user types.
    let list_start = lines.len();
    if view.list.rows.is_empty() {
        lines.push(picker::pad_line(
            Line::from(Span::raw(t!("dialog.model_picker_empty").to_string()).fg(styles.dim())),
            inner_width,
            None,
        ));
    } else {
        for row in view
            .list
            .rows
            .iter()
            .take(view.list.visible.end)
            .skip(view.list.visible.start)
        {
            lines.push(render_model_row(row, view.selected, styles, inner_width));
        }
    }
    while lines.len() - list_start < list_visible {
        lines.push(picker::blank_line(inner_width));
    }
    lines.truncate(list_start + list_visible);

    lines.push(picker::blank_line(inner_width));
    lines.push(picker::pad_line(
        render_effort_line(m, &view, styles),
        inner_width,
        None,
    ));
    lines.push(picker::blank_line(inner_width));
    let hints = picker::collapse_hints(t!("dialog.model_picker_hints").as_ref(), inner_width);
    lines.push(picker::pad_line(
        Line::from(Span::raw(hints).fg(styles.dim())),
        inner_width,
        None,
    ));
    lines
}

/// Grouped-row count for the UNFILTERED entry list — headers + entries +
/// inter-group blanks, matching `picker::grouped_list`'s layout. Sizing the
/// list window from this (not the filtered count) keeps the box height stable
/// as the filter narrows results.
fn unfiltered_row_count(m: &ModelPickerState) -> usize {
    let mut providers = 0usize;
    let mut last: Option<&str> = None;
    for e in &m.entries {
        if last != Some(e.provider_display.as_str()) {
            providers += 1;
            last = Some(e.provider_display.as_str());
        }
    }
    m.entries.len() + providers + providers.saturating_sub(1)
}

/// `⌕ ` search-icon prefix width, in display columns.
const FILTER_PREFIX_COLS: u16 = 2;
/// Content-relative row of the filter line (role tabs, blank, then filter).
pub(crate) const FILTER_ROW: u16 = 2;

/// Content-relative `(col, row)` of the filter caret, so the caller can pin the
/// terminal cursor there (IME anchoring — see `frame_layout::modal_text_cursor`).
pub(crate) fn filter_caret(m: &ModelPickerState) -> (u16, u16) {
    (
        FILTER_PREFIX_COLS + layout::text_width(&m.filter) as u16,
        FILTER_ROW,
    )
}

struct ModelPickerViewModel<'a> {
    filtered: Vec<&'a ModelEntry>,
    list: PickerListView<'a, ModelEntry>,
    selected: Option<usize>,
}

fn build_view_model(m: &ModelPickerState, list_height: usize) -> ModelPickerViewModel<'_> {
    let filtered = filtered_entries(m);
    let selected = layout::selected_in_bounds(m.selected, filtered.len());
    let list = picker::grouped_list(&filtered, selected, list_height, |entry| {
        entry.provider_display.as_str()
    });
    ModelPickerViewModel {
        filtered,
        list,
        selected,
    }
}

fn filtered_entries(m: &ModelPickerState) -> Vec<&ModelEntry> {
    let filter_lower = m.filter.to_lowercase();
    m.entries
        .iter()
        .filter(|e| {
            filter_lower.is_empty()
                || e.display_name.to_lowercase().contains(&filter_lower)
                || e.provider_display.to_lowercase().contains(&filter_lower)
        })
        .collect()
}

/// `Role:  ▸Main◂  Fast  Plan  …` — the active role gets a reverse-video pill.
fn render_role_tabs(active: ModelRole, styles: UiStyles<'_>) -> Line<'static> {
    let mut spans = vec![
        Span::raw(t!("dialog.model_picker_role_label").to_string()).fg(styles.dim()),
        Span::raw("  "),
    ];
    for (idx, role) in ROLE_ORDER.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw("  "));
        }
        let label = role_display(*role);
        if *role == active {
            spans.push(
                Span::raw(format!("▸{label}◂"))
                    .fg(styles.selection_fg())
                    .bg(styles.selection_bg())
                    .bold(),
            );
        } else {
            spans.push(Span::raw(format!(" {label} ")).fg(styles.text()));
        }
    }
    Line::from(spans)
}

/// `⌕ <query>` — a search affordance so it's obvious you can type to filter
/// straight away (no need to select a row first). The terminal cursor is pinned
/// to the caret by the modal renderer, so it reads as a live text field.
fn render_filter_line(m: &ModelPickerState, styles: UiStyles<'_>) -> Line<'static> {
    let icon = Span::raw("⌕ ").fg(styles.dim());
    if m.filter.is_empty() {
        Line::from(vec![
            icon,
            Span::raw(t!("dialog.model_picker_type_filter").to_string()).fg(styles.dim()),
        ])
    } else {
        Line::from(vec![icon, Span::raw(m.filter.clone()).fg(styles.text())])
    }
}

fn render_model_row(
    row: &PickerRow<'_, ModelEntry>,
    selected: Option<usize>,
    styles: UiStyles<'_>,
    width: usize,
) -> Line<'static> {
    match row {
        PickerRow::Blank => picker::blank_line(width),
        PickerRow::Header(provider) => picker::pad_line(
            Line::from(
                Span::raw((*provider).to_string())
                    .fg(styles.primary())
                    .bold(),
            ),
            width,
            None,
        ),
        PickerRow::Entry {
            filtered_index,
            item: entry,
        } => {
            let is_selected = Some(*filtered_index) == selected;
            // Two distinct cues that never conflate: the moving cursor gets a
            // full-row background fill; the current-for-role model gets a `●`
            // accent bullet in its gutter.
            let bg = is_selected.then_some(styles.selection_bg());
            let text_fg = if !entry.unavailable_reasons.is_empty() {
                styles.dim()
            } else if is_selected {
                styles.selection_fg()
            } else {
                styles.text()
            };
            let mut spans = Vec::new();
            if is_selected {
                spans.push(Span::raw("  ").bg_opt(bg));
                spans.push(Span::raw("❯ ").fg(styles.selection_fg()).bg_opt(bg).bold());
            } else if entry.is_current_for_role {
                spans.push(Span::raw("  "));
                spans.push(Span::raw("● ").fg(styles.primary()).bold());
            } else {
                spans.push(Span::raw("    "));
            }
            let name = Span::raw(entry.display_name.clone()).fg(text_fg).bg_opt(bg);
            spans.push(if is_selected { name.bold() } else { name });
            if let Some(context_window) = entry.context_window {
                spans.push(
                    Span::raw(format!(" · {}", format_context_window(context_window)))
                        .fg(if is_selected {
                            styles.selection_fg()
                        } else {
                            styles.dim()
                        })
                        .bg_opt(bg),
                );
            }
            if !entry.unavailable_reasons.is_empty() {
                spans.push(
                    Span::raw(format!(" · {}", t!("dialog.model_picker_unavailable_tag")))
                        .fg(styles.warning())
                        .bg_opt(bg)
                        .bold(),
                );
            }
            if !entry.supported_efforts.is_empty() {
                spans.push(
                    Span::raw(format!(" · {}", t!("dialog.model_picker_thinking_tag")))
                        .fg(styles.thinking())
                        .bg_opt(bg),
                );
            }
            if entry.is_current_for_role {
                spans.push(
                    Span::raw(format!("  [{}]", t!("dialog.model_picker_current")))
                        .fg(if is_selected {
                            styles.selection_fg()
                        } else {
                            styles.primary()
                        })
                        .bg_opt(bg)
                        .bold(),
                );
            }
            picker::pad_line(Line::from(spans), width, bg)
        }
    }
}

/// `Thinking:  low  ▸medium◂  high` for the focused model, or an
/// unavailability summary when the focused provider can't be used.
fn render_effort_line(
    m: &ModelPickerState,
    view: &ModelPickerViewModel<'_>,
    styles: UiStyles<'_>,
) -> Line<'static> {
    let Some(entry) = view
        .selected
        .and_then(|selected| view.filtered.get(selected))
    else {
        return Line::from(
            Span::raw(t!("dialog.model_picker_thinking_label").to_string()).fg(styles.dim()),
        );
    };
    if let Some(summary) = unavailable_summary(&entry.unavailable_reasons) {
        return Line::from(vec![
            Span::raw(t!("dialog.model_picker_unavailable_label").to_string())
                .fg(styles.warning())
                .bold(),
            Span::raw("  "),
            Span::raw(summary).fg(styles.dim()),
        ]);
    }
    let mut spans = vec![
        Span::raw(t!("dialog.model_picker_thinking_label").to_string()).fg(styles.dim()),
        Span::raw("  "),
    ];
    if entry.supported_efforts.is_empty() {
        spans.push(
            Span::raw(t!("dialog.model_picker_thinking_unavailable").to_string()).fg(styles.dim()),
        );
        return Line::from(spans);
    }
    let active = m.effort.or(entry.default_effort);
    for (idx, effort) in entry.supported_efforts.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::raw("  "));
        }
        let label = effort.as_str();
        if Some(*effort) == active {
            spans.push(
                Span::raw(format!("▸{label}◂"))
                    .fg(styles.selection_fg())
                    .bg(styles.selection_bg())
                    .bold(),
            );
        } else {
            spans.push(Span::raw(format!(" {label} ")).fg(styles.text()));
        }
    }
    Line::from(spans)
}

/// Format a token count as `1M` / `200K` / `1024`.
fn format_context_window(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        let m = tokens as f64 / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{}M", m.round() as i64)
        } else {
            format!("{m:.1}M")
        }
    } else if tokens >= 1_000 {
        format!("{}K", tokens / 1_000)
    } else {
        format!("{tokens}")
    }
}

fn unavailable_summary(reasons: &[ProviderUnavailableReason]) -> Option<String> {
    if reasons.is_empty() {
        return None;
    }
    Some(
        reasons
            .iter()
            .map(unavailable_reason_label)
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn unavailable_reason_label(reason: &ProviderUnavailableReason) -> String {
    match reason {
        ProviderUnavailableReason::MissingBaseUrl => {
            t!("dialog.model_picker_unavailable_base_url").to_string()
        }
        ProviderUnavailableReason::MissingApiKey { env_key } => t!(
            "dialog.model_picker_unavailable_api_key",
            env_key = env_key.as_str()
        )
        .to_string(),
        ProviderUnavailableReason::NotLoggedIn { provider } => t!(
            "dialog.model_picker_unavailable_not_logged_in",
            provider = provider.as_str()
        )
        .to_string(),
        ProviderUnavailableReason::NoModels => {
            t!("dialog.model_picker_unavailable_no_models").to_string()
        }
    }
}

/// User-facing role display name.
fn role_display(role: ModelRole) -> String {
    let key = match role {
        ModelRole::Main => "role.main",
        ModelRole::Fast => "role.fast",
        ModelRole::Plan => "role.plan",
        ModelRole::Explore => "role.explore",
        ModelRole::Review => "role.review",
        ModelRole::HookAgent => "role.hook_agent",
        ModelRole::Memory => "role.memory",
        ModelRole::Subagent => "role.subagent",
    };
    t!(key).to_string()
}

#[cfg(test)]
#[path = "model_picker.test.rs"]
mod tests;
