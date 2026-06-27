//! Bottom interaction pane (tui-v2 §6.4, Stage 3): per-prompt behavior
//! modules behind one routing layer.
//!
//! The prompt *stack* (priority queue, delayed permissions, request-id
//! replacement) is TEA Model state and stays in
//! [`crate::state::interaction::InteractionPaneState`]; this module owns the
//! prompt *behavior* — the `route_*` functions below match the focused
//! prompt once and delegate to its surface module, replacing the old
//! eight-way matches spread through `update/interaction.rs`'s free
//! functions. Modal (full-screen) surface handling stays in
//! `update/interaction.rs`; the update-layer entry points there try the
//! focused prompt first (or in `confirm`'s case, after modals — the
//! pre-existing order, preserved exactly) and fall through to modals.

pub(crate) mod permission;
pub(crate) mod plan;
pub(crate) mod question;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::events::TuiCommand;
use crate::state::AppState;
use crate::state::PanePromptState;

/// Keys for the confirmation-class prompts (permission / sandbox / MCP
/// approval / plan entry / plan exit) — shared across the family, so the
/// map lives on the routing layer rather than one surface.
pub(crate) fn confirmation_map_key(state: &AppState, key: KeyEvent) -> Option<TuiCommand> {
    match key.code {
        KeyCode::Char('y' | 'Y') => Some(TuiCommand::Approve),
        KeyCode::Char('n' | 'N') => Some(TuiCommand::Deny),
        KeyCode::Char('a' | 'A')
            if current_permission_actions(state).is_some_and(|actions| {
                actions.contains(&crate::permission_options::PermissionAction::AllowLocal)
                    || actions.contains(&crate::permission_options::PermissionAction::AllowSession)
            }) =>
        {
            Some(TuiCommand::ApproveAll)
        }
        KeyCode::Char('s' | 'S')
            if current_permission_actions(state).is_some_and(|actions| {
                actions.contains(&crate::permission_options::PermissionAction::AllowSession)
            }) =>
        {
            Some(TuiCommand::ApproveSession)
        }
        // Digit shortcuts commit the numbered classic-permission row
        // directly; non-permission confirmation prompts ignore them.
        KeyCode::Char(c @ '1'..='9') => Some(TuiCommand::PermissionDigit(
            c.to_digit(10).map(|d| d as usize).unwrap_or(0),
        )),
        // Tab cycles multi-option confirmations; for simple Y/N
        // dialogs the handler is a no-op.
        KeyCode::Tab => Some(TuiCommand::SurfaceNext),
        KeyCode::BackTab => Some(TuiCommand::SurfacePrev),
        KeyCode::Up | KeyCode::Char('k') => Some(TuiCommand::SurfacePrev),
        KeyCode::Down | KeyCode::Char('j') => Some(TuiCommand::SurfaceNext),
        // PageUp/PageDown scroll the (possibly long) command/detail body while
        // the action rows stay pinned — Up/Down remain reserved for navigating
        // the action/choice list above.
        KeyCode::PageUp => Some(TuiCommand::PermissionScrollUp),
        KeyCode::PageDown => Some(TuiCommand::PermissionScrollDown),
        KeyCode::Enter => Some(TuiCommand::SurfaceConfirm),
        KeyCode::Esc => Some(TuiCommand::Cancel),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(TuiCommand::Cancel)
        }
        _ => None,
    }
}

fn current_permission_actions(
    state: &AppState,
) -> Option<Vec<crate::permission_options::PermissionAction>> {
    let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_ref() else {
        return None;
    };
    if p.choices.is_some() {
        return None;
    }
    Some(crate::permission_options::classic_actions(
        p,
        state.session.permission_mode,
    ))
}

/// Route `Approve` to the focused prompt. Returns `true` when the keystroke was
/// consumed by a focused prompt; `false` means the caller should try the modal
/// surfaces. A modal renders on top of any prompt and owns the keys, so an
/// active modal yields `false` (the prompt is hidden beneath it). A prompt
/// that doesn't treat Approve as a decision keeps itself open — the pending
/// request must never be silently dropped.
pub(crate) async fn route_approve(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    if state.ui.modal.is_some() {
        return false;
    }
    let permission_mode = state.session.permission_mode;
    let Some(prompt) = state.ui.interaction.active_prompt.as_ref() else {
        return false;
    };
    let resolved = match prompt {
        PanePromptState::Permission(p) => {
            permission::approve_permission(p, permission_mode, command_tx).await;
            true
        }
        PanePromptState::SandboxPermission(s) => {
            permission::respond_sandbox(s, /*approved*/ true, command_tx).await;
            true
        }
        PanePromptState::McpServerApproval(m) => {
            permission::respond_mcp_server(m, /*approved*/ true, command_tx).await;
            true
        }
        PanePromptState::PlanEntry(_) => {
            plan::approve_plan_entry(state, command_tx).await;
            true
        }
        // Approve is not a decision key for these — consume it but keep the
        // prompt open (Question/PlanApproval answer via Enter; CostWarning via
        // its own keys). Dismissing here orphaned the pending request.
        PanePromptState::Question(_)
        | PanePromptState::CostWarning(_)
        | PanePromptState::PlanApproval(_) => false,
    };
    if resolved {
        state.ui.dismiss_prompt();
    }
    true
}

/// Route `Deny` to the focused prompt. Returns `true` when the keystroke was
/// consumed by a focused prompt; `false` falls through to modal surfaces. Like
/// [`route_approve`], a prompt that doesn't treat Deny as a decision keeps
/// itself open rather than dropping the request.
pub(crate) async fn route_deny(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    if state.ui.modal.is_some() {
        return false;
    }
    let permission_mode = state.session.permission_mode;
    let Some(prompt) = state.ui.interaction.active_prompt.as_ref() else {
        return false;
    };
    let resolved = match prompt {
        PanePromptState::Permission(p) => {
            permission::deny_permission(p, permission_mode, command_tx).await;
            true
        }
        PanePromptState::SandboxPermission(s) => {
            permission::respond_sandbox(s, /*approved*/ false, command_tx).await;
            true
        }
        PanePromptState::McpServerApproval(m) => {
            permission::respond_mcp_server(m, /*approved*/ false, command_tx).await;
            true
        }
        PanePromptState::Question(_)
        | PanePromptState::CostWarning(_)
        | PanePromptState::PlanEntry(_)
        | PanePromptState::PlanApproval(_) => false,
    };
    if resolved {
        state.ui.dismiss_prompt();
    }
    true
}

/// Route `Confirm` (Enter) to the focused prompt, taken off the stack by the
/// caller. Question prompts may restore themselves (multi-page flows); every
/// other prompt commits and finishes.
pub(crate) async fn route_confirm(
    state: &mut AppState,
    prompt: PanePromptState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    match prompt {
        PanePromptState::Question(q) => {
            if question::confirm_question_prompt(state, q, command_tx).await {
                return;
            }
            state.ui.finish_taken_prompt();
        }
        PanePromptState::PlanApproval(p) => {
            plan::confirm_plan_approval(&p, command_tx).await;
            state.ui.finish_taken_prompt();
        }
        PanePromptState::Permission(ref p) => {
            permission::confirm_permission(p, state.session.permission_mode, command_tx).await;
            state.ui.finish_taken_prompt();
        }
        // Plan entry is a benign confirmation (not a privilege escalation), so
        // Enter commits it exactly like `y` — leaving it on the restore branch
        // made Enter a silent no-op on the "Enter plan mode?" prompt.
        PanePromptState::PlanEntry(_) => {
            plan::approve_plan_entry(state, command_tx).await;
            state.ui.finish_taken_prompt();
        }
        // Enter is not a decision key for these binary approvals — restore the
        // prompt so the pending engine/teammate request is answered by an
        // explicit y/n rather than silently dropped (which hung the request),
        // and without auto-approving a sandbox/MCP escalation on Enter.
        // CostWarning answers via its own keys.
        PanePromptState::SandboxPermission(_)
        | PanePromptState::CostWarning(_)
        | PanePromptState::McpServerApproval(_) => {
            state.ui.restore_prompt(prompt);
        }
    }
}

/// Route a digit shortcut to the focused prompt. Only classic permission
/// prompts treat digits as decisions (committing the numbered row); every
/// other prompt consumes the keystroke and stays open.
pub(crate) async fn route_permission_digit(
    state: &mut AppState,
    digit: usize,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    if state.ui.modal.is_some() {
        return false;
    }
    let Some(prompt) = state.ui.interaction.active_prompt.as_ref() else {
        return false;
    };
    let resolved = match prompt {
        PanePromptState::Permission(p) => {
            permission::commit_permission_digit(p, digit, state.session.permission_mode, command_tx)
                .await
        }
        _ => false,
    };
    if resolved {
        state.ui.dismiss_prompt();
    }
    true
}

/// Route selection movement to the focused prompt. Returns `true` when a
/// prompt was focused (the keystroke is consumed even if the prompt has no
/// cursor).
pub(crate) fn route_nav(state: &mut AppState, delta: i32) -> bool {
    if state.ui.modal.is_some() {
        return false;
    }
    let permission_mode = state.session.permission_mode;
    let Some(prompt) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    match prompt {
        PanePromptState::Question(q) => question::question_nav(q, delta),
        PanePromptState::Permission(p) => permission::nav_permission(p, permission_mode, delta),
        PanePromptState::PlanApproval(p) => {
            if delta != 0 {
                p.toggle_focus();
            }
        }
        PanePromptState::SandboxPermission(_)
        | PanePromptState::CostWarning(_)
        | PanePromptState::PlanEntry(_)
        | PanePromptState::McpServerApproval(_) => {}
    }
    true
}

/// Route a filter keystroke to the focused prompt. Returns `true` when a
/// prompt consumed it (only Question prompts route filter keys today).
pub(crate) fn route_filter(state: &mut AppState, c: char) -> bool {
    if state.ui.modal.is_some() {
        return false;
    }
    if !matches!(
        state.ui.interaction.active_prompt,
        Some(PanePromptState::Question(_))
    ) {
        return false;
    }
    question::filter_question(state, c);
    true
}

/// Route a filter backspace to the focused prompt. Returns `true` when a
/// prompt consumed it.
pub(crate) fn route_filter_backspace(state: &mut AppState) -> bool {
    if state.ui.modal.is_some() {
        return false;
    }
    if !matches!(
        state.ui.interaction.active_prompt,
        Some(PanePromptState::Question(_))
    ) {
        return false;
    }
    question::question_free_text_backspace(state);
    true
}
