//! `/provider` add-provider wizard presentation.
//!
//! Rendered through a dedicated styled branch in `surface/modal.rs` (like the
//! model / theme pickers): a fixed-width box whose body depends on the active
//! [`ProviderWizardStep`]. The template step is a select list; the remaining
//! steps are single-line text fields with a block caret; the confirm step is a
//! read-only summary.

use ratatui::prelude::*;

use super::layout;
use super::picker;
use crate::i18n::t;
use crate::state::ProviderWizardState;
use crate::state::ProviderWizardStep;
use crate::state::WizardTextField;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::widgets::SelectItem;
use coco_tui_ui::widgets::SelectListStyle;
use coco_tui_ui::widgets::render_select_list;

/// `<label>  ` prefix width in display columns (`›` + two spaces).
const FIELD_LABEL_COLS: u16 = 3;
/// Content-relative row of the text field within a text step (prompt, blank,
/// then field).
const FIELD_ROW: u16 = 2;

/// Fixed, step-independent title so the top border never changes width.
pub(crate) fn provider_wizard_title() -> String {
    t!("dialog.provider_wizard_title").to_string()
}

/// Content-relative `(col, row)` of the caret for the active text step, or
/// `None` for the select (`Template`) / read-only (`Confirm`) steps. Lets the
/// modal renderer pin the terminal cursor for IME anchoring.
pub(crate) fn active_field_caret(m: &ProviderWizardState) -> Option<(u16, u16)> {
    let field = m.active_field()?;
    let (before, _) = field.split_at_cursor();
    // The API-key field renders as `•` bullets (1 col each); everything else is
    // shown verbatim, so measure display width.
    let cols = if matches!(m.step, ProviderWizardStep::ApiKey) {
        before.chars().count() as u16
    } else {
        layout::text_width(before) as u16
    };
    Some((FIELD_LABEL_COLS + cols, FIELD_ROW))
}

fn dim_line(text: impl Into<String>, styles: UiStyles<'_>, width: usize) -> Line<'static> {
    picker::pad_line(
        Line::from(Span::styled(text.into(), Style::default().fg(styles.dim()))),
        width,
        None,
    )
}

/// A single-line text field `<label>  <value>`. The caret is the terminal
/// hardware cursor (pinned by the modal renderer), not a styled span — so a CJK
/// IME anchors correctly and there's no double caret. `mask` renders each char
/// as `•` for the secret field.
fn field_line(
    label: &str,
    field: &WizardTextField,
    styles: UiStyles<'_>,
    width: usize,
    mask: bool,
    placeholder: &str,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{label}  "),
        Style::default().fg(styles.dim()),
    )];
    if field.text.is_empty() {
        spans.push(Span::styled(
            placeholder.to_string(),
            Style::default().fg(styles.dim()),
        ));
    } else {
        let shown = if mask {
            "•".repeat(field.text.chars().count())
        } else {
            field.text.clone()
        };
        spans.push(Span::styled(shown, Style::default().fg(styles.text())));
    }
    picker::pad_line(Line::from(spans), width, None)
}

/// Styled body lines for the wizard's active step. The caller wraps them in the
/// bordered box + title.
pub(crate) fn provider_wizard_lines(
    m: &ProviderWizardState,
    styles: UiStyles<'_>,
    inner_width: usize,
    list_visible: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    match m.step {
        ProviderWizardStep::Template => {
            lines.push(dim_line(
                t!("dialog.provider_wizard_pick_template"),
                styles,
                inner_width,
            ));
            lines.push(picker::blank_line(inner_width));
            let items: Vec<SelectItem> = m
                .templates
                .iter()
                .map(|tpl| {
                    let secondary = if tpl.is_custom {
                        t!("dialog.provider_wizard_custom_hint").to_string()
                    } else {
                        tpl.api.as_str().to_string()
                    };
                    SelectItem::new(tpl.name.clone()).with_secondary(secondary)
                })
                .collect();
            lines.extend(render_select_list(
                &items,
                m.template_idx.min(items.len().saturating_sub(1)),
                &SelectListStyle {
                    numbered: false,
                    visible_count: list_visible.max(1),
                },
                styles,
            ));
        }
        ProviderWizardStep::Name => {
            lines.push(dim_line(
                t!("dialog.provider_wizard_name"),
                styles,
                inner_width,
            ));
            lines.push(picker::blank_line(inner_width));
            lines.push(field_line(
                "›",
                &m.name,
                styles,
                inner_width,
                false,
                &t!("dialog.provider_wizard_name_placeholder"),
            ));
        }
        ProviderWizardStep::BaseUrl => {
            lines.push(dim_line(
                t!("dialog.provider_wizard_base_url"),
                styles,
                inner_width,
            ));
            lines.push(picker::blank_line(inner_width));
            lines.push(field_line(
                "›",
                &m.base_url,
                styles,
                inner_width,
                false,
                "https://api.example.com/v1",
            ));
        }
        ProviderWizardStep::ApiKey => {
            lines.push(dim_line(
                t!("dialog.provider_wizard_api_key"),
                styles,
                inner_width,
            ));
            lines.push(picker::blank_line(inner_width));
            lines.push(field_line(
                "›",
                &m.api_key,
                styles,
                inner_width,
                true,
                &t!("dialog.provider_wizard_api_key_placeholder"),
            ));
        }
        ProviderWizardStep::Model => {
            lines.push(dim_line(
                t!("dialog.provider_wizard_model"),
                styles,
                inner_width,
            ));
            lines.push(picker::blank_line(inner_width));
            lines.push(field_line(
                "›",
                &m.model_id,
                styles,
                inner_width,
                false,
                &t!("dialog.provider_wizard_model_placeholder"),
            ));
        }
        ProviderWizardStep::Confirm => {
            lines.extend(confirm_summary(m, styles, inner_width));
        }
    }

    if let Some(err) = &m.error {
        lines.push(picker::blank_line(inner_width));
        lines.push(picker::pad_line(
            Line::from(Span::styled(
                err.clone(),
                Style::default().fg(styles.warning()).bold(),
            )),
            inner_width,
            None,
        ));
    }

    lines.push(picker::blank_line(inner_width));
    let hint = if matches!(m.step, ProviderWizardStep::Confirm) {
        t!("dialog.provider_wizard_hints_confirm")
    } else if matches!(m.step, ProviderWizardStep::Template) {
        t!("dialog.provider_wizard_hints_select")
    } else {
        t!("dialog.provider_wizard_hints_input")
    };
    lines.push(dim_line(hint, styles, inner_width));
    lines
}

fn confirm_summary(
    m: &ProviderWizardState,
    styles: UiStyles<'_>,
    width: usize,
) -> Vec<Line<'static>> {
    let tpl = m.selected_template();
    let key_display = if m.api_key.text.is_empty() {
        t!("dialog.provider_wizard_key_env", env = tpl.env_key.as_str()).to_string()
    } else {
        "•".repeat(m.api_key.text.chars().count().min(24))
    };
    let row = |k: &str, v: String| -> Line<'static> {
        picker::pad_line(
            Line::from(vec![
                Span::styled(format!("{k:<10}"), Style::default().fg(styles.dim())),
                Span::styled(v, Style::default().fg(styles.text())),
            ]),
            width,
            None,
        )
    };
    let mut lines = vec![dim_line(
        t!("dialog.provider_wizard_confirm"),
        styles,
        width,
    )];
    lines.push(picker::blank_line(width));
    lines.push(row(
        &t!("dialog.provider_wizard_field_name"),
        m.resolved_name(),
    ));
    lines.push(row("API", tpl.api.as_str().to_string()));
    lines.push(row(
        &t!("dialog.provider_wizard_field_base_url"),
        m.resolved_base_url(),
    ));
    lines.push(row(&t!("dialog.provider_wizard_field_key"), key_display));
    if !m.model_id.text.trim().is_empty() {
        lines.push(row(
            &t!("dialog.provider_wizard_field_model"),
            m.model_id.text.trim().to_string(),
        ));
    }
    lines
}

#[cfg(test)]
#[path = "provider_wizard.test.rs"]
mod tests;
