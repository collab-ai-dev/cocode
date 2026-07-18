//! Permission-family prompt behavior: tool permission, sandbox permission,
//! and MCP-server approval (the three `ApprovalResponse`-carrying prompts).
//!
//! Owns the always-allow rule construction (disk-persisted `LocalSettings`
//! allow rules, read-path directory widening) and the multi-choice payload
//! splice.

use rust_i18n::t;
use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::permission_options::PermissionAction;
use crate::state::AppState;
use crate::state::PanePromptState;
use crate::state::Toast;

/// Route an inline-editing command to the editable always-allow prefix field
/// when an allow row is focused on a shell-tool prompt (the
/// `PermissionPrefixEdit` keybinding context emits `InsertChar` / `Cursor*` /
/// `Delete*` there instead of y/n/a hotkeys). Returns `true` when the command
/// was consumed. Runs before the main dispatch so the keystroke edits the rule
/// field rather than leaking into the chat composer.
pub(crate) fn intercept_prefix_edit(state: &mut AppState, cmd: &crate::events::TuiCommand) -> bool {
    use crate::events::TuiCommand as C;
    if !matches!(
        cmd,
        C::InsertChar(_)
            | C::DeleteBackward
            | C::DeleteWordBackward
            | C::CursorLeft
            | C::CursorRight
            | C::CursorHome
            | C::CursorEnd
    ) {
        return false;
    }
    let mode = state.session.permission_mode;
    let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    if !crate::permission_options::prefix_editing(p, mode) {
        return false;
    }
    let Some(input) = p.prefix_input.as_mut() else {
        return false;
    };
    match cmd {
        C::InsertChar(c) => input.insert(*c),
        C::DeleteBackward => input.backspace(),
        C::DeleteWordBackward => input.delete_word_backward(),
        C::CursorLeft => input.left(),
        C::CursorRight => input.right(),
        C::CursorHome => input.home(),
        C::CursorEnd => input.end(),
        _ => return false,
    }
    true
}

/// Route inline editing to the permission prompt's deny-reason field while it
/// is open. Cloned from [`intercept_exit_plan_feedback_edit`] below — the same
/// shape, on the classic tool prompt.
///
/// Takes precedence over the y/n/a hotkeys: once the field is open the user is
/// typing prose, and a `n` in "won't work on Windows" must not deny the call.
pub(crate) fn intercept_deny_reason_edit(
    state: &mut AppState,
    cmd: &crate::events::TuiCommand,
) -> bool {
    use crate::events::TuiCommand as C;
    if !matches!(
        cmd,
        C::InsertChar(_)
            | C::DeleteBackward
            | C::DeleteWordBackward
            | C::CursorLeft
            | C::CursorRight
            | C::CursorHome
            | C::CursorEnd
    ) {
        return false;
    }
    let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    let Some(input) = p.deny_reason_input.as_mut() else {
        return false;
    };
    match cmd {
        C::InsertChar(c) => input.insert(*c),
        C::DeleteBackward => input.backspace(),
        C::DeleteWordBackward => input.delete_word_backward(),
        C::CursorLeft => input.left(),
        C::CursorRight => input.right(),
        C::CursorHome => input.home(),
        C::CursorEnd => input.end(),
        _ => return false,
    }
    true
}

/// Route inline editing to ExitPlanMode's "No, keep planning" feedback field
/// while that row is focused. Mirrors the source UI's input row without
/// widening the generic permission-choice wire type.
pub(crate) fn intercept_exit_plan_feedback_edit(
    state: &mut AppState,
    cmd: &crate::events::TuiCommand,
) -> bool {
    use crate::events::TuiCommand as C;
    if !matches!(
        cmd,
        C::InsertChar(_)
            | C::DeleteBackward
            | C::DeleteWordBackward
            | C::CursorLeft
            | C::CursorRight
            | C::CursorHome
            | C::CursorEnd
    ) {
        return false;
    }
    let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_mut() else {
        return false;
    };
    if !exit_plan_feedback_editing(p) {
        return false;
    }
    let crate::state::PermissionDetail::ExitPlanMode { feedback_input, .. } = &mut p.detail else {
        return false;
    };
    match cmd {
        C::InsertChar(c) => feedback_input.insert(*c),
        C::DeleteBackward => feedback_input.backspace(),
        C::DeleteWordBackward => feedback_input.delete_word_backward(),
        C::CursorLeft => feedback_input.left(),
        C::CursorRight => feedback_input.right(),
        C::CursorHome => feedback_input.home(),
        C::CursorEnd => feedback_input.end(),
        _ => return false,
    }
    true
}

/// Single resolution chokepoint for classic (non-choice) tool-permission
/// prompts. Every classic decision — `y` / `n` / `a` hotkeys, Enter on the
/// focused row, digit shortcuts — funnels through here so the
/// `ApprovalResponse` construction and the structured decision log exist
/// exactly once.
pub(crate) async fn resolve_classic_permission(
    p: &crate::state::PermissionPromptState,
    action: PermissionAction,
    current_mode: coco_types::PermissionMode,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    let (approved, always_allow, permission_updates) = match action {
        PermissionAction::ApproveOnce => (true, false, vec![]),
        PermissionAction::AllowSession => (
            true,
            true,
            crate::permission_options::session_allow_updates(p, current_mode),
        ),
        PermissionAction::AllowLocal => (
            true,
            true,
            crate::permission_options::local_allow_updates(p),
        ),
        PermissionAction::Deny => (false, false, vec![]),
    };
    // A denial's reason, when the user opened the field and typed one. It
    // reaches the model as `feedback`, so a denied call can be corrected
    // instead of blindly retried. Only meaningful on Deny — an approval's
    // field is never opened.
    let feedback = (!approved)
        .then_some(p.deny_reason_input.as_ref())
        .flatten()
        .map(|input| input.value.trim())
        .filter(|reason| !reason.is_empty())
        .map(str::to_string);
    tracing::info!(
        target: "coco_tui::permission",
        request_id = %p.request_id,
        tool_name = %p.tool_name,
        permission_decision = if approved { "approve" } else { "deny" },
        always_allow,
        rules = permission_updates.len(),
        multi_choice = false,
        has_deny_reason = feedback.is_some(),
        "user permission decision",
    );
    if let Err(e) = command_tx
        .send(UserCommand::ApprovalResponse {
            request_id: p.request_id.clone(),
            approved,
            always_allow,
            feedback,
            updated_input: None,
            resolution_detail: None,
            permission_updates,
            content_blocks: None,
        })
        .await
    {
        tracing::warn!(
            target: "coco_tui::permission",
            error = %e,
            "failed to dispatch ApprovalResponse (channel closed)",
        );
    }
}

/// Approve ('y' / approve choice) on a tool-permission prompt.
///
/// Multi-choice mode: commits the currently-focused choice (Enter takes the
/// same path via `confirm`). The chosen `value` is carried as typed
/// resolution detail so the tool's `execute()` can branch on it; a choice whose
/// value is "no" denies. Classic mode commits a one-shot approve regardless
/// of the focused row — `y` is the ApproveOnce hotkey, not "confirm
/// selection" (that's Enter); the rendered rows carry their hotkeys so the
/// mapping is visible.
pub(crate) async fn approve_permission(
    p: &crate::state::PermissionPromptState,
    current_mode: coco_types::PermissionMode,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    let Some(choices) = &p.choices else {
        resolve_classic_permission(p, PermissionAction::ApproveOnce, current_mode, command_tx)
            .await;
        return true;
    };
    let chosen_is_no = choices
        .get(p.selected_choice)
        .map(|c| c.value == coco_types::ExitPlanChoice::No.as_str())
        .unwrap_or(false);
    let approved = !chosen_is_no;
    let (feedback, content_blocks) = if chosen_is_no && exit_plan_no_requires_feedback(p) {
        let Some((feedback, content_blocks)) = exit_plan_feedback(p).await else {
            return false;
        };
        (Some(feedback), content_blocks)
    } else {
        (None, None)
    };
    tracing::info!(
        target: "coco_tui::permission",
        request_id = %p.request_id,
        tool_name = %p.tool_name,
        permission_decision = if approved { "approve" } else { "deny" },
        always_allow = false,
        multi_choice = true,
        "user permission decision",
    );
    if let Err(e) = command_tx
        .send(UserCommand::ApprovalResponse {
            request_id: p.request_id.clone(),
            approved,
            always_allow: false,
            feedback,
            updated_input: None,
            resolution_detail: build_choice_detail(p),
            permission_updates: exit_plan_allowed_prompt_updates(p, approved),
            content_blocks,
        })
        .await
    {
        tracing::warn!(
            target: "coco_tui::permission",
            error = %e,
            "failed to dispatch ApprovalResponse (channel closed)",
        );
    }
    true
}

/// Approve/deny a sandbox-permission prompt.
pub(crate) async fn respond_sandbox(
    s: &crate::state::SandboxPermissionPromptState,
    approved: bool,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    tracing::info!(
        target: "coco_tui::permission",
        request_id = %s.request_id,
        kind = "sandbox",
        permission_decision = if approved { "approve" } else { "deny" },
        "user sandbox permission decision",
    );
    let _ = command_tx
        .send(UserCommand::ApprovalResponse {
            request_id: s.request_id.clone(),
            approved,
            always_allow: false,
            feedback: None,
            updated_input: None,
            resolution_detail: None,
            permission_updates: vec![],
            content_blocks: None,
        })
        .await;
}

/// Approve/deny an MCP-server approval prompt.
pub(crate) async fn respond_mcp_server(
    m: &crate::state::McpServerApprovalPromptState,
    approved: bool,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    tracing::info!(
        target: "coco_tui::permission",
        request_id = %m.request_id,
        kind = "mcp_server",
        permission_decision = if approved { "approve" } else { "deny" },
        "user MCP server approval decision",
    );
    let _ = command_tx
        .send(UserCommand::ApprovalResponse {
            request_id: m.request_id.clone(),
            approved,
            always_allow: false,
            feedback: None,
            updated_input: None,
            resolution_detail: None,
            permission_updates: vec![],
            content_blocks: None,
        })
        .await;
}

/// Deny ('n') a tool-permission prompt.
pub(crate) async fn deny_permission(
    p: &crate::state::PermissionPromptState,
    current_mode: coco_types::PermissionMode,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    if p.choices.is_some() {
        return false;
    }
    resolve_classic_permission(p, PermissionAction::Deny, current_mode, command_tx).await;
    true
}

/// Handle `ApproveAll` (always-allow) for permission prompts.
///
/// Builds a `LocalSettings`-scoped allow rule for the tool. `tui_runner`
/// both applies the update to the live `engine_config` via
/// `coco_permissions::apply_permission_updates` (so subsequent same-tool
/// calls in the session don't re-prompt) and persists it to
/// `project config dir/settings.local.json` via `SettingsPermissionStore::persist_update`
/// (so the grant survives restart). `LocalSettings` is the gitignored,
/// per-developer file — a reflexive "don't ask again" must never silently
/// edit team-shared (`ProjectSettings`) or global (`UserSettings`) config.
///
/// Picking `Project` / `User` destinations lives in the dedicated
/// `/permissions` rule-editor overlay, not this inline popup.
pub(crate) async fn approve_all(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_ref() else {
        return;
    };
    // Choice dialogs have no always-allow affordance ('a' is not a decision
    // key there); ignore silently like any other unmapped key.
    if p.choices.is_some() {
        return;
    }
    if !p.show_always_allow {
        // Gated off (managed settings allow only managed permission rules).
        // Never no-op silently: tell the user why their keypress did
        // nothing and leave the prompt open for an explicit y/n.
        tracing::info!(
            target: "coco_tui::permission",
            request_id = %p.request_id,
            tool_name = %p.tool_name,
            "always-allow requested but disabled by managed settings",
        );
        state
            .ui
            .add_toast(Toast::warning(t!("toast.always_allow_disabled")));
        return;
    }
    let actions = crate::permission_options::classic_actions(p, state.session.permission_mode);
    let action = if actions.contains(&PermissionAction::AllowLocal) {
        PermissionAction::AllowLocal
    } else if actions.contains(&PermissionAction::AllowSession) {
        PermissionAction::AllowSession
    } else {
        state
            .ui
            .add_toast(Toast::warning(t!("toast.always_allow_disabled")));
        return;
    };
    resolve_classic_permission(p, action, state.session.permission_mode, command_tx).await;
    state.ui.dismiss_prompt();
}

/// Handle the explicit session-scoped allow hotkey.
pub(crate) async fn approve_session(state: &mut AppState, command_tx: &mpsc::Sender<UserCommand>) {
    let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_ref() else {
        return;
    };
    if p.choices.is_some() {
        return;
    }
    let actions = crate::permission_options::classic_actions(p, state.session.permission_mode);
    if !actions.contains(&PermissionAction::AllowSession) {
        return;
    }
    resolve_classic_permission(
        p,
        PermissionAction::AllowSession,
        state.session.permission_mode,
        command_tx,
    )
    .await;
    state.ui.dismiss_prompt();
}

/// Handle `ClassifierAutoApprove` — background classifier approved the pending
/// request before the user responded.
pub(crate) async fn classifier_auto_approve(
    state: &mut AppState,
    command_tx: &mpsc::Sender<UserCommand>,
    request_id: String,
) {
    if let Some(PanePromptState::Permission(p)) = state.ui.interaction.active_prompt.as_ref()
        && p.request_id == request_id
    {
        tracing::info!(
            target: "coco_tui::permission",
            request_id = %p.request_id,
            tool_name = %p.tool_name,
            permission_decision = "approve",
            source = "classifier",
            "classifier auto-approve",
        );
        let _ = command_tx
            .send(UserCommand::ApprovalResponse {
                request_id: p.request_id.clone(),
                approved: true,
                always_allow: false,
                feedback: None,
                updated_input: None,
                resolution_detail: None,
                permission_updates: vec![],
                content_blocks: None,
            })
            .await;
        state.ui.dismiss_prompt();
    }
}

/// Confirm (Enter) on a tool-permission prompt: commit the focused choice
/// (multi-choice) or the focused classic action.
pub(crate) async fn confirm_permission(
    p: &crate::state::PermissionPromptState,
    current_mode: coco_types::PermissionMode,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    if p.choices.is_some() {
        // Multi-choice commit shares `approve_permission`'s splice + log.
        return approve_permission(p, current_mode, command_tx).await;
    }
    resolve_classic_permission(
        p,
        crate::permission_options::selected_classic_action(p, current_mode),
        current_mode,
        command_tx,
    )
    .await;
    true
}

/// Digit shortcut (`1`-`3`) on a classic tool-permission prompt: commit the
/// numbered row directly. Returns `false` when the digit doesn't address a
/// row (multi-choice mode or out of range) — the caller keeps the prompt
/// open.
pub(crate) async fn commit_permission_digit(
    p: &crate::state::PermissionPromptState,
    digit: usize,
    current_mode: coco_types::PermissionMode,
    command_tx: &mpsc::Sender<UserCommand>,
) -> bool {
    if p.choices.is_some() {
        return false;
    }
    let Some(index) = digit.checked_sub(1) else {
        return false;
    };
    let actions = crate::permission_options::classic_actions(p, current_mode);
    if index >= actions.len() {
        return false;
    }
    resolve_classic_permission(
        p,
        crate::permission_options::classic_action_at(p, current_mode, index),
        current_mode,
        command_tx,
    )
    .await;
    true
}

/// Move the choice cursor on a permission prompt (wrapping).
pub(crate) fn nav_permission(
    p: &mut crate::state::PermissionPromptState,
    current_mode: coco_types::PermissionMode,
    delta: i32,
) {
    let count = p
        .choices
        .as_ref()
        .map(Vec::len)
        .unwrap_or_else(|| crate::permission_options::classic_actions(p, current_mode).len())
        as i32;
    if count > 0 {
        let current = p.selected_choice as i32;
        let next = (current + delta).rem_euclid(count);
        p.selected_choice = next as usize;
    }
}

pub(crate) fn exit_plan_feedback_editing(p: &crate::state::PermissionPromptState) -> bool {
    if p.tool_name != coco_types::ToolName::ExitPlanMode.as_str() {
        return false;
    }
    let Some(choice) = p
        .choices
        .as_ref()
        .and_then(|choices| choices.get(p.selected_choice))
    else {
        return false;
    };
    choice.value == coco_types::ExitPlanChoice::No.as_str() && exit_plan_no_requires_feedback(p)
}

fn exit_plan_no_requires_feedback(p: &crate::state::PermissionPromptState) -> bool {
    p.tool_name == coco_types::ToolName::ExitPlanMode.as_str()
        && matches!(
            p.detail,
            crate::state::PermissionDetail::ExitPlanMode {
                outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
                ..
            }
        )
}

async fn exit_plan_feedback(
    p: &crate::state::PermissionPromptState,
) -> Option<(String, Option<Vec<serde_json::Value>>)> {
    let crate::state::PermissionDetail::ExitPlanMode {
        feedback_input,
        feedback_images,
        ..
    } = &p.detail
    else {
        return None;
    };
    let content_blocks = image_content_blocks(feedback_images).await;
    let trimmed = feedback_input.value.trim();
    if !trimmed.is_empty() {
        return Some((trimmed.to_string(), content_blocks));
    }
    if content_blocks.is_some() {
        return Some(("(See attached image)".to_string(), content_blocks));
    }
    None
}

async fn image_content_blocks(
    images: &[crate::state::FeedbackImage],
) -> Option<Vec<serde_json::Value>> {
    if images.is_empty() {
        return None;
    }
    use base64::Engine as _;
    let mut blocks = Vec::with_capacity(images.len());
    for image in images {
        let source_mime = if image.mime.is_empty() {
            "image/png"
        } else {
            image.mime.as_str()
        };
        let normalized = normalize_feedback_image(image.bytes.to_vec(), source_mime.to_string())
            .await
            .unwrap_or_else(|| (image.bytes.to_vec(), source_mime.to_string()));
        let (bytes, media_type) = normalized;
        blocks.push(serde_json::json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": base64::engine::general_purpose::STANDARD.encode(bytes),
            }
        }));
    }
    Some(blocks)
}

async fn normalize_feedback_image(bytes: Vec<u8>, media_type: String) -> Option<(Vec<u8>, String)> {
    let raw_len = bytes.len();
    let result = tokio::task::spawn_blocking(move || {
        coco_utils_image::normalize_image_bytes(bytes, &media_type)
    })
    .await;
    match result {
        Ok(Ok((normalized_bytes, normalized_mime))) => Some((normalized_bytes, normalized_mime)),
        Ok(Err(error)) => {
            tracing::warn!(
                target: "coco_tui::permission",
                bytes = raw_len,
                error = %error,
                "failed to normalize ExitPlanMode feedback image; using original bytes",
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                target: "coco_tui::permission",
                bytes = raw_len,
                error = %error,
                "ExitPlanMode feedback image normalization task failed; using original bytes",
            );
            None
        }
    }
}

/// Build trusted typed metadata for a multi-choice permission selection.
pub(crate) fn build_choice_detail(
    p: &crate::state::PermissionPromptState,
) -> Option<coco_types::PermissionResolutionDetail> {
    let selected = p.choices.as_ref()?.get(p.selected_choice)?;
    if p.tool_name != coco_types::ToolName::ExitPlanMode.as_str() {
        return None;
    }
    let choice = coco_types::ExitPlanChoice::from_wire(&selected.value)?;
    tracing::info!(
        target: "coco_tui::permission",
        selected_choice = p.selected_choice,
        ?choice,
        clears_context = choice.clears_context(),
        "ExitPlanMode resolution detail built",
    );
    let edited_plan = if choice == coco_types::ExitPlanChoice::No {
        None
    } else {
        match &p.detail {
            crate::state::PermissionDetail::ExitPlanMode { edited_plan, .. } => edited_plan.clone(),
            _ => None,
        }
    };
    Some(coco_types::PermissionResolutionDetail::ExitPlanMode {
        choice,
        edited_plan,
    })
}

fn exit_plan_allowed_prompt_updates(
    p: &crate::state::PermissionPromptState,
    approved: bool,
) -> Vec<coco_types::PermissionUpdate> {
    if !approved || p.tool_name != coco_types::ToolName::ExitPlanMode.as_str() {
        return Vec::new();
    }
    let crate::state::PermissionDetail::ExitPlanMode {
        allowed_prompts, ..
    } = &p.detail
    else {
        return Vec::new();
    };
    if allowed_prompts.is_empty() {
        return Vec::new();
    }
    let rules = allowed_prompts
        .iter()
        .map(|prompt| coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::Session,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: prompt.tool.clone(),
                rule_content: Some(prompt.prompt.clone()),
            },
        })
        .collect();
    vec![coco_types::PermissionUpdate::AddRules {
        rules,
        destination: coco_types::PermissionUpdateDestination::Session,
    }]
}
