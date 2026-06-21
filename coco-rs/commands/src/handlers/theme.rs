//! `/theme` — open the standalone theme picker.
//!
//! Ignores any argument and always opens the theme picker.

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;

pub struct ThemeHandler;

#[async_trait]
impl CommandHandler for ThemeHandler {
    /// Always open the picker overlay — the live-preview picker is the single
    /// entry point; any typed argument is ignored.
    async fn execute_command(&self, _args: &str) -> crate::Result<CommandResult> {
        Ok(CommandResult::OpenDialog(DialogSpec::ThemePicker))
    }

    fn handler_name(&self) -> &str {
        "theme"
    }
}

#[cfg(test)]
#[path = "theme.test.rs"]
mod tests;
