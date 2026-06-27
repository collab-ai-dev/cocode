//! Filterable-list picker state content builders.

use ratatui::prelude::*;

use crate::presentation::model_picker;
use crate::presentation::permissions_editor;
use crate::presentation::picker;
use crate::state::AgentsDialogState;
use crate::state::McpServerSelectState;
use crate::state::ModelPickerState;
use crate::state::PermissionsEditorState;
use crate::state::PluginDialogState;
use crate::state::SkillsDialogState;
use crate::state::SubagentInstance;
use coco_tui_ui::style::UiStyles;

pub(super) fn model_picker_content(
    m: &ModelPickerState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    model_picker::content(m, styles)
}

pub(super) fn skills_dialog_content(
    s: &SkillsDialogState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    picker::skills_dialog_content(s, styles)
}

pub(super) fn plugin_dialog_content(
    p: &PluginDialogState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    picker::plugin_dialog_content(p, styles)
}

pub(super) fn agents_dialog_content(
    a: &AgentsDialogState,
    subagents: &[SubagentInstance],
    styles: UiStyles<'_>,
    now_ms: i64,
) -> (String, String, Color) {
    picker::agents_dialog_content(a, subagents, styles, now_ms)
}

pub(super) fn permissions_editor_content(
    p: &PermissionsEditorState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    permissions_editor::permissions_editor_content(p, styles)
}

pub(super) fn mcp_server_select_content(
    ms: &McpServerSelectState,
    styles: UiStyles<'_>,
) -> (String, String, Color) {
    picker::mcp_server_select_content(ms, styles)
}
