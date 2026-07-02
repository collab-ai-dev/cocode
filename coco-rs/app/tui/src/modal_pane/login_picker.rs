//! Login picker modal — a flat, filterable list of OAuth-capable provider
//! instances (the no-arg `/login` form). Confirm dispatches
//! `UserCommand::ProviderLogin` for the focused row; the CLI runner then runs
//! that instance's OAuth flow on the shared `AuthService`.

use tokio::sync::mpsc;

use crate::command::UserCommand;
use crate::state::AppState;
use crate::state::LoginEntry;
use crate::state::LoginPickerState;

/// Rows visible under the current substring filter — matches the provider
/// label and the auth label, case-insensitive.
pub(crate) fn filtered(l: &LoginPickerState) -> Vec<&LoginEntry> {
    let f = l.filter.to_lowercase();
    l.entries
        .iter()
        .filter(|e| {
            f.is_empty()
                || e.provider_display.to_lowercase().contains(&f)
                || e.auth_label.to_lowercase().contains(&f)
        })
        .collect()
}

/// Enter — start the OAuth login flow for the focused provider instance.
pub(super) async fn confirm(
    state: &mut AppState,
    l: LoginPickerState,
    command_tx: &mpsc::Sender<UserCommand>,
) {
    if let Some(entry) = filtered(&l).get(l.selected as usize).copied() {
        let _ = command_tx
            .send(UserCommand::ProviderLogin {
                provider: entry.provider.clone(),
            })
            .await;
    }
    state.ui.finish_taken_modal();
}

#[cfg(test)]
#[path = "login_picker.test.rs"]
mod tests;
