//! `/rewind` command — opens the message-selector overlay.
//!
//! The handler ignores `args` entirely; the `messageSelectorPreselect`
//! mechanism reaches the picker only through the message-actions edit
//! keyboard gesture.
//!

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

pub struct RewindHandler;

#[async_trait]
impl CommandHandler for RewindHandler {
    async fn execute_command(&self, _args: &str) -> crate::Result<CommandResult> {
        tracing::info!(target: "rewind::cmd", "rewind dispatched; opening picker");
        Ok(CommandResult::OpenDialog(DialogSpec::MessageSelector))
    }

    fn handler_name(&self) -> &str {
        "rewind"
    }
}

#[cfg(test)]
#[path = "rewind.test.rs"]
mod tests;
