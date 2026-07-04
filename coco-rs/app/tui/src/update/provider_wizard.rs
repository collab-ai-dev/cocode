//! Key dispatch + persistence for the `/provider` add-provider wizard.
//!
//! Mirrors the `/agents` create wizard's `intercept_wizard`: a linear step
//! machine where `Enter` validates + advances, `Esc` steps back (or closes on
//! the first step), and the text steps edit a [`WizardTextField`]. The final
//! `Confirm` step writes the new provider to `settings.json` under
//! `providers.<name>` via `coco_config::global_config::write_user_setting`.

use std::collections::BTreeMap;

use crate::events::TuiCommand;
use crate::i18n::t;
use crate::state::AppState;
use crate::state::ModalState;
use crate::state::ProviderWizardState;
use crate::state::ProviderWizardStep;
use crate::state::WizardTextField;
use crate::state::ui::Toast;

/// Outcome of [`intercept`]. Mirrors `agents_dialog::Handled`.
pub(super) enum Handled {
    Yes(bool),
    No,
}

pub(super) fn intercept(state: &mut AppState, cmd: &TuiCommand) -> Handled {
    let step = match state.ui.modal.as_ref() {
        Some(ModalState::ProviderWizard(w)) => w.step,
        _ => return Handled::No,
    };

    match step {
        ProviderWizardStep::Template => match cmd {
            TuiCommand::CursorUp => Handled::Yes(cycle_template(state, -1)),
            TuiCommand::CursorDown => Handled::Yes(cycle_template(state, 1)),
            TuiCommand::SubmitInput => Handled::Yes(advance(state)),
            TuiCommand::Cancel => Handled::Yes(back_or_cancel(state)),
            _ => Handled::Yes(false),
        },
        ProviderWizardStep::Confirm => match cmd {
            TuiCommand::SubmitInput => {
                confirm(state);
                Handled::Yes(true)
            }
            TuiCommand::Cancel => Handled::Yes(back_or_cancel(state)),
            _ => Handled::Yes(false),
        },
        // Text-input steps: Name / BaseUrl / ApiKey / Model.
        _ => match cmd {
            TuiCommand::SubmitInput => Handled::Yes(validate_and_advance(state)),
            TuiCommand::Cancel => Handled::Yes(back_or_cancel(state)),
            TuiCommand::DeleteBackward => Handled::Yes(edit(state, WizardTextField::delete_back)),
            TuiCommand::DeleteForward => Handled::Yes(edit(state, WizardTextField::delete_forward)),
            TuiCommand::CursorLeft => Handled::Yes(edit(state, WizardTextField::move_left)),
            TuiCommand::CursorRight => Handled::Yes(edit(state, WizardTextField::move_right)),
            TuiCommand::CursorHome => Handled::Yes(edit(state, WizardTextField::move_home)),
            TuiCommand::CursorEnd => Handled::Yes(edit(state, WizardTextField::move_end)),
            TuiCommand::InsertChar(c) => {
                let c = *c;
                Handled::Yes(edit(state, |f| f.insert_char(c)))
            }
            _ => Handled::Yes(false),
        },
    }
}

fn wizard_mut(state: &mut AppState) -> Option<&mut ProviderWizardState> {
    match state.ui.modal.as_mut() {
        Some(ModalState::ProviderWizard(w)) => Some(w),
        _ => None,
    }
}

fn edit(state: &mut AppState, f: impl FnOnce(&mut WizardTextField)) -> bool {
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    let Some(field) = w.active_field_mut() else {
        return false;
    };
    f(field);
    w.error = None;
    true
}

fn cycle_template(state: &mut AppState, delta: i32) -> bool {
    match wizard_mut(state) {
        Some(w) => {
            w.cycle_template(delta);
            true
        }
        None => false,
    }
}

fn advance(state: &mut AppState) -> bool {
    match wizard_mut(state) {
        Some(w) => w.advance(),
        None => false,
    }
}

fn back_or_cancel(state: &mut AppState) -> bool {
    if let Some(w) = wizard_mut(state)
        && w.back()
    {
        return true;
    }
    state.ui.dismiss_modal();
    true
}

fn validate_and_advance(state: &mut AppState) -> bool {
    let err = match state.ui.modal.as_ref() {
        Some(ModalState::ProviderWizard(w)) => validate_step(w),
        _ => return false,
    };
    let Some(w) = wizard_mut(state) else {
        return false;
    };
    match err {
        Some(e) => w.error = Some(e),
        None => {
            w.advance();
        }
    }
    true
}

/// Per-step validation. Only `Name` and (custom) `BaseUrl` are required; the
/// API key and model id are optional.
fn validate_step(w: &ProviderWizardState) -> Option<String> {
    match w.step {
        ProviderWizardStep::Name if w.resolved_name().is_empty() => {
            Some(t!("dialog.provider_wizard_err_name").to_string())
        }
        ProviderWizardStep::BaseUrl => {
            let url = w.base_url.text.trim();
            (!url.starts_with("http://") && !url.starts_with("https://"))
                .then(|| t!("dialog.provider_wizard_err_base_url").to_string())
        }
        _ => None,
    }
}

/// Build the `PartialProviderConfig` the wizard will persist. Pure — split out
/// from [`confirm`] so the written shape can be unit-tested without `AppState`.
fn build_partial(w: &ProviderWizardState) -> coco_config::PartialProviderConfig {
    let tpl = w.selected_template();
    let key = w.api_key.text.trim().to_string();
    let model_id = w.model_id.text.trim().to_string();

    let mut partial = coco_config::PartialProviderConfig {
        api: Some(tpl.api),
        base_url: Some(w.resolved_base_url()),
        wire_api: Some(tpl.wire_api),
        ..Default::default()
    };
    if !tpl.env_key.is_empty() {
        partial.env_key = Some(tpl.env_key.clone());
    }
    if !key.is_empty() {
        partial.api_key = Some(coco_config::RedactedSecret::new(key));
    }
    if !model_id.is_empty() {
        let mut models = BTreeMap::new();
        models.insert(
            model_id,
            coco_config::PartialProviderModelOverride::default(),
        );
        partial.models = Some(models);
    }
    partial
}

/// Build a `PartialProviderConfig` from the wizard and persist it to
/// `settings.json` under `providers.<name>`, then close with a toast.
fn confirm(state: &mut AppState) {
    let Some(ModalState::ProviderWizard(w)) = state.ui.modal.as_ref() else {
        return;
    };
    let name = w.resolved_name();
    let partial = build_partial(w);

    let toast = match serde_json::to_value(&partial) {
        Ok(value) => {
            match coco_config::global_config::write_user_setting(
                &format!("providers.{name}"),
                value,
            ) {
                Ok(path) => Toast::info(
                    t!(
                        "dialog.provider_wizard_saved",
                        name = name.as_str(),
                        path = path.display().to_string().as_str()
                    )
                    .to_string(),
                ),
                Err(e) => Toast::warning(
                    t!(
                        "dialog.provider_wizard_save_failed",
                        err = e.to_string().as_str()
                    )
                    .to_string(),
                ),
            }
        }
        Err(e) => Toast::warning(
            t!(
                "dialog.provider_wizard_save_failed",
                err = e.to_string().as_str()
            )
            .to_string(),
        ),
    };

    state.ui.dismiss_modal();
    state.ui.add_toast(toast);
}

#[cfg(test)]
#[path = "provider_wizard.test.rs"]
mod tests;
