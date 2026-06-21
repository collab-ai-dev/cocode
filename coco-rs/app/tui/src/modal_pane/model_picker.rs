//! Model picker modal — filterable model list with an effort axis
//! (Left/Right) and a role axis (Tab/Shift+Tab).

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::ModelEntry;
use crate::state::ModelPickerState;
use crate::state::ProviderUnavailableReason;
use crate::state::ui::Toast;

pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Home => Some(TuiCommand::SurfaceJumpStart),
        KeyCode::End => Some(TuiCommand::SurfaceJumpEnd),
        KeyCode::Up if shift => Some(TuiCommand::SurfaceJumpStart),
        KeyCode::Down if shift => Some(TuiCommand::SurfaceJumpEnd),
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Left => Some(TuiCommand::ModelPickerCycleEffort(-1)),
        KeyCode::Right => Some(TuiCommand::ModelPickerCycleEffort(1)),
        KeyCode::Tab => Some(TuiCommand::SettingsNextTab),
        KeyCode::BackTab => Some(TuiCommand::SettingsPrevTab),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Backspace => Some(TuiCommand::SurfaceFilterBackspace),
        KeyCode::Char('c') if ctrl => Some(TuiCommand::Cancel),
        KeyCode::Char('p') if ctrl => Some(TuiCommand::SurfacePrev),
        KeyCode::Char('n') if ctrl => Some(TuiCommand::SurfaceNext),
        KeyCode::Char(c) => Some(TuiCommand::SurfaceFilter(c)),
        _ => None,
    }
}

pub(super) async fn confirm(
    state: &mut AppState,
    m: ModelPickerState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    if let Some(entry) = filtered_models(&m).get(m.selected as usize).copied() {
        if let Some(summary) = unavailable_summary(&entry.unavailable_reasons) {
            state.ui.restore_modal(ModalState::ModelPicker(m));
            state.ui.add_toast(Toast::warning(format!(
                "{} {summary}",
                t!("dialog.model_picker_unavailable_label")
            )));
            return;
        }
        let _ = command_tx
            .send(UserCommand::SetModelRole {
                role: m.role,
                provider: entry.provider.clone(),
                model_id: entry.model_id.clone(),
                effort: m.effort,
            })
            .await;
        if matches!(m.role, coco_types::ModelRole::Main) {
            state.session.model = entry.model_id.clone();
        }
    }
    state.ui.finish_taken_modal();
}

pub(crate) fn cycle_effort(state: &mut AppState, delta: i32) {
    let Some(ModalState::ModelPicker(m)) = state.ui.modal.as_mut() else {
        return;
    };
    let next_effort = {
        let filtered = filtered_models(m);
        let Some(entry) = filtered.get(m.selected as usize) else {
            return;
        };
        if !entry.unavailable_reasons.is_empty() || entry.supported_efforts.is_empty() {
            return;
        }
        let current_idx = m
            .effort
            .and_then(|e| entry.supported_efforts.iter().position(|&se| se == e))
            .unwrap_or(0) as i32;
        let n = entry.supported_efforts.len() as i32;
        let next_idx = (current_idx + delta).rem_euclid(n) as usize;
        entry.supported_efforts[next_idx]
    };
    m.effort = Some(next_effort);
}

pub(crate) fn filtered_models(m: &ModelPickerState) -> Vec<&ModelEntry> {
    let filter_lower = m.filter.to_lowercase();
    m.entries
        .iter()
        .filter(|e| {
            filter_lower.is_empty()
                || e.display_name.to_lowercase().contains(&filter_lower)
                || e.provider_display.to_lowercase().contains(&filter_lower)
        })
        .collect()
}

fn unavailable_summary(reasons: &[ProviderUnavailableReason]) -> Option<String> {
    if reasons.is_empty() {
        return None;
    }
    Some(
        reasons
            .iter()
            .map(unavailable_reason_label)
            .collect::<Vec<_>>()
            .join("; "),
    )
}

fn unavailable_reason_label(reason: &ProviderUnavailableReason) -> String {
    match reason {
        ProviderUnavailableReason::MissingBaseUrl => {
            t!("dialog.model_picker_unavailable_base_url").to_string()
        }
        ProviderUnavailableReason::MissingApiKey { env_key } => t!(
            "dialog.model_picker_unavailable_api_key",
            env_key = env_key.as_str()
        )
        .to_string(),
        ProviderUnavailableReason::NotLoggedIn { provider } => t!(
            "dialog.model_picker_unavailable_not_logged_in",
            provider = provider.as_str()
        )
        .to_string(),
        ProviderUnavailableReason::NoModels => {
            t!("dialog.model_picker_unavailable_no_models").to_string()
        }
    }
}

#[cfg(test)]
#[path = "model_picker.test.rs"]
mod tests;
