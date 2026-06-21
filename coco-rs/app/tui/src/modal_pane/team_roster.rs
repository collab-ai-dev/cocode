//! Teams roster picker — Up/Down select a teammate, Left/Right cycle the
//! focused teammate's mode, Shift+Left/Right cycle all modes in tandem.

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;

use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::ModalState;

pub(crate) fn map_key(key: KeyEvent) -> Option<TuiCommand> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    match key.code {
        KeyCode::Up => Some(TuiCommand::SurfacePrev),
        KeyCode::Down => Some(TuiCommand::SurfaceNext),
        KeyCode::Left if shift => Some(TuiCommand::TeamRosterCycleAllModes(-1)),
        KeyCode::Right if shift => Some(TuiCommand::TeamRosterCycleAllModes(1)),
        KeyCode::Left => Some(TuiCommand::TeamRosterCycleMode(-1)),
        KeyCode::Right => Some(TuiCommand::TeamRosterCycleMode(1)),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char('c') if ctrl => Some(TuiCommand::Cancel),
        _ => None,
    }
}

const ROSTER_MODE_ORDER: [coco_types::PermissionMode; 4] = [
    coco_types::PermissionMode::Default,
    coco_types::PermissionMode::AcceptEdits,
    coco_types::PermissionMode::Plan,
    coco_types::PermissionMode::BypassPermissions,
];

fn next_roster_mode(current: coco_types::PermissionMode, delta: i32) -> coco_types::PermissionMode {
    let idx = ROSTER_MODE_ORDER
        .iter()
        .position(|m| *m == current)
        .unwrap_or(0) as i32;
    let n = ROSTER_MODE_ORDER.len() as i32;
    ROSTER_MODE_ORDER[(idx + delta).rem_euclid(n) as usize]
}

pub(crate) fn cycle_mode(
    state: &mut AppState,
    delta: i32,
) -> Option<(String, coco_types::PermissionMode)> {
    let Some(ModalState::TeamRoster(r)) = state.ui.modal.as_mut() else {
        return None;
    };
    let member = r.members.get_mut(r.selected)?;
    member.mode = next_roster_mode(member.mode, delta);
    Some((member.name.clone(), member.mode))
}

pub(crate) fn cycle_all_modes(
    state: &mut AppState,
    delta: i32,
) -> Vec<(String, coco_types::PermissionMode)> {
    let Some(ModalState::TeamRoster(r)) = state.ui.modal.as_mut() else {
        return Vec::new();
    };
    let Some(first_mode) = r.members.first().map(|m| m.mode) else {
        return Vec::new();
    };
    let all_same = r.members.iter().all(|m| m.mode == first_mode);
    let target = if all_same {
        next_roster_mode(first_mode, delta)
    } else {
        coco_types::PermissionMode::Default
    };
    r.members
        .iter_mut()
        .map(|m| {
            m.mode = target;
            (m.name.clone(), target)
        })
        .collect()
}

#[cfg(test)]
#[path = "team_roster.test.rs"]
mod tests;
