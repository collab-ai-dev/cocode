//! Render the `/agents` 2-tab overlay (Running + Library) and its
//! inline 4-step create wizard.
//!
//! The wizard steps through name → description → source → confirm;
//! tools / model / memory default in the template and live in `$EDITOR`.
//!
//! This module owns ONLY view-string composition; cursor / step
//! mutation stays in `update/agents_dialog.rs` and state shapes in
//! `state/agents_dialog.rs`.

use ratatui::style::Color;

use crate::i18n::t;
use crate::state::AgentsDialogState;
use crate::state::AgentsDialogTab;
use crate::state::CreateWizardState;
use crate::state::CreateWizardStep;
use crate::state::LibraryRow;
use crate::state::SubagentInstance;
use crate::state::SubagentStatus;
use crate::state::WizardError;
use crate::state::WizardSource;
use crate::state::WizardTextField;
use coco_tui_ui::style::UiStyles;

/// Caret glyph rendered between the text-before-cursor and text-
/// after-cursor halves of an active input field. Static, not
/// blinking — ratatui doesn't redraw on its own without animation
/// scheduling, and a stable caret reads more like a form field.
const CARET_GLYPH: char = '▏';

/// Render the `/agents` dialog. Title is always `"Agents"`; body
/// includes the tab strip (`Running` / `Library`) followed by the
/// content of the focused tab. Tab title shows live count for
/// Running (`Running (N)` when N > 0).
pub(crate) fn agents_dialog_content(
    s: &AgentsDialogState,
    subagents: &[SubagentInstance],
    styles: UiStyles<'_>,
    now_ms: i64,
) -> (String, String, Color) {
    let title = t!("dialog.title_agents").to_string();
    let running_count = subagents
        .iter()
        .filter(|s| s.status == SubagentStatus::Running)
        .count();

    let mut body = String::new();
    body.push_str(&render_tab_strip(s.selected_tab, running_count));
    body.push('\n');

    // Wizard mode replaces the Library list with the inline form.
    // Tabs remain visible so the user sees they're still inside the
    // /agents overlay; switching tabs while the wizard is active is
    // blocked by the dispatch layer.
    let tab_body = match (s.selected_tab, s.wizard.as_ref()) {
        (AgentsDialogTab::Library, Some(wizard)) => render_create_wizard(wizard),
        (AgentsDialogTab::Running, _) => render_running_tab(s, subagents, now_ms),
        (AgentsDialogTab::Library, None) => render_library_tab(s),
    };
    body.push_str(&tab_body);

    body.push_str("\n\n");
    let hint = if let Some(path) = s.pending_delete.as_ref() {
        let name = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("this agent");
        t!("dialog.agents_delete_confirm", name = name).to_string()
    } else if s.is_in_wizard() {
        t!("dialog.agents_wizard_hint").to_string()
    } else {
        t!("dialog.agents_hint").to_string()
    };
    body.push_str(&hint);

    (title, body, styles.primary())
}

fn render_tab_strip(focused: AgentsDialogTab, running_count: usize) -> String {
    let running_label = format_tab_label(
        t!("dialog.agents_tab_running").as_ref(),
        running_count,
        focused == AgentsDialogTab::Running,
    );
    // Library never carries a count badge — format inline rather than
    // running it through `format_tab_label` with a zero count, which
    // would risk emitting a `Library (0)` once the helper grows.
    let library_label = if focused == AgentsDialogTab::Library {
        format!("[{}]", t!("dialog.agents_tab_library").as_ref())
    } else {
        format!(" {} ", t!("dialog.agents_tab_library").as_ref())
    };
    [running_label, library_label].join("  ")
}

fn format_tab_label(label: &str, count: usize, focused: bool) -> String {
    let body = if count > 0 {
        format!("{label} ({count})")
    } else {
        label.to_string()
    };
    if focused {
        format!("[{body}]")
    } else {
        format!(" {body} ")
    }
}

fn render_running_tab(
    s: &AgentsDialogState,
    subagents: &[SubagentInstance],
    now_ms: i64,
) -> String {
    let active: Vec<&SubagentInstance> = subagents
        .iter()
        .filter(|s| s.status == SubagentStatus::Running)
        .collect();
    if active.is_empty() {
        return t!("dialog.agents_running_empty").to_string();
    }

    let mut out = String::new();
    let cursor = s.running_cursor.min(active.len().saturating_sub(1));
    for (i, sub) in active.iter().enumerate() {
        let marker = if i == cursor { "▸" } else { " " };
        let elapsed = sub
            .started_at_ms
            .map(|started| format_elapsed(now_ms, started))
            .unwrap_or_else(|| "--".to_string());
        let last = sub.last_tool_name.as_deref().unwrap_or("…");
        out.push_str(&format!(
            "{marker} {name}  ·  {elapsed}  ·  {tools} tools  ·  {last}\n",
            name = sub.agent_type,
            tools = sub.tool_count,
        ));
    }

    // Recently-completed section — last 5, newest first, filtering on
    // completed | failed | killed.
    let recent: Vec<&SubagentInstance> = subagents
        .iter()
        .filter(|s| !matches!(s.status, SubagentStatus::Running))
        .rev()
        .take(5)
        .collect();
    if !recent.is_empty() {
        out.push('\n');
        out.push_str(&t!("dialog.agents_running_completed_header"));
        out.push('\n');
        for sub in recent {
            let glyph = match sub.status {
                SubagentStatus::Completed => "✓",
                SubagentStatus::Failed => "✗",
                SubagentStatus::Running => "·", // unreachable
            };
            out.push_str(&format!("  {glyph} {}\n", sub.agent_type));
        }
    }
    out
}

fn render_library_tab(s: &AgentsDialogState) -> String {
    if s.library.is_empty() {
        return t!("dialog.agents_library_empty").to_string();
    }
    let mut out = String::new();
    for (i, row) in s.library.iter().enumerate() {
        let is_focused = i == s.library_cursor && row.is_selectable();
        let marker = if is_focused { "▸" } else { " " };
        match row {
            LibraryRow::CreateNew => {
                out.push_str(&format!("{marker} {}\n", t!("dialog.agents_create_new")));
            }
            LibraryRow::SourceHeader { label } => {
                out.push_str(&format!("\n  ── {label} ──\n"));
            }
            LibraryRow::Agent {
                name,
                description,
                source,
                is_builtin,
                is_overridden,
                running_count,
                ..
            } => {
                // Library row layout: `name · source · suffix · badge`.
                // The source label keeps each row self-explanatory
                // even when group headers scroll off-screen.
                let source_label = LibraryRow::short_source_label(*source);
                let suffix = if *is_overridden {
                    t!("dialog.agents_overridden_suffix").to_string()
                } else if *is_builtin {
                    t!("dialog.agents_builtin_suffix").to_string()
                } else {
                    String::new()
                };
                let badge = if *running_count > 0 {
                    format!("  ·  {running_count} running")
                } else {
                    String::new()
                };
                let desc = description.as_deref().unwrap_or("");
                out.push_str(&format!(
                    "{marker} {name}  ·  {source_label}{suffix}{badge}\n"
                ));
                if !desc.is_empty() {
                    out.push_str(&format!("    {desc}\n"));
                }
            }
        }
    }
    out
}

fn render_create_wizard(w: &CreateWizardState) -> String {
    const TOTAL_STEPS: i32 = 4;
    let mut out = String::new();
    let step_label = match w.step {
        CreateWizardStep::Name => t!("dialog.agents_create_step_name").to_string(),
        CreateWizardStep::Description => t!("dialog.agents_create_step_desc").to_string(),
        CreateWizardStep::Source => t!("dialog.agents_create_step_source").to_string(),
        CreateWizardStep::Confirm => t!("dialog.agents_create_step_confirm").to_string(),
    };
    let step_idx = match w.step {
        CreateWizardStep::Name => 1,
        CreateWizardStep::Description => 2,
        CreateWizardStep::Source => 3,
        CreateWizardStep::Confirm => 4,
    };
    out.push_str(&t!(
        "dialog.agents_create_step_header",
        step = step_idx.to_string().as_str(),
        total = TOTAL_STEPS.to_string().as_str(),
        label = step_label.as_str(),
    ));
    out.push_str("\n\n");

    match w.step {
        CreateWizardStep::Name => {
            out.push_str(&t!("dialog.agents_create_name_prompt"));
            out.push('\n');
            out.push_str(&format!("  > {}\n", caret_render(&w.name)));
            out.push('\n');
            out.push_str(&t!("dialog.agents_create_name_help"));
        }
        CreateWizardStep::Description => {
            out.push_str(&t!("dialog.agents_create_desc_prompt"));
            out.push('\n');
            out.push_str(&format!("  > {}\n", caret_render(&w.description)));
            out.push('\n');
            out.push_str(&t!("dialog.agents_create_desc_help"));
        }
        CreateWizardStep::Source => {
            out.push_str(&t!("dialog.agents_create_source_prompt"));
            out.push('\n');
            let sources = [
                (WizardSource::User, t!("dialog.agents_create_source_user")),
                (
                    WizardSource::Project,
                    t!("dialog.agents_create_source_project"),
                ),
            ];
            for (source, label) in &sources {
                let marker = if *source == w.source { "▸" } else { " " };
                out.push_str(&format!("  {marker} {label}\n", label = label.as_ref()));
            }
        }
        CreateWizardStep::Confirm => {
            // Surface every input the user picked so they can review
            // before the irreversible filesystem write. Pressing
            // Enter dispatches CreateAgent; Esc walks back to Source.
            out.push_str(&t!("dialog.agents_create_confirm_prompt"));
            out.push_str("\n\n");
            let source_label = match w.source {
                WizardSource::User => t!("dialog.agents_create_source_user_short"),
                WizardSource::Project => t!("dialog.agents_create_source_project_short"),
            };
            out.push_str(&t!(
                "dialog.agents_create_confirm_name",
                name = w.name.text.as_str()
            ));
            out.push('\n');
            out.push_str(&t!(
                "dialog.agents_create_confirm_desc",
                desc = w.description.text.as_str()
            ));
            out.push('\n');
            out.push_str(&t!(
                "dialog.agents_create_confirm_source",
                source = source_label.as_ref()
            ));
            out.push('\n');
        }
    }

    if let Some(err) = &w.error {
        out.push_str("\n  ");
        out.push_str(&render_wizard_error(err));
    }

    out
}

/// Render a wizard text field with the caret inserted at the cursor
/// position. Single-line — newlines are filtered out at input.
fn caret_render(field: &WizardTextField) -> String {
    let (before, after) = field.split_at_cursor();
    format!("{before}{CARET_GLYPH}{after}")
}

fn render_wizard_error(err: &WizardError) -> String {
    match err {
        WizardError::NameEmpty => t!("dialog.agents_create_err_name_empty").to_string(),
        WizardError::NameLead => t!("dialog.agents_create_err_name_lead").to_string(),
        WizardError::NameChars => t!("dialog.agents_create_err_name_chars").to_string(),
        WizardError::DescEmpty => t!("dialog.agents_create_err_desc_empty").to_string(),
        WizardError::AlreadyExists { path } => t!(
            "dialog.agents_create_exists",
            path = path.display().to_string().as_str()
        )
        .to_string(),
        WizardError::NonWritableSource => {
            // The wizard restricts source to User / Project so this
            // is unreachable under normal flow. Surface a generic
            // diagnostic so a future regression is visible rather
            // than silent.
            t!("dialog.agents_create_err_non_writable").to_string()
        }
    }
}

fn format_elapsed(now_ms: i64, started_at_ms: i64) -> String {
    let elapsed_secs = ((now_ms - started_at_ms) / 1000).max(0);
    if elapsed_secs < 60 {
        format!("{elapsed_secs}s")
    } else if elapsed_secs < 3600 {
        format!("{}m{:02}s", elapsed_secs / 60, elapsed_secs % 60)
    } else {
        format!("{}h{:02}m", elapsed_secs / 3600, (elapsed_secs % 3600) / 60)
    }
}

#[cfg(test)]
#[path = "agents_dialog.test.rs"]
mod tests;
