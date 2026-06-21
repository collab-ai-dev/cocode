//! `/clear` — clear conversation history and session-scoped state.
//!
//! Returns a status message describing what was cleared. Actual state
//! clearing happens in the TUI layer; this handler only reports intent.

use std::pin::Pin;

/// Async handler for `/clear`.
///
/// Checks `~/.coco/sessions/` to provide context on preserved history,
/// then returns a message describing the clear operation.
pub fn handler(
    _args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move {
        let sessions_dir = coco_config::global_config::config_home().join("sessions");

        let session_count = count_session_files(&sessions_dir).await;

        let mut out = String::from(
            "Conversation cleared. Plan state, file caches, and \
             cache-break tracking reset. Session preserved for /resume.",
        );
        if session_count > 0 {
            out.push_str(&format!(
                "\n\n{session_count} session(s) available via /session list."
            ));
        }
        Ok(out)
    })
}

/// Count JSON session files in the sessions directory.
async fn count_session_files(sessions_dir: &std::path::Path) -> i64 {
    if !sessions_dir.exists() {
        return 0;
    }

    let Ok(mut entries) = tokio::fs::read_dir(sessions_dir).await else {
        return 0;
    };

    let mut count: i64 = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
            count += 1;
        }
    }
    count
}

#[cfg(test)]
#[path = "clear.test.rs"]
mod tests;
