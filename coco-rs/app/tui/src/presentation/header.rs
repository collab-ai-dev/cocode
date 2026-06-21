//! Header chrome shared by native history and state presentation.

use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use coco_types::ModelRole;

use crate::i18n::t;
use crate::state::AppState;
use coco_tui_ui::style::UiStyles;

/// Logo gutter width (9 logo cells + 2-space padding).
const HEADER_LOGO_WIDTH: u16 = 11;

/// Crate version surfaced in the header bar.
const COCO_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) struct HeaderBarView {
    pub(crate) logo_lines: Vec<Line<'static>>,
    pub(crate) info_lines: Vec<Line<'static>>,
}

/// Header band: 3-row COCO mascot + 3 info rows.
///
/// Row 1 shows brand + version, row 2 shows the active main model with the
/// live thinking-effort dial and fast-mode flag, and row 3 shows cwd + git
/// branch + worktree when present.
pub(crate) fn header_bar_view(
    state: &AppState,
    styles: UiStyles<'_>,
    info_width: u16,
) -> HeaderBarView {
    let logo_color = Style::default().fg(styles.primary());
    let logo_lines = vec![
        Line::from(Span::styled(" ╭─╮ ╭─╮  ", logo_color)),
        Line::from(Span::styled(" │●│ │●│  ", logo_color)),
        Line::from(Span::styled(" ╰─╯ ╰─╯  ", logo_color)),
    ];

    let mut row1_spans = vec![
        Span::styled("COCO", Style::default().fg(styles.text()).bold()),
        Span::raw(" "),
        Span::styled(
            format!("v{COCO_VERSION}"),
            Style::default().fg(styles.dim()),
        ),
    ];
    // `pid == 0` is the unset sentinel (tests / pre-bootstrap state); only the
    // real app stamps a live pid in `App::new`. Surfacing it lets concurrent
    // sessions be told apart and matched to `logs/coco.<pid>.log.<date>`.
    if state.session.pid != 0 {
        row1_spans.push(Span::styled(
            format!("  ·  pid {}", state.session.pid),
            Style::default().fg(styles.dim()),
        ));
    }
    let row1 = Line::from(row1_spans);

    let (provider, model_id) = state
        .session
        .model_by_role
        .get(&ModelRole::Main)
        .map(|binding| (binding.provider.clone(), binding.model_id.clone()))
        .unwrap_or_else(|| (state.session.provider.clone(), state.session.model.clone()));
    let row2 = if model_id.is_empty() {
        Line::from(Span::styled(
            t!("status.no_model").to_string(),
            Style::default().fg(styles.dim()).italic(),
        ))
    } else {
        let model = if provider.is_empty() {
            model_id
        } else {
            format!("{provider}/{model_id}")
        };
        let mut spans = vec![
            Span::styled(model, Style::default().fg(styles.primary()).bold()),
            Span::styled("  *  ", Style::default().fg(styles.border())),
            Span::styled(
                state.session.thinking_effort.to_string(),
                Style::default().fg(styles.accent()),
            ),
        ];
        if state.session.fast_mode {
            spans.push(Span::raw("  "));
            spans.push(Span::styled("⚡", Style::default().fg(styles.warning())));
        }
        Line::from(spans)
    };

    let mut row3_spans: Vec<Span<'static>> = Vec::new();
    if let Some(ref dir) = state.session.working_dir {
        let display = tildify_path(dir);
        let max_w = info_width.saturating_sub(2) as usize;
        row3_spans.push(Span::styled(
            truncate_path_for_width(&display, max_w),
            Style::default().fg(styles.dim()),
        ));
    }
    if let Some(ref branch) = state.session.git_branch {
        if !row3_spans.is_empty() {
            row3_spans.push(Span::raw(" "));
        }
        row3_spans.push(Span::styled(
            format!(" {branch}"),
            Style::default().fg(styles.dim()),
        ));
    }
    if let Some(ref wt) = state.session.worktree_path {
        let short = wt.rsplit('/').next().unwrap_or(wt);
        if !row3_spans.is_empty() {
            row3_spans.push(Span::raw(" "));
        }
        row3_spans.push(Span::styled(
            format!("🌿 {short}"),
            Style::default().fg(styles.success()),
        ));
    }

    HeaderBarView {
        logo_lines,
        info_lines: vec![row1, row2, Line::from(row3_spans)],
    }
}

/// Cheap content key over every input [`header_history_lines`] reads — the
/// native surface rebuilds (and re-fingerprints) the session header only when
/// this key changes, instead of building ~6 `Line`s + hashing them per frame.
/// MUST cover every state field the header renderer consumes, or a header
/// change silently stops triggering the history replay.
pub(crate) fn header_input_key(state: &AppState, theme_hash: u64, width: u16) -> u64 {
    use std::hash::Hash;
    use std::hash::Hasher;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    width.hash(&mut h);
    theme_hash.hash(&mut h);
    state.session.pid.hash(&mut h);
    match state.session.model_by_role.get(&ModelRole::Main) {
        Some(binding) => {
            binding.provider.hash(&mut h);
            binding.model_id.hash(&mut h);
        }
        None => {
            state.session.provider.hash(&mut h);
            state.session.model.hash(&mut h);
        }
    }
    state.session.thinking_effort.hash(&mut h);
    state.session.fast_mode.hash(&mut h);
    state.session.working_dir.hash(&mut h);
    state.session.git_branch.hash(&mut h);
    state.session.worktree_path.hash(&mut h);
    h.finish()
}

pub(crate) fn header_history_lines(
    state: &AppState,
    styles: UiStyles<'_>,
    width: u16,
) -> Vec<Line<'static>> {
    let info_width = width.saturating_sub(HEADER_LOGO_WIDTH.min(width));
    let view = header_bar_view(state, styles, info_width);

    let mut lines: Vec<_> = view
        .logo_lines
        .into_iter()
        .zip(view.info_lines)
        .map(|(logo, info)| {
            let mut spans = logo.spans;
            spans.extend(info.spans);
            Line::from(spans)
        })
        .collect();
    lines.push(Line::default());
    lines
}

fn tildify_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir()
        && let Some(home_str) = home.to_str()
        && let Some(rest) = path.strip_prefix(home_str)
    {
        return if rest.is_empty() {
            "~".to_string()
        } else if rest.starts_with('/') {
            format!("~{rest}")
        } else {
            format!("~/{rest}")
        };
    }
    path.to_string()
}

fn truncate_path_for_width(path: &str, max_width: usize) -> String {
    if max_width == 0 || path.chars().count() <= max_width {
        return path.to_string();
    }
    let suffix_chars: String = path
        .chars()
        .rev()
        .take(max_width.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{suffix_chars}")
}

#[cfg(test)]
#[path = "header.test.rs"]
mod tests;
