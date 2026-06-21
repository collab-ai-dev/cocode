//! `/keybindings` — write template + open ~/.coco/keybindings.json in $EDITOR.
//!
//! Writes a template (with `wx` exclusive-create) so existing customizations
//! are never clobbered, then opens the file in the user's editor. Uses a
//! small JSON skeleton as the template.

use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;

const TEMPLATE: &str = r#"{
  "// docs": "Custom keybindings. Each entry maps a context+chord to an action.",
  "// example": {
    "context": "input",
    "chord": "ctrl+l",
    "action": "input:clear"
  },
  "bindings": []
}
"#;

pub fn handler(
    _args: String,
) -> Pin<Box<dyn std::future::Future<Output = crate::Result<String>> + Send>> {
    Box::pin(async move { handler_with_overrides(default_home(), default_editor()).await })
}

/// Test-friendly variant: callers pass an explicit `$HOME` and editor command
/// so parallel tests don't race on process-wide env vars.
pub async fn handler_with_overrides(home: PathBuf, editor: String) -> crate::Result<String> {
    let path = keybindings_path(&home);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let created = match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .await
    {
        Ok(mut f) => {
            use tokio::io::AsyncWriteExt;
            f.write_all(TEMPLATE.as_bytes()).await?;
            true
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(e) => return Err(e.into()),
    };

    if editor.is_empty() {
        let verb = if created { "Created" } else { "Found" };
        return Ok(format!(
            "{verb} {}.\nSet $EDITOR (or $VISUAL) and re-run /keybindings to open it.",
            path.display()
        ));
    }

    // Spawn editor; respect shell-tokenized $EDITOR (e.g. "code -w").
    let mut parts = editor.split_whitespace();
    let bin = match parts.next() {
        Some(b) => b,
        None => {
            return Ok(format!("Opened {} (no editor configured).", path.display()));
        }
    };
    let extra: Vec<&str> = parts.collect();
    let status = tokio::process::Command::new(bin)
        .args(extra)
        .arg(&path)
        .status()
        .await;

    let verb = if created { "Created" } else { "Opened" };
    match status {
        Ok(s) if s.success() => Ok(format!("{verb} {} in your editor.", path.display())),
        Ok(s) => Ok(format!(
            "{verb} {} but the editor exited with status {s}.",
            path.display()
        )),
        Err(e) => Ok(format!(
            "{verb} {} but couldn't launch editor `{editor}`: {e}",
            path.display()
        )),
    }
}

fn default_home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn default_editor() -> String {
    std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_default()
}

fn keybindings_path(home: &Path) -> PathBuf {
    home.join(".coco").join("keybindings.json")
}

#[cfg(test)]
#[path = "keybindings.test.rs"]
mod tests;
