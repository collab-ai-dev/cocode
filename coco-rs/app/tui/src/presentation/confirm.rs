//! Confirmation dialog renderers — small, mostly one-screen text surfaces.
//!
//! Covers: cost warning, plan exit/entry, sandbox permission, MCP server
//! approval, worktree exit, doctor, bridge, invalid config, idle return,
//! trust, auto-mode opt-in, bypass permissions, task detail, and feedback.
//! Kept together because each is short.

use ratatui::prelude::Color;

use crate::i18n::t;
use crate::presentation::pager;
use crate::state::BridgeState;
use crate::state::BypassPermissionsState;
use crate::state::CostWarningPromptState;
use crate::state::DoctorState;
use crate::state::FeedbackState;
use crate::state::IdleReturnState;
use crate::state::InvalidConfigState;
use crate::state::McpServerApprovalPromptState;
use crate::state::PlanApprovalPromptState;
use crate::state::PlanEntryPromptState;
use crate::state::PluginHintState;
use crate::state::SandboxPermissionPromptState;
use crate::state::TaskDetailState;
use crate::state::TrustState;
use crate::state::WorktreeExitState;
use coco_tui_ui::style::UiStyles;

pub(crate) fn cost_warning_content(
    c: &CostWarningPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    (
        t!("dialog.title_cost").to_string(),
        format!(
            "{}\n{}\n\n{}",
            t!(
                "dialog.cost_current",
                amount = format!("{:.2}", c.current_cost_cents as f64 / 100.0)
            ),
            t!(
                "dialog.cost_threshold",
                amount = format!("{:.2}", c.threshold_cents as f64 / 100.0)
            ),
            t!("dialog.cost_continue"),
        ),
        styles.warning(),
    )
}

pub(crate) fn plan_entry_content(
    p: &PlanEntryPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    (
        t!("dialog.title_enter_plan").to_string(),
        format!("{}\n\n{}", p.description, t!("dialog.confirm_yn")),
        styles.plan(),
    )
}

pub(crate) fn sandbox_content(
    s: &SandboxPermissionPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    (
        t!("dialog.title_sandbox").to_string(),
        format!("{}\n\n{}", s.description, t!("dialog.allow_yn")),
        styles.error(),
    )
}

pub(crate) fn mcp_server_approval_content(
    m: &McpServerApprovalPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    (
        t!("dialog.title_mcp_server").to_string(),
        format!(
            "{}\n{}\n{}\n\n{}",
            t!("dialog.server_prefix", name = m.server_name.as_str()),
            m.server_url.as_deref().unwrap_or(""),
            t!("dialog.tools_prefix", list = m.tools.join(", ")),
            t!("dialog.actions_approve_deny"),
        ),
        styles.accent(),
    )
}

pub(crate) fn worktree_exit_content(
    w: &WorktreeExitState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let files = if w.changed_files.is_empty() {
        t!("dialog.no_uncommitted_changes").to_string()
    } else {
        w.changed_files
            .iter()
            .map(|f| format!("  {f}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    (
        t!("dialog.title_exit_worktree").to_string(),
        format!(
            "{}\n\n{files}\n\n{}",
            t!("dialog.branch_prefix", name = w.branch.as_str()),
            t!("dialog.yn_exit_stay"),
        ),
        styles.warning(),
    )
}

pub(crate) fn doctor_content(d: &DoctorState, styles: UiStyles<'_>) -> (String, String, Color) {
    let checks: Vec<String> = d
        .checks
        .iter()
        .map(|c| {
            let icon = if c.passed { "✓" } else { "✗" };
            format!("  {icon} {}: {}", c.name, c.message)
        })
        .collect();
    let body = if checks.is_empty() {
        format!(
            "{}\n\n{}",
            t!("dialog.running_diagnostics"),
            t!("dialog.esc_close")
        )
    } else {
        format!("{}\n\n{}", checks.join("\n"), t!("dialog.esc_close"))
    };
    (
        t!("dialog.title_doctor").to_string(),
        body,
        styles.primary(),
    )
}

pub(crate) fn bridge_content(b: &BridgeState, styles: UiStyles<'_>) -> (String, String, Color) {
    (
        t!("dialog.title_bridge", bridge_type = b.bridge_type.as_str()).to_string(),
        format!(
            "{}\n\n{}\n\n{}",
            t!("dialog.status_prefix", status = b.status.as_str()),
            b.details,
            t!("dialog.esc_close")
        ),
        styles.accent(),
    )
}

pub(crate) fn invalid_config_content(
    ic: &InvalidConfigState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let errors = ic
        .errors
        .iter()
        .map(|e| format!("  • {e}"))
        .collect::<Vec<_>>()
        .join("\n");
    (
        t!("dialog.title_invalid_config").to_string(),
        format!(
            "{}\n\n{errors}\n\n{}",
            t!("dialog.config_errors"),
            t!("dialog.hints_invalid_config"),
        ),
        styles.error(),
    )
}

pub(crate) fn idle_return_content(
    ir: &IdleReturnState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let mins = ir.idle_duration_secs / 60;
    (
        t!("dialog.title_welcome_back").to_string(),
        format!(
            "{}\n\n{}",
            t!("dialog.welcome_back_body", mins = mins),
            t!("dialog.enter_continue")
        ),
        styles.primary(),
    )
}

pub(crate) fn trust_content(tr: &TrustState, styles: UiStyles<'_>) -> (String, String, Color) {
    (
        t!("dialog.title_trust").to_string(),
        format!(
            "{}\n\n  {}\n\n{}\n\n{}",
            t!("dialog.trust_prompt"),
            tr.path,
            tr.description,
            t!("dialog.yn_trust_deny"),
        ),
        styles.warning(),
    )
}

pub(crate) fn bypass_permissions_content(
    bp: &BypassPermissionsState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    (
        t!("dialog.title_bypass_permissions").to_string(),
        format!(
            "{}\n\n{}",
            t!("dialog.bypass_body", mode = bp.current_mode.as_str()),
            t!("dialog.yn_enable_cancel"),
        ),
        styles.error(),
    )
}

pub(crate) fn task_detail_content(
    td: &TaskDetailState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    let output_lines: Vec<&str> = td.output.lines().collect();
    let window = pager::pager_window(output_lines.len(), td.scroll, 20);
    let visible: String = output_lines
        .get(window.range())
        .unwrap_or_default()
        .join("\n");
    let position = window.position_suffix();
    let title = if position.is_empty() {
        t!("dialog.title_task", name = td.task_type.as_str()).to_string()
    } else {
        format!(
            "{}{position} ",
            t!("dialog.title_task", name = td.task_type.as_str())
                .to_string()
                .trim_end()
        )
    };
    (
        title,
        format!(
            "{}\n{}\n\n{visible}\n\n{}",
            td.description,
            t!("dialog.status_prefix", status = td.status.as_str()),
            t!("dialog.scroll_close_hints"),
        ),
        styles.primary(),
    )
}

pub(crate) fn plan_approval_content(
    p: &PlanApprovalPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    // Cap the plan preview so the state stays readable when the plan
    // body is very long. Full content is still on disk at
    // `p.plan_file_path` for the leader to inspect outside the state.
    const MAX_PREVIEW_LINES: usize = 18;
    let preview: String = p
        .plan_content
        .lines()
        .take(MAX_PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let truncated = p.plan_content.lines().count() > MAX_PREVIEW_LINES;
    let preview_block = if truncated {
        format!("{preview}\n… {}", t!("dialog.plan_approval_truncated"))
    } else {
        preview
    };

    let approve_marker = if p.is_approve_focused() { "▸ " } else { "  " };
    let deny_marker = if p.is_approve_focused() { "  " } else { "▸ " };
    let buttons = format!(
        "{approve_marker}{}    {deny_marker}{}",
        t!("dialog.plan_approval_approve"),
        t!("dialog.plan_approval_deny"),
    );

    let path_line = p
        .plan_file_path
        .as_deref()
        .map(|p| format!("{}\n\n", t!("dialog.plan_approval_file", path = p)))
        .unwrap_or_default();

    (
        t!("dialog.plan_approval_title", from = p.from.as_str()).to_string(),
        format!(
            "{path_line}{preview_block}\n\n{buttons}\n\n{}",
            t!("dialog.plan_approval_hints")
        ),
        styles.plan(),
    )
}

pub(crate) fn plugin_hint_content(
    ph: &PluginHintState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    // A short body describing the plugin plus a 3-option select
    // (install / dismiss / disable-all).
    let options = [
        t!("dialog.plugin_hint_install", name = ph.plugin_name.as_str()).to_string(),
        t!("dialog.plugin_hint_no").to_string(),
        t!("dialog.plugin_hint_disable").to_string(),
    ];
    let items: Vec<String> = options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let marker = if i as i32 == ph.selected {
                "▸ "
            } else {
                "  "
            };
            format!("{marker}{opt}")
        })
        .collect();

    let mut body = String::new();
    body.push_str(&t!(
        "dialog.plugin_hint_suggests",
        command = ph.source_command.as_str()
    ));
    body.push_str("\n\n");
    body.push_str(&t!(
        "dialog.plugin_hint_plugin",
        name = ph.plugin_name.as_str()
    ));
    body.push('\n');
    body.push_str(&t!(
        "dialog.plugin_hint_marketplace",
        marketplace = ph.marketplace_name.as_str()
    ));
    if let Some(desc) = &ph.plugin_description {
        body.push('\n');
        body.push_str(desc);
    }
    body.push_str("\n\n");
    body.push_str(&t!("dialog.plugin_hint_prompt"));
    body.push_str("\n\n");
    body.push_str(&items.join("\n"));
    body.push_str("\n\n");
    body.push_str(&t!("dialog.plugin_hint_hints"));

    (
        t!("dialog.title_plugin_hint").to_string(),
        body,
        styles.primary(),
    )
}

pub(crate) fn feedback_content(f: &FeedbackState, styles: UiStyles<'_>) -> (String, String, Color) {
    let items: Vec<String> = f
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let marker = if i as i32 == f.selected { "▸ " } else { "  " };
            format!("{marker}{opt}")
        })
        .collect();
    (
        t!("dialog.title_feedback").to_string(),
        format!("{}\n\n{}", f.prompt, items.join("\n")),
        styles.primary(),
    )
}

#[cfg(test)]
#[path = "confirm.test.rs"]
mod tests;
