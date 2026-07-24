//! Production [`CheckExecutor`]: runs deterministic goal-contract checks
//! against the session's working directory.
//!
//! Contract checks are user-approved at goal creation (a model-drafted
//! plan can never silently redefine success), so executing them is
//! running user-approved commands — same trust class as hooks. Bounded:
//! wall-clock timeout + output cap; every failure maps to the verifier's
//! fail-closed "check did not pass".

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use coco_goal_runtime::{CheckExecutor, CommandObservation};

/// Wall-clock bound per check command; a hung check must not wedge the
/// goal driver.
const CHECK_COMMAND_TIMEOUT_SECS: u64 = 120;
/// Byte cap on captured output / file content fed to expectation matching.
const CHECK_OUTPUT_CAP_BYTES: usize = 65_536;

pub(crate) struct SessionCheckExecutor {
    cwd: PathBuf,
}

impl SessionCheckExecutor {
    pub(crate) fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let candidate = Path::new(path);
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.cwd.join(candidate)
        }
    }
}

fn cap_output(text: &str) -> String {
    coco_utils_string::take_bytes_at_char_boundary(text, CHECK_OUTPUT_CAP_BYTES).to_string()
}

#[async_trait]
impl CheckExecutor for SessionCheckExecutor {
    async fn run_command(&self, command: &str) -> Result<CommandObservation, String> {
        let run = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(Duration::from_secs(CHECK_COMMAND_TIMEOUT_SECS), run)
            .await
            .map_err(|_| format!("timed out after {CHECK_COMMAND_TIMEOUT_SECS}s"))?
            .map_err(|error| error.to_string())?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        if !output.stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        Ok(CommandObservation {
            exit_success: output.status.success(),
            output: cap_output(&text),
        })
    }

    async fn read_file(&self, path: &str) -> Result<String, String> {
        let content = tokio::fs::read_to_string(self.resolve(path))
            .await
            .map_err(|error| error.to_string())?;
        Ok(cap_output(&content))
    }

    async fn artifact_exists(&self, locator: &str) -> bool {
        tokio::fs::metadata(self.resolve(locator)).await.is_ok()
    }
}
