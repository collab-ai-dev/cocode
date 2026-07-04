//! `/provider` — open the add-provider wizard (no args).
//!
//! Without args, opens the `ProviderWizard` overlay: pick a provider template
//! from the builtin catalog, supply the secret (and, for a custom provider,
//! its base URL), then persist to `settings.json` under `providers.<name>`.
//! There is no argument form — the flow is interactive-only.

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

pub struct ProviderHandler;

#[async_trait]
impl CommandHandler for ProviderHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        if args.trim().is_empty() {
            return Ok(CommandResult::OpenDialog(DialogSpec::ProviderWizard));
        }
        // No argument form — guide the user to the interactive wizard.
        Ok(CommandResult::Text(
            "Usage: /provider — opens an interactive wizard to add a provider. \
             (Configure providers directly under `providers.<name>` in \
             settings.json for non-interactive setups.)"
                .to_string(),
        ))
    }

    fn handler_name(&self) -> &str {
        "provider"
    }
}
