//! `/context` — runtime-owned context window usage.
//!
//! The static command registry cannot compute this command correctly because
//! it needs live `SessionRuntime` state. TUI/SDK dispatch intercept `/context`
//! before this handler.

use std::pin::Pin;

pub fn handler(
    _args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(
        async move { Ok("Context usage is available from an active session runtime.".to_string()) },
    )
}

#[cfg(test)]
#[path = "context.test.rs"]
mod tests;
