//! `/moa` — run one prompt through the configured default MoA preset.

use async_trait::async_trait;

use crate::CommandHandler;
use crate::CommandResult;

pub struct MoaHandler;

#[async_trait]
impl CommandHandler for MoaHandler {
    async fn execute_command(&self, args: &str) -> crate::Result<CommandResult> {
        let prompt = args.trim();
        if prompt.is_empty() {
            return Ok(CommandResult::Text(
                "Usage: /moa <prompt>\nRuns one prompt through settings.moa.default_preset."
                    .to_string(),
            ));
        }
        Ok(CommandResult::MoaOneShot {
            prompt: prompt.to_string(),
        })
    }

    fn handler_name(&self) -> &str {
        "moa"
    }
}

#[cfg(test)]
#[path = "moa.test.rs"]
mod tests;
