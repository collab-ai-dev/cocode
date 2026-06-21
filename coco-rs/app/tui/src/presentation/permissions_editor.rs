//! Render the `/permissions` rule-editor overlay (Allow / Ask / Deny /
//! Workspace) plus its inline add form and delete confirmation.
//!
//! This module owns ONLY view-string composition; cursor / form mutation
//! stays in `update/permissions_editor.rs` and state shapes in
//! `state/permissions_editor.rs`.

use ratatui::style::Color;

use crate::i18n::t;
use crate::state::AddForm;
use crate::state::AddStep;
use crate::state::DeleteConfirm;
use crate::state::DeleteTarget;
use crate::state::EditorDestination;
use crate::state::PermissionsEditorState;
use crate::state::PermissionsEditorTab;
use crate::state::WizardTextField;
use crate::state::permissions_editor::short_source_label;
use coco_tui_ui::style::UiStyles;

/// Caret glyph between the before / after halves of the active input —
/// matches the agents-dialog wizard for visual consistency.
const CARET_GLYPH: char = '▏';

/// Render the `/permissions` editor. Returns `(title, body, border)`.
pub(crate) fn permissions_editor_content(
    s: &PermissionsEditorState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let title = t!("dialog.title_permissions").to_string();

    let mut body = String::new();
    body.push_str(&render_tab_strip(s.selected_tab));
    body.push('\n');

    if s.managed_only {
        body.push('\n');
        body.push_str(&t!("dialog.perms_managed_banner"));
        body.push('\n');
    }

    // Form / confirm states replace the list; the tab strip stays visible
    // so the user knows they're still inside the overlay.
    if let Some(form) = &s.add_form {
        body.push('\n');
        body.push_str(&render_add_form(s.selected_tab, form));
    } else if let Some(confirm) = &s.delete_confirm {
        body.push('\n');
        body.push_str(&render_delete_confirm(confirm));
    } else {
        body.push('\n');
        body.push_str(&render_list(s));
    }

    body.push_str("\n\n");
    body.push_str(&render_hint(s));

    (title, body, styles.primary())
}

fn render_tab_strip(focused: PermissionsEditorTab) -> String {
    PermissionsEditorTab::ORDER
        .iter()
        .map(|tab| {
            let label = tab_label(*tab);
            if *tab == focused {
                format!("[{label}]")
            } else {
                format!(" {label} ")
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn tab_label(tab: PermissionsEditorTab) -> String {
    match tab {
        PermissionsEditorTab::Allow => t!("dialog.perms_tab_allow").to_string(),
        PermissionsEditorTab::Ask => t!("dialog.perms_tab_ask").to_string(),
        PermissionsEditorTab::Deny => t!("dialog.perms_tab_deny").to_string(),
        PermissionsEditorTab::Workspace => t!("dialog.perms_tab_workspace").to_string(),
    }
}

fn render_list(s: &PermissionsEditorState) -> String {
    let mut out = String::new();
    out.push_str(&tab_subtitle(s.selected_tab));
    out.push_str("\n\n");

    let cursor = s.active_cursor();
    let mut row = 0usize;

    // Row 0 — the "Add…" sentinel.
    let add_label = match s.selected_tab {
        PermissionsEditorTab::Workspace => t!("dialog.perms_add_dir").to_string(),
        _ => t!("dialog.perms_add_rule").to_string(),
    };
    out.push_str(&format!("{} {add_label}\n", marker(row == cursor)));
    row += 1;

    match s.selected_tab.behavior() {
        Some(behavior) => {
            let rules = s.rules_for(behavior);
            if rules.is_empty() {
                out.push_str(&format!("  {}\n", t!("dialog.perms_empty_rules")));
            }
            for rule in rules {
                let focused = row == cursor;
                let source = short_source_label(rule.source);
                let readonly = if rule.is_editable() {
                    String::new()
                } else {
                    format!("  ·  {}", t!("dialog.perms_readonly"))
                };
                out.push_str(&format!(
                    "{} {}  ·  {source}{readonly}\n",
                    marker(focused),
                    rule.display(),
                ));
                row += 1;
            }
        }
        None => {
            for dir in &s.directories {
                let focused = row == cursor;
                let tag = if dir.is_cwd {
                    t!("dialog.perms_cwd_label").to_string()
                } else {
                    short_source_label(dir.source).to_string()
                };
                let readonly = if dir.is_editable() {
                    String::new()
                } else {
                    format!("  ·  {}", t!("dialog.perms_readonly"))
                };
                out.push_str(&format!(
                    "{} {}  ·  {tag}{readonly}\n",
                    marker(focused),
                    dir.path,
                ));
                row += 1;
            }
        }
    }
    out
}

fn tab_subtitle(tab: PermissionsEditorTab) -> String {
    match tab {
        PermissionsEditorTab::Allow => t!("dialog.perms_sub_allow").to_string(),
        PermissionsEditorTab::Ask => t!("dialog.perms_sub_ask").to_string(),
        PermissionsEditorTab::Deny => t!("dialog.perms_sub_deny").to_string(),
        PermissionsEditorTab::Workspace => t!("dialog.perms_sub_workspace").to_string(),
    }
}

fn render_add_form(tab: PermissionsEditorTab, form: &AddForm) -> String {
    let mut out = String::new();
    match form.step {
        AddStep::Input => {
            let prompt = match tab {
                PermissionsEditorTab::Workspace => t!("dialog.perms_input_dir_prompt").to_string(),
                _ => t!("dialog.perms_input_rule_prompt").to_string(),
            };
            out.push_str(&prompt);
            out.push('\n');
            out.push_str(&format!("  > {}\n", caret_render(&form.input)));
            out.push('\n');
            out.push_str(&t!("dialog.perms_input_help"));
        }
        AddStep::Destination => {
            out.push_str(&t!("dialog.perms_dest_prompt"));
            out.push_str("\n\n");
            let selected = form.selected_destination();
            for dest in EditorDestination::ORDER {
                let radio = if dest == selected { "◉" } else { "○" };
                let focus = if dest == selected { "▸" } else { " " };
                out.push_str(&format!("{focus} {radio} {}\n", destination_label(dest)));
                out.push_str(&format!("      {}\n", destination_desc(dest)));
            }
        }
    }
    if let Some(err) = &form.error {
        out.push_str("\n  ");
        out.push_str(&render_error(*err));
    }
    out
}

fn destination_label(dest: EditorDestination) -> String {
    match dest {
        EditorDestination::Local => t!("dialog.perms_dest_local").to_string(),
        EditorDestination::Project => t!("dialog.perms_dest_project").to_string(),
        EditorDestination::User => t!("dialog.perms_dest_user").to_string(),
    }
}

fn destination_desc(dest: EditorDestination) -> String {
    match dest {
        EditorDestination::Local => t!("dialog.perms_dest_local_desc").to_string(),
        EditorDestination::Project => t!("dialog.perms_dest_project_desc").to_string(),
        EditorDestination::User => t!("dialog.perms_dest_user_desc").to_string(),
    }
}

fn render_error(err: crate::state::PermEditorError) -> String {
    match err {
        crate::state::PermEditorError::EmptyInput => t!("dialog.perms_err_empty").to_string(),
    }
}

fn render_delete_confirm(confirm: &DeleteConfirm) -> String {
    let mut out = String::new();
    let (prompt, target) = match &confirm.target {
        DeleteTarget::Rule(rule) => (
            t!("dialog.perms_delete_rule_prompt").to_string(),
            rule.display(),
        ),
        DeleteTarget::Dir(dir) => (
            t!("dialog.perms_delete_dir_prompt").to_string(),
            dir.path.clone(),
        ),
    };
    out.push_str(&prompt);
    out.push_str("\n\n");
    out.push_str(&format!("  {target}\n\n"));
    out.push_str(&format!(
        "  {} {}\n",
        marker(confirm.yes),
        t!("dialog.perms_confirm_yes")
    ));
    out.push_str(&format!(
        "  {} {}\n",
        marker(!confirm.yes),
        t!("dialog.perms_confirm_no")
    ));
    out
}

fn render_hint(s: &PermissionsEditorState) -> String {
    if let Some(form) = &s.add_form {
        match form.step {
            AddStep::Input => t!("dialog.perms_hint_input").to_string(),
            AddStep::Destination => t!("dialog.perms_hint_dest").to_string(),
        }
    } else if s.delete_confirm.is_some() {
        t!("dialog.perms_hint_confirm").to_string()
    } else {
        t!("dialog.perms_hint_list").to_string()
    }
}

fn marker(focused: bool) -> &'static str {
    if focused { "▸" } else { " " }
}

fn caret_render(field: &WizardTextField) -> String {
    let (before, after) = field.split_at_cursor();
    format!("{before}{CARET_GLYPH}{after}")
}

#[cfg(test)]
#[path = "permissions_editor.test.rs"]
mod tests;
