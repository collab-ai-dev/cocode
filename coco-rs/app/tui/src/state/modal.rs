//! Full-screen modal state.

use std::collections::VecDeque;

use crate::state::agents_dialog::AgentsDialogState;
use crate::state::permissions_editor::PermissionsEditorState;
use crate::state::plugin_dialog::PluginDialogState;
use crate::state::skills_dialog::SkillsDialogState;
use crate::state::surface_payloads::BackgroundTasksState;
use crate::state::surface_payloads::CopyPickerState;
use crate::state::surface_payloads::DiffViewState;
use crate::state::surface_payloads::DoctorState;
use crate::state::surface_payloads::ExportState;
use crate::state::surface_payloads::GlobalSearchState;
use crate::state::surface_payloads::MemoryDialogState;
use crate::state::surface_payloads::ModelPickerState;
use crate::state::surface_payloads::QuickOpenState;
use crate::state::surface_payloads::SessionBrowserState;
use crate::state::surface_payloads::TaskDetailState;
use crate::state::surface_payloads::TeamRosterState;
use crate::state::surface_payloads::ThemePickerState;
use crate::state::surface_payloads::{self};

#[derive(Debug, Clone)]
pub enum ModalState {
    Help,
    Error(String),
    ModelPicker(ModelPickerState),
    ThemePicker(ThemePickerState),
    SessionBrowser(SessionBrowserState),
    GlobalSearch(GlobalSearchState),
    QuickOpen(QuickOpenState),
    Export(ExportState),
    DiffView(DiffViewState),
    Rewind(crate::state::rewind::RewindState),
    Settings(crate::widgets::settings_panel::SettingsPanelState),
    MemoryDialog(MemoryDialogState),
    SkillsDialog(SkillsDialogState),
    PluginDialog(PluginDialogState),
    AgentsDialog(AgentsDialogState),
    PermissionsEditor(PermissionsEditorState),
    Transcript(crate::state::transcript::TranscriptState),
    Doctor(DoctorState),
    WorktreeExit(surface_payloads::WorktreeExitState),
    Bridge(surface_payloads::BridgeState),
    InvalidConfig(surface_payloads::InvalidConfigState),
    IdleReturn(surface_payloads::IdleReturnState),
    Trust(surface_payloads::TrustState),
    BypassPermissions(surface_payloads::BypassPermissionsState),
    TaskDetail(TaskDetailState),
    BackgroundTasks(BackgroundTasksState),
    Feedback(surface_payloads::FeedbackState),
    McpServerSelect(surface_payloads::McpServerSelectState),
    CopyPicker(CopyPickerState),
    TeamRoster(TeamRosterState),
    PluginHint(crate::state::plugin_dialog::PluginHintState),
    /// `/add-dir` (no-arg) interactive directory-path input.
    AddDirectory(surface_payloads::AddDirectoryState),
}

impl ModalState {
    pub fn priority(&self) -> i32 {
        match self {
            Self::WorktreeExit(_) | Self::BypassPermissions(_) => 3,
            Self::Error(_) | Self::InvalidConfig(_) => 4,
            Self::Rewind(_) | Self::DiffView(_) => 5,
            Self::Trust(_) | Self::Bridge(_) | Self::McpServerSelect(_) => 6,
            Self::ModelPicker(_)
            | Self::ThemePicker(_)
            | Self::SessionBrowser(_)
            | Self::GlobalSearch(_)
            | Self::QuickOpen(_)
            | Self::Export(_)
            | Self::Feedback(_)
            | Self::TaskDetail(_)
            | Self::BackgroundTasks(_)
            | Self::Doctor(_)
            | Self::Settings(_)
            | Self::Transcript(_)
            | Self::MemoryDialog(_)
            | Self::SkillsDialog(_)
            | Self::PluginDialog(_)
            | Self::AgentsDialog(_)
            | Self::PermissionsEditor(_)
            | Self::CopyPicker(_)
            | Self::TeamRoster(_)
            | Self::PluginHint(_)
            | Self::AddDirectory(_)
            | Self::IdleReturn(_) => 7,
            Self::Help => 8,
        }
    }

    pub fn requires_fullscreen_isolation(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModalQueue {
    inner: VecDeque<ModalState>,
}

impl ModalQueue {
    pub fn push(&mut self, modal: ModalState) {
        let prio = modal.priority();
        let pos = self
            .inner
            .iter()
            .position(|queued| queued.priority() > prio)
            .unwrap_or(self.inner.len());
        self.inner.insert(pos, modal);
    }

    pub fn pop_front(&mut self) -> Option<ModalState> {
        self.inner.pop_front()
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}
