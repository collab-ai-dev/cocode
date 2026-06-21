//! Confirmation dialog content builders.

use ratatui::prelude::Color;

use crate::presentation::confirm;
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

pub(super) fn cost_warning_content(
    c: &CostWarningPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::cost_warning_content(c, styles)
}

pub(super) fn plan_entry_content(
    p: &PlanEntryPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::plan_entry_content(p, styles)
}

pub(super) fn sandbox_content(
    s: &SandboxPermissionPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::sandbox_content(s, styles)
}

pub(super) fn mcp_server_approval_content(
    m: &McpServerApprovalPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::mcp_server_approval_content(m, styles)
}

pub(super) fn worktree_exit_content(
    w: &WorktreeExitState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::worktree_exit_content(w, styles)
}

pub(super) fn doctor_content(d: &DoctorState, styles: UiStyles<'_>) -> (String, String, Color) {
    confirm::doctor_content(d, styles)
}

pub(super) fn bridge_content(b: &BridgeState, styles: UiStyles<'_>) -> (String, String, Color) {
    confirm::bridge_content(b, styles)
}

pub(super) fn invalid_config_content(
    ic: &InvalidConfigState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::invalid_config_content(ic, styles)
}

pub(super) fn idle_return_content(
    ir: &IdleReturnState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::idle_return_content(ir, styles)
}

pub(super) fn trust_content(tr: &TrustState, styles: UiStyles<'_>) -> (String, String, Color) {
    confirm::trust_content(tr, styles)
}

pub(super) fn bypass_permissions_content(
    bp: &BypassPermissionsState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::bypass_permissions_content(bp, styles)
}

pub(super) fn task_detail_content(
    td: &TaskDetailState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::task_detail_content(td, styles)
}

pub(super) fn plan_approval_content(
    p: &PlanApprovalPromptState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::plan_approval_content(p, styles)
}

pub(super) fn feedback_content(f: &FeedbackState, styles: UiStyles<'_>) -> (String, String, Color) {
    confirm::feedback_content(f, styles)
}

pub(super) fn plugin_hint_content(
    ph: &PluginHintState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    confirm::plugin_hint_content(ph, styles)
}
