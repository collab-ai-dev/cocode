use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::path::Path;

/// Platform information.
///
/// Has two textual forms:
/// - [`Display`](fmt::Display) — human title-case (`macOS` / `Linux` /
///   `Windows`). Used for the os-version fallback string.
/// - [`Self::ts_name`] — lowercase wire identifier (`darwin` / `linux` /
///   `win32`) used in the system-prompt `<env>` block.
///
/// Kept as two methods because the two forms are semantically distinct
/// (human display vs wire format), but `Display` is idiomatic Rust so
/// the title-case form goes through `to_string()` rather than a named
/// `display_name()` helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Darwin,
    Linux,
    Windows,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Darwin
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// Lowercase identifier for the system-prompt env block
    /// (`darwin`/`linux`/`win32`).
    pub fn ts_name(&self) -> &'static str {
        match self {
            Self::Darwin => "darwin",
            Self::Linux => "linux",
            Self::Windows => "win32",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Darwin => "macOS",
            Self::Linux => "Linux",
            Self::Windows => "Windows",
        })
    }
}

/// Shell type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind {
    Bash,
    Zsh,
    Sh,
    PowerShell,
}

impl ShellKind {
    /// Detect the current shell from $SHELL env var.
    pub fn detect() -> Self {
        let shell = std::env::var("SHELL").unwrap_or_default();
        if shell.contains("zsh") {
            Self::Zsh
        } else if shell.contains("bash") {
            Self::Bash
        } else if shell.contains("pwsh") || shell.contains("powershell") {
            Self::PowerShell
        } else {
            Self::Bash // default
        }
    }

    /// Lowercase shell name (`bash`/`zsh`/`sh`/`powershell`).
    pub fn ts_name(&self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Sh => "sh",
            Self::PowerShell => "powershell",
        }
    }
}

/// Per-model knowledge cutoff date. Delegates to
/// [`coco_model_card::knowledge_cutoff`] — exact-id lookup against the
/// process-global model-card catalog. Returns `None` for unknown model IDs so
/// the env block omits the line rather than render a wrong date.
pub fn knowledge_cutoff_for_model(model_id: &str) -> Option<String> {
    coco_model_card::knowledge_cutoff(model_id)
}

/// Collected environment information for system prompt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentInfo {
    pub cwd: String,
    pub platform: Platform,
    pub shell: ShellKind,
    pub os_version: String,
    pub model: String,
    pub knowledge_cutoff: String,
    pub is_git_repo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_status: Option<GitStatus>,
}

/// Git repository status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitStatus {
    pub branch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    pub status: String,
    pub recent_commits: String,
}

/// Build environment info for the given working directory.
///
/// `include_git_status` gates the git-status snapshot (suppressed when
/// git instructions are disabled). `is_git_repo` is reported
/// independently — the `<env>` repo flag is kept even when the status
/// block is suppressed.
pub fn get_environment_info(cwd: &Path, model: &str, include_git_status: bool) -> EnvironmentInfo {
    let is_git_repo = cwd.join(".git").exists();
    let git_status = if is_git_repo && include_git_status {
        get_git_status(cwd).ok()
    } else {
        None
    };

    // Single normalization point for the model name shown to the agent:
    // strip any provider prefix and resolve to the catalog-canonical bare
    // id. Every `<env>` block (main loop, headless, subagents) routes
    // through here, so main and child agents render the model identically.
    // Unknown / self-hosted models pass through unchanged.
    let model = coco_model_card::display_model_name(model);
    let os_version = get_os_version();
    let knowledge_cutoff = knowledge_cutoff_for_model(&model).unwrap_or_default();

    EnvironmentInfo {
        cwd: cwd.to_string_lossy().to_string(),
        platform: Platform::current(),
        shell: ShellKind::detect(),
        os_version,
        model,
        knowledge_cutoff,
        is_git_repo,
        git_status,
    }
}

/// Get git status for a repository.
fn get_git_status(cwd: &Path) -> crate::Result<GitStatus> {
    let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let status = run_git(cwd, &["status", "--short"])?;
    let user = run_git(cwd, &["config", "user.name"]).ok();
    let recent_commits = run_git(cwd, &["log", "--oneline", "-5"]).unwrap_or_default();

    // Detect main branch
    let main_branch = detect_main_branch(cwd);

    Ok(GitStatus {
        branch,
        main_branch,
        user,
        status,
        recent_commits,
    })
}

fn detect_main_branch(cwd: &Path) -> Option<String> {
    for candidate in &["main", "master"] {
        if run_git(cwd, &["rev-parse", "--verify", candidate]).is_ok() {
            return Some(candidate.to_string());
        }
    }
    None
}

fn run_git(cwd: &Path, args: &[&str]) -> crate::Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(crate::ContextError::git_failed("git command failed"))
    }
}

fn get_os_version() -> String {
    let output = std::process::Command::new("uname").arg("-sr").output().ok();
    output
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| Platform::current().to_string())
}

#[cfg(test)]
#[path = "environment.test.rs"]
mod tests;
