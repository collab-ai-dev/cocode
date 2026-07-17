//! Styled renderer for the `/journey` learning-timeline overlay.
//!
//! Layout: a legend, a recency-inked horizontal timeline (skill `━` + memory
//! `◆` segments), an axis, then either the node list (List mode) or a detail
//! panel (Detail mode). Returns `(title, body lines, border)` for the styled
//! modal path. All clipping goes through `truncate_to_width`; timeline ink is
//! computed at render time via `coco_tui_ui::color::rgb`.

use ratatui::prelude::*;

use crate::i18n::t;
use crate::presentation::layout::truncate_to_width;
use crate::state::JourneyMode;
use crate::state::JourneyState;
use coco_tui_ui::style::UiStyles;
use coco_tui_ui::widgets::SelectItem;
use coco_tui_ui::widgets::SelectListStyle;
use coco_tui_ui::widgets::render_select_list;
use coco_types::{JourneyEvent, JourneyNodeBodyWire, JourneyNodeWire};

const SKILL_GLYPH: char = '━';
const MEMORY_GLYPH: char = '◆';
const SKILL_BADGE: char = '●';
const MEMORY_BADGE: char = '◆';
const PROMOTED_MARK: char = '★';
const RETIRED_BADGE: char = '✕';
/// Maximum timeline bar length (cells); longer runs scale down.
const BAR_MAX: usize = 30;
/// Column width for a node title before ellipsis.
const TITLE_WIDTH: usize = 32;
/// Column width for a timeline bucket label. Sized to the widest label
/// `bucketize` emits — month granularity, `"Jul 2026"` (3 + 1 + 4 = 8). A
/// narrower budget clips it to `"Jul 202"`, which reads as a corrupt year.
const BUCKET_LABEL_WIDTH: usize = 8;

/// Entry point for the styled-modal path.
pub(crate) fn journey_lines(
    j: &JourneyState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> (String, Vec<Line<'static>>, Color) {
    let title = t!("dialog.title_journey").to_string();
    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(legend_line(j, styles));
    lines.push(Line::default());

    if j.nodes.is_empty() {
        lines.push(dim_line(t!("dialog.journey_empty"), styles));
        lines.push(Line::default());
        lines.push(dim_line(t!("dialog.journey_hints"), styles));
        return (title, lines, styles.primary());
    }

    lines.extend(timeline_bars(j, styles));
    lines.push(dim_line(t!("dialog.journey_axis"), styles));
    lines.push(dim_line("─".repeat(46), styles));

    match &j.mode {
        JourneyMode::Detail => {
            lines.extend(detail_panel(j, styles));
            lines.push(Line::default());
            lines.push(dim_line(t!("dialog.journey_hints_detail"), styles));
        }
        JourneyMode::DeleteMemoryConfirm { yes_selected } => {
            lines.extend(confirm_panel(j, *yes_selected, styles));
        }
        JourneyMode::List => {
            let items: Vec<SelectItem> = j
                .nodes
                .iter()
                .map(|n| SelectItem::new(node_row_label(n, styles)))
                .collect();
            lines.extend(render_select_list(
                &items,
                j.selected.min(items.len().saturating_sub(1)),
                &SelectListStyle {
                    numbered: false,
                    visible_count: list_budget.max(1),
                },
                styles,
            ));
            lines.push(Line::default());
            lines.push(dim_line(t!("dialog.journey_hints"), styles));
        }
    }

    (title, lines, styles.primary())
}

fn dim_line(text: impl Into<String>, styles: UiStyles<'_>) -> Line<'static> {
    Line::from(Span::styled(text.into(), Style::default().fg(styles.dim())))
}

fn legend_line(j: &JourneyState, styles: UiStyles<'_>) -> Line<'static> {
    let s = &j.stats;
    let skills = s.learning + s.learned + s.user_skills;
    Line::from(vec![
        Span::styled(
            format!("{SKILL_BADGE} "),
            Style::default().fg(styles.accent()),
        ),
        Span::styled(
            t!("dialog.journey_legend_skills", count = skills).to_string(),
            Style::default().fg(styles.text()),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{MEMORY_BADGE} "),
            Style::default().fg(styles.plan()),
        ),
        Span::styled(
            t!("dialog.journey_legend_memories", count = s.memories).to_string(),
            Style::default().fg(styles.text()),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{PROMOTED_MARK} "),
            Style::default().fg(styles.success()),
        ),
        Span::styled(
            t!("dialog.journey_legend_promoted", count = s.learned).to_string(),
            Style::default().fg(styles.text()),
        ),
        Span::raw("   "),
        Span::styled(
            format!("{RETIRED_BADGE} "),
            Style::default().fg(styles.dim()),
        ),
        Span::styled(
            t!("dialog.journey_legend_retired", count = s.retired).to_string(),
            Style::default().fg(styles.text()),
        ),
    ])
}

fn timeline_bars(j: &JourneyState, styles: UiStyles<'_>) -> Vec<Line<'static>> {
    let max_total = j
        .buckets
        .iter()
        .map(|b| (b.skills + b.memories).max(0))
        .max()
        .unwrap_or(1)
        .max(1);
    j.buckets
        .iter()
        .map(|b| {
            let total = (b.skills + b.memories).max(0);
            let glyphs = if max_total as usize <= BAR_MAX {
                total as usize
            } else {
                ((total as f64 / max_total as f64) * BAR_MAX as f64).round() as usize
            };
            let s_glyphs = if total > 0 {
                ((glyphs as f64) * (b.skills.max(0) as f64) / total as f64).round() as usize
            } else {
                0
            };
            let m_glyphs = glyphs.saturating_sub(s_glyphs);
            let mut bar = SKILL_GLYPH.to_string().repeat(s_glyphs);
            bar.push_str(&MEMORY_GLYPH.to_string().repeat(m_glyphs));
            Line::from(vec![
                Span::styled(
                    format!(
                        "{:>BUCKET_LABEL_WIDTH$} ",
                        truncate_to_width(&b.label, BUCKET_LABEL_WIDTH)
                    ),
                    Style::default().fg(styles.dim()),
                ),
                Span::styled("│".to_string(), Style::default().fg(styles.dim())),
                Span::styled(bar, Style::default().fg(recency_color(b.recency))),
                Span::styled(
                    format!("  {}+{}", b.skills, b.memories),
                    Style::default().fg(styles.dim()),
                ),
            ])
        })
        .collect()
}

/// Linear-interpolated ink from old (dim slate) to new (bright cyan) by
/// recency, emitted through `color::rgb` so it auto-downsamples under Ansi256.
fn recency_color(recency: f32) -> Color {
    let t = recency.clamp(0.0, 1.0);
    let lerp = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    coco_tui_ui::color::rgb(lerp(96, 96), lerp(100, 210), lerp(120, 245))
}

fn node_row_label(node: &JourneyNodeWire, _styles: UiStyles<'_>) -> String {
    let (badge, status) = badge_and_status(node);
    let title = truncate_to_width(&node.title, TITLE_WIDTH);
    if status.is_empty() {
        format!("{badge} {title}   {}", node.date_label)
    } else {
        format!("{badge} {title}   {status}   {}", node.date_label)
    }
}

/// Quarantine progress label (`learning 2/5`). Single definition shared by the
/// `/journey` list rows and the `/skills` dialog so the two cannot drift.
///
/// Both numbers come from the host ([`coco_types::SkillQuarantineWire`]) — the
/// threshold is an operator setting, so there is deliberately no UI-side
/// constant to fall back on.
pub(crate) fn quarantine_progress_label(progress: coco_types::SkillQuarantineWire) -> String {
    t!(
        "dialog.journey_learning",
        done = progress.invocations,
        total = progress.required
    )
    .to_string()
}

/// Row badge + short status text for a node.
fn badge_and_status(node: &JourneyNodeWire) -> (char, String) {
    match &node.body {
        JourneyNodeBodyWire::AgentSkill { lifecycle, .. } => match lifecycle {
            coco_types::AgentSkillLifecycleWire::Learning { progress } => {
                (SKILL_BADGE, quarantine_progress_label(*progress))
            }
            coco_types::AgentSkillLifecycleWire::Learned => (
                SKILL_BADGE,
                format!("{} {PROMOTED_MARK}", t!("dialog.journey_learned")),
            ),
            coco_types::AgentSkillLifecycleWire::Retired => {
                (RETIRED_BADGE, t!("dialog.journey_retired").to_string())
            }
        },
        JourneyNodeBodyWire::UserSkill { .. } => (SKILL_BADGE, String::new()),
        JourneyNodeBodyWire::Memory { .. } => (MEMORY_BADGE, String::new()),
    }
}

fn detail_panel(j: &JourneyState, styles: UiStyles<'_>) -> Vec<Line<'static>> {
    let Some(node) = j.selected_node() else {
        return vec![dim_line(t!("dialog.journey_empty"), styles)];
    };
    let mut lines = Vec::new();
    let (badge, status) = badge_and_status(node);
    lines.push(Line::from(vec![
        Span::styled(format!("{badge} "), Style::default().fg(styles.accent())),
        Span::styled(node.title.clone(), Style::default().fg(styles.text())),
        Span::raw("  "),
        Span::styled(status, Style::default().fg(styles.dim())),
    ]));
    if !node.description.is_empty() {
        lines.push(Line::default());
        for wrapped in textwrap::wrap(&node.description, 60) {
            lines.push(Line::from(Span::styled(
                wrapped.to_string(),
                Style::default().fg(styles.text()),
            )));
        }
    }
    // Telemetry (skills only).
    if let Some(tel) = telemetry_of(node) {
        lines.push(Line::default());
        lines.push(dim_line(
            t!(
                "dialog.journey_telemetry",
                success = tel.success_count,
                failure = tel.failure_count,
                patches = tel.patch_count
            ),
            styles,
        ));
    }
    // Journal history, newest first.
    if !node.history.is_empty() {
        lines.push(Line::default());
        lines.push(dim_line(t!("dialog.journey_history"), styles));
        for record in &node.history {
            lines.push(Line::from(Span::styled(
                format!("  • {}", event_label(&record.event)),
                Style::default().fg(styles.dim()),
            )));
        }
    }
    lines
}

fn confirm_panel(j: &JourneyState, yes_selected: bool, styles: UiStyles<'_>) -> Vec<Line<'static>> {
    let name = j
        .selected_node()
        .map(|n| n.title.clone())
        .unwrap_or_default();
    let mut lines = vec![
        Line::from(Span::styled(
            t!("dialog.journey_delete_confirm", name = name).to_string(),
            Style::default().fg(styles.warning()),
        )),
        Line::default(),
    ];
    // No (default) on the left, Yes on the right; the selected one is accented.
    let no_style = if yes_selected {
        Style::default().fg(styles.dim())
    } else {
        Style::default().fg(styles.accent())
    };
    let yes_style = if yes_selected {
        Style::default().fg(styles.accent())
    } else {
        Style::default().fg(styles.dim())
    };
    lines.push(Line::from(vec![
        Span::styled(t!("dialog.journey_confirm_no").to_string(), no_style),
        Span::raw("      "),
        Span::styled(t!("dialog.journey_confirm_yes").to_string(), yes_style),
    ]));
    lines.push(Line::default());
    lines.push(dim_line(t!("dialog.journey_hints_confirm"), styles));
    lines
}

fn telemetry_of(node: &JourneyNodeWire) -> Option<&coco_types::SkillTelemetryWire> {
    match &node.body {
        JourneyNodeBodyWire::AgentSkill { telemetry, .. }
        | JourneyNodeBodyWire::UserSkill { telemetry, .. } => Some(telemetry),
        JourneyNodeBodyWire::Memory { .. } => None,
    }
}

fn event_label(event: &JourneyEvent) -> String {
    match event {
        JourneyEvent::SkillLearned { .. } => t!("dialog.journey_ev_learned").to_string(),
        JourneyEvent::SkillUpdated { .. } => t!("dialog.journey_ev_updated").to_string(),
        JourneyEvent::SkillPromoted { .. } => {
            format!("{} {PROMOTED_MARK}", t!("dialog.journey_ev_promoted"))
        }
        JourneyEvent::SkillRetired { reason, .. } => {
            t!("dialog.journey_ev_retired", reason = reason.as_str()).to_string()
        }
        JourneyEvent::SkillRestored { .. } => t!("dialog.journey_ev_restored").to_string(),
        JourneyEvent::MemoryWritten { files } => {
            t!("dialog.journey_ev_written", count = files.len()).to_string()
        }
        JourneyEvent::MemoryConsolidated { files_touched } => {
            t!("dialog.journey_ev_consolidated", count = files_touched).to_string()
        }
        JourneyEvent::MemoryDeleted { .. } => t!("dialog.journey_ev_deleted").to_string(),
    }
}

#[cfg(test)]
#[path = "journey.test.rs"]
mod tests;
