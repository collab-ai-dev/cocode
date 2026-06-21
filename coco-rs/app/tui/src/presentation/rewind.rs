//! Rewind state presentation.

use ratatui::prelude::Color;

use super::layout;
use crate::i18n::t;
use crate::state::DiffStatsPreview;
use crate::state::rewind::RestoreType;
use crate::state::rewind::RewindPhase;
use crate::state::rewind::RewindState;
use crate::state::rewind::RewindableMessage;
use coco_tui_ui::constants;
use coco_tui_ui::style::UiStyles;

pub(crate) fn rewind_surface_content(
    state: &RewindState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    match state.phase {
        RewindPhase::MessageSelect => message_select(state, styles),
        RewindPhase::RestoreOptions => restore_options(state, styles),
        RewindPhase::SummarizeFeedback => summarize_feedback(state, styles),
        RewindPhase::Confirming => confirming(state, styles),
    }
}

fn confirming(state: &RewindState, styles: UiStyles<'_>) -> (String, String, Color) {
    let body = if state.pending_summarize.is_some() {
        t!("dialog.rewind_summarizing")
    } else {
        t!("dialog.rewind_in_progress")
    };
    (
        t!("dialog.title_rewind").to_string(),
        body.to_string(),
        styles.accent(),
    )
}

fn summarize_feedback(state: &RewindState, styles: UiStyles<'_>) -> (String, String, Color) {
    let prompt = t!("dialog.rewind_summarize_prompt");
    let typed = if state.summarize_feedback.is_empty() {
        t!("dialog.rewind_summarize_placeholder").into_owned()
    } else {
        state.summarize_feedback.clone()
    };
    (
        t!("dialog.title_rewind").to_string(),
        format!(
            "{prompt}\n\n  > {typed}\n\n{}",
            t!("dialog.hints_summarize_feedback")
        ),
        styles.accent(),
    )
}

fn message_select(state: &RewindState, styles: UiStyles<'_>) -> (String, String, Color) {
    if picker_is_empty(state) {
        return (
            t!("dialog.title_rewind").to_string(),
            format!(
                "{}\n\n{}",
                t!("dialog.rewind_no_messages_inline"),
                t!("dialog.esc_close")
            ),
            styles.accent(),
        );
    }

    let selected = layout::selected_in_bounds(state.selected, state.messages.len());
    let visible = layout::visible_window(
        selected.unwrap_or(0),
        state.messages.len(),
        constants::REWIND_MAX_VISIBLE as usize,
    );
    // Reserve column budget for "> " marker (2 cols), " (relative_time)"
    // suffix, and ratatui modal chrome (border + side padding ≈ 6 cols).
    // paddingRight=10 is reserved for the marker and time suffix.
    let display_budget = constants::REWIND_DISPLAY_WIDTH_BUDGET;
    let items: Vec<String> = state.messages[visible.clone()]
        .iter()
        .enumerate()
        .map(|(offset, msg)| {
            let global_idx = visible.start + offset;
            let marker = if Some(global_idx) == selected {
                ">"
            } else {
                " "
            };
            if msg.is_current_prompt {
                return format!("{marker} {}", msg.display_text);
            }

            let suffix_width = layout::text_width(&msg.relative_time) + 3; // " ()" chrome
            let text_width = display_budget.saturating_sub(suffix_width);
            let truncated = layout::truncate_to_width(&msg.display_text, text_width);
            let mut line = format!("{marker} {truncated} ({})", msg.relative_time);
            if state.file_history_enabled
                && let Some(stats_line) = row_diff_stats_line(msg)
            {
                line.push_str(&format!("\n    {stats_line}"));
            }
            line
        })
        .collect();

    let scroll_hint = if state.messages.len() > visible.len() {
        let selected_row = selected.unwrap_or(0) + 1;
        format!("\n  ({selected_row}/{})", state.messages.len())
    } else {
        String::new()
    };
    let prompt_key = if state.file_history_enabled {
        "dialog.rewind_select_with_files"
    } else {
        "dialog.rewind_select_no_files"
    };

    (
        t!("dialog.title_rewind").to_string(),
        format!(
            "{}\n\n{}{scroll_hint}\n\n{}",
            t!(prompt_key),
            items.join("\n"),
            t!("dialog.hints_nav_select_cancel")
        ),
        styles.accent(),
    )
}

fn picker_is_empty(state: &RewindState) -> bool {
    !state.messages.iter().any(|msg| !msg.is_current_prompt)
}

fn row_diff_stats_line(msg: &RewindableMessage) -> Option<String> {
    // `RewindRowMetadataReady` always sets `(diff_stats, can_restore_code)`
    // as a typed pair: `(Some(stats), Some(true))` for restorable rows
    // or `(None, Some(false))` for "no snapshot". Anything else is the
    // pre-load state — render nothing.
    let stats = match (&msg.diff_stats, msg.can_restore_code) {
        (Some(stats), Some(true)) => stats,
        (None, Some(false)) => return Some(t!("dialog.rewind_diff_no_restore").to_string()),
        _ => return None,
    };
    if stats.file_paths.is_empty() {
        return Some(t!("dialog.rewind_diff_no_changes").to_string());
    }

    let prefix = match (stats.files_changed(), stats.file_paths.first()) {
        (1, Some(path)) => {
            t!("dialog.rewind_diff_file_changed_one", file = basename(path)).to_string()
        }
        _ => t!(
            "dialog.rewind_diff_files_changed_many",
            count = stats.files_changed()
        )
        .to_string(),
    };
    let stats_text = t!(
        "dialog.rewind_diff_stats_short",
        ins = stats.insertions,
        del = stats.deletions
    );
    Some(format!("{prefix} {stats_text}"))
}

fn basename(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    trimmed
        .rsplit_once(['/', '\\'])
        .map(|(_, last)| last.to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn file_label(stats: &DiffStatsPreview) -> Option<String> {
    let bases: Vec<String> = stats.file_paths.iter().map(|path| basename(path)).collect();
    match bases.as_slice() {
        [] => None,
        [a] => Some(a.clone()),
        [a, b] => Some(t!("dialog.rewind_files_two", a = a.as_str(), b = b.as_str()).to_string()),
        [a, ..] => Some(
            t!(
                "dialog.rewind_files_many",
                first = a.as_str(),
                rest = stats.files_changed() - 1
            )
            .to_string(),
        ),
    }
}

fn restore_options(state: &RewindState, styles: UiStyles<'_>) -> (String, String, Color) {
    let selected_option =
        layout::selected_in_bounds(state.option_selected, state.available_options.len());
    let items: Vec<String> = state
        .available_options
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let marker = if Some(i) == selected_option { ">" } else { " " };
            format!("{marker} {}", option.label())
        })
        .collect();

    let message_block = match layout::selected_in_bounds(state.selected, state.messages.len())
        .and_then(|idx| state.messages.get(idx))
    {
        Some(msg) => format!("  │ {}\n  │ ({})", msg.display_text, msg.relative_time),
        None => String::new(),
    };

    let focused = selected_option
        .and_then(|idx| state.available_options.get(idx))
        .cloned()
        .unwrap_or(RestoreType::Nevermind);

    let conv_line = match &focused {
        RestoreType::SummarizeFrom { .. } => t!("dialog.rewind_desc_summarize"),
        RestoreType::SummarizeUpTo { .. } => t!("dialog.rewind_desc_summarize_up_to"),
        RestoreType::Both | RestoreType::ConversationOnly => t!("dialog.rewind_desc_forked"),
        RestoreType::CodeOnly | RestoreType::Nevermind => t!("dialog.rewind_desc_unchanged"),
    };

    let code_line = match &focused {
        RestoreType::SummarizeFrom { .. } | RestoreType::SummarizeUpTo { .. } => String::new(),
        RestoreType::Both | RestoreType::CodeOnly => match &state.diff_stats {
            Some(stats) if !stats.file_paths.is_empty() => {
                let stats_text = t!(
                    "dialog.rewind_diff_stats_short",
                    ins = stats.insertions,
                    del = stats.deletions
                );
                let label = file_label(stats).unwrap_or_else(|| {
                    t!(
                        "dialog.rewind_diff_files_changed_many",
                        count = stats.files_changed()
                    )
                    .to_string()
                });
                format!(
                    "\n{}",
                    t!(
                        "dialog.rewind_desc_code_will_restore",
                        stats = stats_text,
                        label = label.as_str()
                    )
                )
            }
            _ => format!("\n{}", t!("dialog.rewind_desc_code_no_changes")),
        },
        RestoreType::ConversationOnly | RestoreType::Nevermind => {
            format!("\n{}", t!("dialog.rewind_desc_code_unchanged"))
        }
    };

    let confirm_prompt_key = if state.has_file_changes {
        "dialog.rewind_confirm_prompt"
    } else {
        "dialog.rewind_confirm_prompt_conv"
    };
    let manual_warning = if state.file_history_enabled && state.has_file_changes {
        format!("\n\n{}", t!("dialog.rewind_manual_warning"))
    } else {
        String::new()
    };

    (
        t!("dialog.title_rewind").to_string(),
        format!(
            "{}\n\n{message_block}\n\n{}\n\n{conv_line}{code_line}{manual_warning}\n\n{}",
            t!(confirm_prompt_key),
            items.join("\n"),
            t!("dialog.hints_nav_confirm_back")
        ),
        styles.accent(),
    )
}

#[cfg(test)]
#[path = "rewind.test.rs"]
mod tests;
