//! `/vim` — persist editor mode (vim ↔ normal).
//!
//! Flips editor mode (vim ↔ normal). Persisted via
//! `~/.coco/state/editor_mode` (single-line text file). The TUI reads
//! this file at startup; absence means "normal".

use std::path::PathBuf;
use std::pin::Pin;

const VALID_MODES: &[&str] = &["normal", "vim"];

pub fn handler(
    args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move { handler_with_home(default_home(), args).await })
}

/// Test-friendly variant: callers pass an explicit `$HOME` so parallel tests
/// don't race via process-wide `std::env::set_var("HOME", ...)`.
pub async fn handler_with_home(home: PathBuf, args: String) -> crate::Result<String> {
    let path = state_file_path(&home);
    let current = read_mode(&path).await;

    let target = match args.trim().to_ascii_lowercase().as_str() {
        "" | "toggle" => match current.as_str() {
            "vim" => "normal",
            _ => "vim",
        }
        .to_string(),
        "vim" | "on" | "enable" => "vim".to_string(),
        "normal" | "off" | "disable" | "emacs" => "normal".to_string(),
        other => {
            return Ok(format!(
                "Unknown editor mode: {other}. Use one of: {}.",
                VALID_MODES.join(", ")
            ));
        }
    };

    write_mode(&path, &target).await?;

    Ok(match target.as_str() {
        "vim" => "Editor mode set to vim. Use Escape to toggle INSERT/NORMAL inside the input."
            .to_string(),
        _ => "Editor mode set to normal (standard readline bindings).".to_string(),
    })
}

fn default_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn state_file_path(home: &std::path::Path) -> PathBuf {
    home.join(".coco").join("state").join("editor_mode")
}

async fn read_mode(path: &std::path::Path) -> String {
    tokio::fs::read_to_string(path)
        .await
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "normal".to_string())
}

async fn write_mode(path: &std::path::Path, mode: &str) -> crate::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, mode).await?;
    Ok(())
}

#[cfg(test)]
#[path = "vim.test.rs"]
mod tests;
