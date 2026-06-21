//! Model picker presentation.

use coco_types::ModelRole;
use ratatui::prelude::*;
#[cfg(test)]
use ratatui::widgets::Block;
#[cfg(test)]
use ratatui::widgets::Borders;
#[cfg(test)]
use ratatui::widgets::Clear;
#[cfg(test)]
use ratatui::widgets::Paragraph;

use super::layout;
#[cfg(test)]
use super::picker;
#[cfg(test)]
use super::picker::PickerListView;
#[cfg(test)]
use super::picker::PickerRow;
#[cfg(test)]
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

#[cfg(test)]
const MODEL_PICKER_BOUNDS: layout::ModalBounds = layout::ModalBounds::new(70, 80, 60, 112, 18, 32);

#[cfg(test)]
struct ModelPickerViewModel<'a> {
    filtered: Vec<&'a ModelEntry>,
    list: PickerListView<'a, ModelEntry>,
    selected: Option<usize>,
}

#[cfg(test)]
pub(crate) fn render_model_picker(
    frame: &mut Frame,
    area: Rect,
    m: &ModelPickerState,
    styles: UiStyles<'_>,
) {
    let role_label = role_display(m.role);
    let title = t!("dialog.model_picker_title", role = role_label.as_str()).to_string();
    let modal_area = layout::centered_modal_area(area, MODEL_PICKER_BOUNDS);

    frame.render_widget(Clear, modal_area);

    let (inner_width, inner_height) = layout::inner_size(modal_area);
    let lines = render_model_picker_lines(m, styles, inner_width, inner_height);
    let content = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(ratatui::style::Style::default().fg(styles.modal_border())),
    );

    frame.render_widget(content, modal_area);
}

pub(crate) fn content(m: &ModelPickerState, styles: UiStyles<'_>) -> (String, String, Color) {
    let filtered = filtered_entries(m);
    let role_line = render_role_pill(m.role);
    let filter_line = if m.filter.is_empty() {
        t!("dialog.model_picker_type_filter").to_string()
    } else {
        t!("dialog.filter_prefix", text = m.filter.as_str()).to_string()
    };

    let model_lines = render_grouped_models(&filtered, m.selected);
    let footer = render_effort_footer(m, &filtered);
    let hints = t!("dialog.model_picker_hints").to_string();

    let body = if footer.is_empty() {
        format!("{role_line}\n\n{filter_line}\n\n{model_lines}\n\n{hints}")
    } else {
        format!("{role_line}\n\n{filter_line}\n\n{model_lines}\n\n{footer}\n\n{hints}")
    };

    let role_label = role_display(m.role);
    let title = t!("dialog.model_picker_title", role = role_label.as_str()).to_string();
    (title, body, styles.primary())
}

#[cfg(test)]
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

#[cfg(test)]
fn render_model_picker_lines(
    m: &ModelPickerState,
    styles: UiStyles<'_>,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    if height == 0 {
        return Vec::new();
    }

    let reserved = 8usize;
    let list_height = height.saturating_sub(reserved).max(1);
    let view = build_view_model(m, list_height);

    let mut lines = Vec::with_capacity(height);
    lines.push(picker::pad_line(
        render_role_tabs(m.role, styles),
        width,
        None,
    ));
    lines.push(picker::blank_line(width));
    lines.push(picker::pad_line(render_filter_line(m, styles), width, None));
    lines.push(picker::blank_line(width));

    if view.list.rows.is_empty() {
        lines.push(picker::pad_line(
            Line::from(Span::raw(t!("dialog.model_picker_empty").to_string()).fg(styles.dim())),
            width,
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
            lines.push(render_model_row(row, view.selected, styles, width));
        }
    }

    while lines.len() < height.saturating_sub(4) {
        lines.push(picker::blank_line(width));
    }
    lines.truncate(height.saturating_sub(4));
    lines.push(picker::blank_line(width));
    lines.push(picker::pad_line(
        render_effort_line(m, &view, styles),
        width,
        None,
    ));
    lines.push(picker::blank_line(width));
    let hints_text = t!("dialog.model_picker_hints");
    let hints = picker::collapse_hints(hints_text.as_ref(), width);
    lines.push(picker::pad_line(
        Line::from(Span::raw(hints).fg(styles.dim())),
        width,
        None,
    ));
    lines.truncate(height);
    lines
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

#[cfg(test)]
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

#[cfg(test)]
fn render_filter_line(m: &ModelPickerState, styles: UiStyles<'_>) -> Line<'static> {
    if m.filter.is_empty() {
        Line::from(Span::raw(t!("dialog.model_picker_type_filter").to_string()).fg(styles.dim()))
    } else {
        Line::from(vec![
            Span::raw(t!("dialog.filter_prefix", text = "").to_string()).fg(styles.dim()),
            Span::raw(m.filter.clone()).fg(styles.text()),
        ])
    }
}

#[cfg(test)]
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
            let bg = entry.is_current_for_role.then_some(styles.selection_bg());
            let text_fg = if entry.unavailable_reasons.is_empty() {
                styles.text()
            } else {
                styles.dim()
            };
            let mut spans = Vec::new();
            if is_selected {
                spans.push(Span::raw("  ").bg_opt(bg));
                spans.push(Span::raw("❯ ").fg(styles.selection_fg()).bg_opt(bg).bold());
            } else {
                spans.push(Span::raw("    ").bg_opt(bg));
            }
            let name = Span::raw(entry.display_name.clone()).fg(text_fg).bg_opt(bg);
            spans.push(if is_selected { name.bold() } else { name });
            if let Some(context_window) = entry.context_window {
                spans.push(
                    Span::raw(format!(" · {}", format_context_window(context_window)))
                        .fg(styles.dim())
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
                        .fg(styles.selection_fg())
                        .bg_opt(bg)
                        .bold(),
                );
            }
            picker::pad_line(Line::from(spans), width, bg)
        }
    }
}

#[cfg(test)]
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

/// Render the role pill row, e.g. `Role:  ▸Main◂  Fast  Plan  ...`.
fn render_role_pill(active: ModelRole) -> String {
    let parts: Vec<String> = ROLE_ORDER
        .iter()
        .map(|r| {
            if *r == active {
                format!("▸{}◂", role_display(*r))
            } else {
                format!(" {} ", role_display(*r))
            }
        })
        .collect();
    format!(
        "{}  {}",
        t!("dialog.model_picker_role_label"),
        parts.join("  ")
    )
}

fn render_grouped_models(entries: &[&ModelEntry], selected: i32) -> String {
    if entries.is_empty() {
        return t!("dialog.model_picker_empty").to_string();
    }
    let mut out: Vec<String> = Vec::with_capacity(entries.len() + 8);
    let mut last_provider: Option<&str> = None;
    for (i, entry) in entries.iter().enumerate() {
        if last_provider != Some(entry.provider_display.as_str()) {
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push(entry.provider_display.clone());
            last_provider = Some(entry.provider_display.as_str());
        }
        let marker = if i as i32 == selected {
            "  ❯ "
        } else {
            "    "
        };
        let context = entry
            .context_window
            .map(|w| format!(" · {}", format_context_window(w)))
            .unwrap_or_default();
        let thinking = if entry.supported_efforts.is_empty() {
            String::new()
        } else {
            format!(" · {}", t!("dialog.model_picker_thinking_tag"))
        };
        let unavailable = if entry.unavailable_reasons.is_empty() {
            String::new()
        } else {
            format!(" · {}", t!("dialog.model_picker_unavailable_tag"))
        };
        let current = if entry.is_current_for_role {
            format!("  [{}]", t!("dialog.model_picker_current"))
        } else {
            String::new()
        };
        out.push(format!(
            "{marker}{}{context}{unavailable}{thinking}{current}",
            entry.display_name
        ));
    }
    out.join("\n")
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

/// Render the thinking-effort footer for the focused model.
fn render_effort_footer(m: &ModelPickerState, filtered: &[&ModelEntry]) -> String {
    let Some(entry) =
        layout::selected_in_bounds(m.selected, filtered.len()).and_then(|idx| filtered.get(idx))
    else {
        return String::new();
    };
    if let Some(summary) = unavailable_summary(&entry.unavailable_reasons) {
        return format!("{}  {summary}", t!("dialog.model_picker_unavailable_label"));
    }
    if entry.supported_efforts.is_empty() {
        return String::new();
    }
    let active = m.effort.or(entry.default_effort);
    let chips: Vec<String> = entry
        .supported_efforts
        .iter()
        .map(|e| {
            let label = e.as_str();
            if Some(*e) == active {
                format!("▸{label}◂")
            } else {
                format!(" {label} ")
            }
        })
        .collect();
    format!(
        "{}  {}",
        t!("dialog.model_picker_thinking_label"),
        chips.join("  ")
    )
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
