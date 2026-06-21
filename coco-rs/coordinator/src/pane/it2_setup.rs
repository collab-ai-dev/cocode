//! iTerm2 CLI (it2) setup utilities.
//!
//! Detects Python package managers, installs the `it2` CLI tool,
//! verifies the Python API is enabled, and persists setup state.

/// Python package manager for it2 installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PythonPackageManager {
    Uvx,
    Pipx,
    Pip,
}

impl PythonPackageManager {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Uvx => "uvx",
            Self::Pipx => "pipx",
            Self::Pip => "pip",
        }
    }

    /// Install command for this package manager.
    pub fn install_command(&self) -> Vec<&'static str> {
        match self {
            Self::Uvx => vec!["uv", "tool", "install", "it2"],
            Self::Pipx => vec!["pipx", "install", "it2"],
            Self::Pip => vec!["pip", "install", "--user", "it2"],
        }
    }
}

/// Result from installing it2.
#[derive(Debug)]
pub struct It2InstallResult {
    pub success: bool,
    pub error: Option<String>,
    pub package_manager: Option<PythonPackageManager>,
}

/// Result from verifying it2 setup.
#[derive(Debug)]
pub struct It2VerifyResult {
    pub success: bool,
    pub error: Option<String>,
    /// Whether Python API needs to be enabled in iTerm2 preferences.
    pub needs_python_api_enabled: bool,
}

/// Detect the best available Python package manager.
///
/// Checks in order: uv → pipx → pip → pip3.
pub async fn detect_python_package_manager() -> Option<PythonPackageManager> {
    // Check uv (preferred — globally isolated)
    if is_command_available("uv").await {
        return Some(PythonPackageManager::Uvx);
    }
    // Check pipx
    if is_command_available("pipx").await {
        return Some(PythonPackageManager::Pipx);
    }
    // Check pip
    if is_command_available("pip").await {
        return Some(PythonPackageManager::Pip);
    }
    // Check pip3 (fallback)
    if is_command_available("pip3").await {
        return Some(PythonPackageManager::Pip);
    }
    None
}

/// Install the it2 CLI using the given package manager.
pub async fn install_it2(pm: PythonPackageManager) -> It2InstallResult {
    let cmd = pm.install_command();
    let Some((program, args)) = cmd.split_first() else {
        return It2InstallResult {
            success: false,
            error: Some("install command is empty".to_string()),
            package_manager: Some(pm),
        };
    };

    let result = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => It2InstallResult {
            success: true,
            error: None,
            package_manager: Some(pm),
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            // Fallback: if pip failed, try pip3
            if pm == PythonPackageManager::Pip {
                let fallback = tokio::process::Command::new("pip3")
                    .args(["install", "--user", "it2"])
                    .output()
                    .await;
                if let Ok(fb) = fallback
                    && fb.status.success()
                {
                    return It2InstallResult {
                        success: true,
                        error: None,
                        package_manager: Some(pm),
                    };
                }
            }
            It2InstallResult {
                success: false,
                error: Some(stderr),
                package_manager: Some(pm),
            }
        }
        Err(e) => It2InstallResult {
            success: false,
            error: Some(e.to_string()),
            package_manager: Some(pm),
        },
    }
}

/// Verify that it2 is installed and the Python API is enabled.
pub async fn verify_it2_setup() -> It2VerifyResult {
    // Check if it2 is installed
    if !is_command_available("it2").await {
        return It2VerifyResult {
            success: false,
            error: Some("it2 CLI not found".to_string()),
            needs_python_api_enabled: false,
        };
    }

    // Test the Python API connection
    let result = tokio::process::Command::new("it2")
        .args(["session", "list"])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => It2VerifyResult {
            success: true,
            error: None,
            needs_python_api_enabled: false,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
            let needs_api = stderr.contains("api")
                || stderr.contains("python")
                || stderr.contains("connection refused")
                || stderr.contains("not enabled");
            It2VerifyResult {
                success: false,
                error: Some(String::from_utf8_lossy(&output.stderr).to_string()),
                needs_python_api_enabled: needs_api,
            }
        }
        Err(e) => It2VerifyResult {
            success: false,
            error: Some(e.to_string()),
            needs_python_api_enabled: false,
        },
    }
}

/// Get instructions for enabling the Python API in iTerm2.
pub fn get_python_api_instructions() -> Vec<&'static str> {
    vec![
        "Almost done! Enable the Python API in iTerm2:",
        "",
        "  iTerm2 → Settings → General → Magic → Enable Python API",
        "",
        "After enabling, you may need to restart iTerm2.",
    ]
}

/// Mark it2 setup as complete in global config.
pub fn mark_it2_setup_complete() {
    let path = it2_setup_state_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    let _ = std::fs::write(&path, "complete");
}

/// Check if it2 setup has been completed.
pub fn is_it2_setup_complete() -> bool {
    it2_setup_state_path().exists()
}

/// Set preference: use tmux over iTerm2.
pub fn set_prefer_tmux_over_iterm2(prefer: bool) {
    let path = tmux_preference_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    let _ = std::fs::write(&path, if prefer { "true" } else { "false" });
}

/// Get preference: use tmux over iTerm2.
pub fn get_prefer_tmux_over_iterm2() -> bool {
    std::fs::read_to_string(tmux_preference_path())
        .ok()
        .is_some_and(|v| v.trim() == "true")
}

// ── Helpers ──

fn it2_setup_state_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".coco")
        .join("it2-setup-complete")
}

fn tmux_preference_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".coco")
        .join("prefer-tmux-over-iterm2")
}

async fn is_command_available(cmd: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(cmd)
        .output()
        .await
        .is_ok_and(|o| o.status.success())
}

#[cfg(test)]
#[path = "it2_setup.test.rs"]
mod tests;
