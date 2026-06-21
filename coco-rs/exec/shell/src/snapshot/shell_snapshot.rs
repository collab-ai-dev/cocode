//! Shell snapshot capture and management.
//!
//! Creates a snapshot of the user's shell environment at session start,
//! which is then sourced before each command to avoid login shell overhead.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use tokio::fs;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::time::timeout;

use crate::shell_types::Shell;
use crate::shell_types::ShellType;
use crate::snapshot::scripts::bash_snapshot_script;
use crate::snapshot::scripts::powershell_snapshot_script;
use crate::snapshot::scripts::sh_snapshot_script;
use crate::snapshot::scripts::zsh_snapshot_script;

/// Default timeout for snapshot capture operations.
const DEFAULT_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default retention period for snapshot files (7 days).
const DEFAULT_SNAPSHOT_RETENTION: Duration = Duration::from_secs(60 * 60 * 24 * 7);

/// Default directory name for shell snapshots (hyphenated).
const DEFAULT_SNAPSHOT_DIR: &str = "shell-snapshots";

/// Configuration for shell snapshotting.
#[derive(Debug, Clone)]
pub struct SnapshotConfig {
    /// Directory to store snapshot files.
    pub snapshot_dir: PathBuf,
    /// Timeout for snapshot capture operations.
    pub timeout: Duration,
    /// How long to retain inactive snapshot files before cleanup.
    pub retention: Duration,
}

impl SnapshotConfig {
    /// Creates a new config with the given home directory.
    pub fn new(coco_home: &Path) -> Self {
        Self {
            snapshot_dir: coco_home.join(DEFAULT_SNAPSHOT_DIR),
            timeout: DEFAULT_SNAPSHOT_TIMEOUT,
            retention: DEFAULT_SNAPSHOT_RETENTION,
        }
    }

    /// Returns the default snapshot directory name.
    pub fn default_dir_name() -> &'static str {
        DEFAULT_SNAPSHOT_DIR
    }

    /// Returns the default retention duration.
    pub fn default_retention() -> Duration {
        DEFAULT_SNAPSHOT_RETENTION
    }
}

impl Default for SnapshotConfig {
    fn default() -> Self {
        let home = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".coco");
        Self {
            snapshot_dir: home.join(DEFAULT_SNAPSHOT_DIR),
            timeout: DEFAULT_SNAPSHOT_TIMEOUT,
            retention: DEFAULT_SNAPSHOT_RETENTION,
        }
    }
}

/// A captured shell environment snapshot.
///
/// When dropped, the snapshot file is automatically deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellSnapshot {
    /// Path to the snapshot file on disk.
    pub path: PathBuf,
}

impl ShellSnapshot {
    /// Starts asynchronous shell snapshotting in a background task.
    ///
    /// Spawns a tokio task that captures the shell environment and sends
    /// the result through a watch channel on the shell's snapshot receiver.
    pub fn start_snapshotting(config: SnapshotConfig, session_id: &str, shell: &mut Shell) {
        let (tx, rx) = watch::channel(None);
        shell.set_shell_snapshot_receiver(rx);

        let snapshot_shell = shell.clone();
        let snapshot_session_id = session_id.to_string();

        tokio::spawn(async move {
            let snapshot = Self::try_new(&config, &snapshot_session_id, &snapshot_shell)
                .await
                .map(Arc::new);

            if snapshot.is_some() {
                tracing::info!("Shell snapshot created for session {snapshot_session_id}");
            } else {
                tracing::warn!("Failed to create shell snapshot for session {snapshot_session_id}");
            }

            let _ = tx.send(snapshot);
        });
    }

    /// Attempts to create a new shell snapshot.
    ///
    /// Returns `None` if creation fails (unsupported shell, timeout, validation
    /// failure, etc.). Commands will fall back to login shell mode.
    ///
    /// Naming: `snapshot-<shell>-<ts>-<rand6>.sh` — unique across sessions and
    /// shells so re-shell within a session doesn't clobber the previous file.
    pub async fn try_new(config: &SnapshotConfig, session_id: &str, shell: &Shell) -> Option<Self> {
        let extension = match shell.shell_type() {
            ShellType::PowerShell => "ps1",
            _ => "sh",
        };

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let rand_id: u32 = rand::random();
        let _ = session_id; // accepted for diagnostics + future correlation
        let path = config.snapshot_dir.join(format!(
            "snapshot-{}-{timestamp}-{rand_id:06x}.{extension}",
            shell.name()
        ));

        let snapshot = match write_shell_snapshot(shell, &path, config.timeout).await {
            Ok(path) => {
                tracing::debug!("Shell snapshot written to: {}", path.display());
                Some(Self { path })
            }
            Err(err) => {
                tracing::warn!(
                    "Failed to create shell snapshot for {}: {err:?}",
                    shell.name()
                );
                None
            }
        };

        // Validate the snapshot can be sourced without errors
        if let Some(ref snapshot) = snapshot
            && let Err(err) = validate_snapshot(shell, &snapshot.path, config.timeout).await
        {
            tracing::error!("Shell snapshot validation failed: {err:?}");
            let _ = fs::remove_file(&snapshot.path).await;
            return None;
        }

        snapshot
    }

    /// Returns the path to the snapshot file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ShellSnapshot {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Failed to delete shell snapshot at {:?}: {err:?}",
                self.path
            );
        }
    }
}

/// Writes a shell snapshot to disk.
async fn write_shell_snapshot(
    shell: &Shell,
    output_path: &Path,
    timeout: Duration,
) -> Result<PathBuf> {
    let shell_type = shell.shell_type();

    if *shell_type == ShellType::PowerShell || *shell_type == ShellType::Cmd {
        bail!("Shell snapshot not yet supported for {shell_type:?}");
    }

    let raw_snapshot = capture_snapshot(shell, timeout).await?;
    let snapshot = strip_snapshot_preamble(&raw_snapshot)?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!("Failed to create snapshot directory: {}", parent.display())
        })?;
    }

    fs::write(output_path, snapshot)
        .await
        .with_context(|| format!("Failed to write snapshot to: {}", output_path.display()))?;

    Ok(output_path.to_path_buf())
}

/// Captures a shell environment snapshot by running the appropriate script.
async fn capture_snapshot(shell: &Shell, snapshot_timeout: Duration) -> Result<String> {
    let script = match shell.shell_type() {
        ShellType::Zsh => zsh_snapshot_script(),
        ShellType::Bash => bash_snapshot_script(),
        ShellType::Sh => sh_snapshot_script(),
        ShellType::PowerShell => powershell_snapshot_script().to_string(),
        ShellType::Cmd => bail!("Shell snapshotting is not supported for cmd"),
    };

    run_script_with_timeout(shell, &script, snapshot_timeout, /*login*/ true).await
}

/// Strips shell initialization output before the snapshot marker.
fn strip_snapshot_preamble(snapshot: &str) -> Result<String> {
    let marker = "# Snapshot file";
    let Some(start) = snapshot.find(marker) else {
        bail!("Snapshot output missing marker '{marker}'");
    };
    Ok(snapshot[start..].to_string())
}

/// Validates a snapshot by attempting to source it in a non-login shell.
#[cfg_attr(test, allow(dead_code))]
pub(crate) async fn validate_snapshot(
    shell: &Shell,
    snapshot_path: &Path,
    timeout: Duration,
) -> Result<()> {
    let script = format!("set -e; . \"{}\"", snapshot_path.display());
    run_script_with_timeout(shell, &script, timeout, /*login*/ false)
        .await
        .map(|_| ())
}

/// Runs a shell script with a timeout, returning stdout on success.
///
/// Sets environment variables:
/// - `GIT_EDITOR=true`: prevents git from blocking with an editor
/// - `COCO=1`: signals to user config scripts that this is a coco subprocess
/// - `SHELL`: set to the shell binary path for correct shell detection
async fn run_script_with_timeout(
    shell: &Shell,
    script: &str,
    snapshot_timeout: Duration,
    use_login_shell: bool,
) -> Result<String> {
    let args = shell.derive_exec_args(script, use_login_shell);
    let shell_name = shell.name();

    let mut handler = Command::new(&args[0]);
    handler.args(&args[1..]);
    handler.kill_on_drop(true);

    handler.env("GIT_EDITOR", "true");
    handler.env("COCO", "1");
    handler.env("SHELL", shell.shell_path().as_os_str());

    let output = timeout(snapshot_timeout, handler.output())
        .await
        .map_err(|_| anyhow::anyhow!("Snapshot command timed out for {shell_name}"))?
        .with_context(|| format!("Failed to execute {shell_name}"))?;

    if !output.status.success() {
        let status = output.status;
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Snapshot command exited with status {status}: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
#[path = "shell_snapshot.test.rs"]
mod tests;
