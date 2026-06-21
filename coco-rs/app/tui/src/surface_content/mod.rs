//! Surface content builders — one file per category to stay under the 800-LoC
//! module-size guideline.
//!
//! Each submodule produces `(title, body, border_color)` for the caller to
//! wrap in a `Paragraph` with a `Block` border.

mod confirm;
mod diff;
mod help;
mod permission;
mod pickers;
mod rewind;
mod settings;

use ratatui::prelude::*;

use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::PanePromptState;
use crate::state::PermissionDetail;
use coco_tui_ui::style::UiStyles;

pub(crate) enum TextSurfaceContent<'a> {
    Permission(&'a crate::state::PermissionPromptState),
    Help,
    Error(&'a str),
    PlanEntry(&'a crate::state::PlanEntryPromptState),
    CostWarning(&'a crate::state::CostWarningPromptState),
    ModelPicker(&'a crate::state::ModelPickerState),
    SandboxPermission(&'a crate::state::SandboxPermissionPromptState),
    DiffView(&'a crate::state::DiffViewState),
    McpServerApproval(&'a crate::state::McpServerApprovalPromptState),
    WorktreeExit(&'a crate::state::WorktreeExitState),
    Doctor(&'a crate::state::DoctorState),
    Bridge(&'a crate::state::BridgeState),
    InvalidConfig(&'a crate::state::InvalidConfigState),
    IdleReturn(&'a crate::state::IdleReturnState),
    Trust(&'a crate::state::TrustState),
    BypassPermissions(&'a crate::state::BypassPermissionsState),
    TaskDetail(&'a crate::state::TaskDetailState),
    Feedback(&'a crate::state::FeedbackState),
    PluginHint(&'a crate::state::PluginHintState),
    McpServerSelect(&'a crate::state::McpServerSelectState),
    Rewind(&'a crate::state::RewindState),
    Settings(&'a crate::widgets::settings_panel::SettingsPanelState),
    PlanApproval(&'a crate::state::PlanApprovalPromptState),
    SkillsDialog(&'a crate::state::SkillsDialogState),
    PluginDialog(&'a crate::state::PluginDialogState),
    AgentsDialog(&'a crate::state::AgentsDialogState),
    PermissionsEditor(&'a crate::state::PermissionsEditorState),
    AddDirectory(&'a crate::state::AddDirectoryState),
}

pub(crate) fn prompt_text_surface(prompt: &PanePromptState) -> Option<TextSurfaceContent<'_>> {
    Some(match prompt {
        PanePromptState::Permission(p)
            if matches!(p.detail, PermissionDetail::ExitPlanMode { .. }) =>
        {
            return None;
        }
        PanePromptState::Permission(p) => TextSurfaceContent::Permission(p),
        // Question renders through the dedicated area-based `QuestionWidget`
        // (see `render_interaction_prompt`), not this flat text-surface path.
        PanePromptState::Question(_) => return None,
        PanePromptState::SandboxPermission(s) => TextSurfaceContent::SandboxPermission(s),
        PanePromptState::CostWarning(c) => TextSurfaceContent::CostWarning(c),
        PanePromptState::PlanEntry(p) => TextSurfaceContent::PlanEntry(p),
        PanePromptState::PlanApproval(p) => TextSurfaceContent::PlanApproval(p),
        PanePromptState::McpServerApproval(m) => TextSurfaceContent::McpServerApproval(m),
    })
}

pub(crate) fn modal_text_surface(modal: &ModalState) -> Option<TextSurfaceContent<'_>> {
    Some(match modal {
        ModalState::Help => TextSurfaceContent::Help,
        ModalState::Error(msg) => TextSurfaceContent::Error(msg),
        ModalState::ModelPicker(m) => TextSurfaceContent::ModelPicker(m),
        ModalState::SessionBrowser(_) => return None,
        ModalState::GlobalSearch(_) => return None,
        ModalState::QuickOpen(_) => return None,
        // Export migrated to the styled `render_select_list` path.
        ModalState::Export(_) => return None,
        ModalState::DiffView(d) => TextSurfaceContent::DiffView(d),
        ModalState::Rewind(r) => TextSurfaceContent::Rewind(r),
        ModalState::Settings(s) => TextSurfaceContent::Settings(s),
        ModalState::MemoryDialog(_) => return None,
        ModalState::SkillsDialog(s) => TextSurfaceContent::SkillsDialog(s),
        ModalState::PluginDialog(p) => TextSurfaceContent::PluginDialog(p),
        ModalState::AgentsDialog(a) => TextSurfaceContent::AgentsDialog(a),
        ModalState::PermissionsEditor(p) => TextSurfaceContent::PermissionsEditor(p),
        ModalState::AddDirectory(a) => TextSurfaceContent::AddDirectory(a),
        ModalState::Doctor(d) => TextSurfaceContent::Doctor(d),
        ModalState::WorktreeExit(w) => TextSurfaceContent::WorktreeExit(w),
        ModalState::Bridge(b) => TextSurfaceContent::Bridge(b),
        ModalState::InvalidConfig(ic) => TextSurfaceContent::InvalidConfig(ic),
        ModalState::IdleReturn(ir) => TextSurfaceContent::IdleReturn(ir),
        ModalState::Trust(tr) => TextSurfaceContent::Trust(tr),
        ModalState::BypassPermissions(bp) => TextSurfaceContent::BypassPermissions(bp),
        ModalState::TaskDetail(td) => TextSurfaceContent::TaskDetail(td),
        // Styled render path (see `picker_styled::background_tasks_lines`).
        ModalState::BackgroundTasks(_) => return None,
        ModalState::Feedback(f) => TextSurfaceContent::Feedback(f),
        ModalState::McpServerSelect(ms) => TextSurfaceContent::McpServerSelect(ms),
        ModalState::PluginHint(ph) => TextSurfaceContent::PluginHint(ph),
        ModalState::CopyPicker(_) => return None,
        ModalState::Transcript(_) => return None,
        // Styled render path (see `surface/modal.rs`) — not a text surface.
        ModalState::ThemePicker(_) => return None,
        ModalState::TeamRoster(_) => return None,
    })
}

/// Styled body for the list dialogs migrated onto `render_select_list`.
/// Returns `(title, body lines, border)`; `None` falls through to the
/// monochrome text-surface path. `inner_width` is the usable content width
/// (box minus border + padding).
pub(crate) fn modal_styled_surface(
    modal: &ModalState,
    state: &AppState,
    styles: UiStyles<'_>,
    list_budget: usize,
) -> Option<(String, Vec<Line<'static>>, Color)> {
    use crate::presentation::picker_styled as ps;
    Some(match modal {
        ModalState::Export(e) => ps::export_lines(e, styles, list_budget),
        ModalState::MemoryDialog(m) => ps::memory_dialog_lines(m, styles, list_budget),
        ModalState::QuickOpen(q) => ps::quick_open_lines(q, styles, list_budget),
        ModalState::SessionBrowser(s) => ps::session_browser_lines(s, styles, list_budget),
        ModalState::GlobalSearch(g) => ps::global_search_lines(g, styles, list_budget),
        ModalState::CopyPicker(cp) => ps::copy_picker_lines(cp, styles, list_budget),
        ModalState::TeamRoster(r) => ps::team_roster_lines(r, styles, list_budget),
        ModalState::BackgroundTasks(bt) => {
            ps::background_tasks_lines(bt, state, styles, list_budget)
        }
        _ => return None,
    })
}

/// Produce (title, body, border_color) for text surfaces.
pub(crate) fn surface_content(
    content: TextSurfaceContent<'_>,
    state: &AppState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    match content {
        TextSurfaceContent::Permission(p) => {
            permission::permission_content(p, state.session.permission_mode, styles)
        }
        TextSurfaceContent::Help => help::help_content(state, styles),
        TextSurfaceContent::Error(msg) => (
            t!("dialog.title_error").to_string(),
            msg.to_string(),
            styles.error(),
        ),
        TextSurfaceContent::PlanEntry(p) => confirm::plan_entry_content(p, styles),
        TextSurfaceContent::CostWarning(c) => confirm::cost_warning_content(c, styles),
        TextSurfaceContent::ModelPicker(m) => pickers::model_picker_content(m, styles),
        TextSurfaceContent::SandboxPermission(s) => confirm::sandbox_content(s, styles),
        TextSurfaceContent::DiffView(d) => diff::diff_view_content(d, styles),
        TextSurfaceContent::McpServerApproval(m) => confirm::mcp_server_approval_content(m, styles),
        TextSurfaceContent::WorktreeExit(w) => confirm::worktree_exit_content(w, styles),
        TextSurfaceContent::Doctor(d) => confirm::doctor_content(d, styles),
        TextSurfaceContent::Bridge(b) => confirm::bridge_content(b, styles),
        TextSurfaceContent::InvalidConfig(ic) => confirm::invalid_config_content(ic, styles),
        TextSurfaceContent::IdleReturn(ir) => confirm::idle_return_content(ir, styles),
        TextSurfaceContent::Trust(tr) => confirm::trust_content(tr, styles),
        TextSurfaceContent::BypassPermissions(bp) => {
            confirm::bypass_permissions_content(bp, styles)
        }
        TextSurfaceContent::TaskDetail(td) => confirm::task_detail_content(td, styles),
        TextSurfaceContent::Feedback(f) => confirm::feedback_content(f, styles),
        TextSurfaceContent::PluginHint(ph) => confirm::plugin_hint_content(ph, styles),
        TextSurfaceContent::McpServerSelect(ms) => pickers::mcp_server_select_content(ms, styles),
        TextSurfaceContent::Rewind(r) => rewind::rewind_surface_content(r, styles),
        TextSurfaceContent::Settings(s) => settings::settings_surface_content(s, styles),
        TextSurfaceContent::PlanApproval(p) => confirm::plan_approval_content(p, styles),
        TextSurfaceContent::SkillsDialog(s) => pickers::skills_dialog_content(s, styles),
        TextSurfaceContent::PluginDialog(p) => pickers::plugin_dialog_content(p, styles),
        TextSurfaceContent::AgentsDialog(a) => pickers::agents_dialog_content(
            a,
            &state.session.subagents,
            styles,
            state.clock.now_ms(),
        ),
        TextSurfaceContent::PermissionsEditor(p) => pickers::permissions_editor_content(p, styles),
        TextSurfaceContent::AddDirectory(a) => {
            crate::presentation::add_directory::add_directory_content(a, styles)
        }
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
