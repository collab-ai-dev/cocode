use std::path::{Path, PathBuf};

use crate::session_runtime::SessionHandle;

use super::{SessionControlError, require_runtime};

pub struct SetPermissionModeResult {
    pub session_id: coco_types::SessionId,
    pub changed: bool,
}

pub struct PermissionModeStatus {
    pub session_id: coco_types::SessionId,
    pub current: coco_types::PermissionMode,
    pub bypass_available: bool,
}

pub struct PermissionRuleResetResult {
    pub cleared_allow_rules: usize,
    pub cleared_deny_rules: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionsMutation {
    Allow(String),
    Deny(String),
    Reset,
}

pub enum PermissionMutationAction {
    Apply {
        update: coco_types::PermissionUpdate,
        confirmation: String,
    },
    Reset {
        confirmation: String,
    },
}

pub struct PreparedDirectoryAccess {
    pub path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum DirectoryAccessPreparationError {
    #[error("Cannot add directory `{raw_path}`: {source}")]
    Canonicalize {
        raw_path: String,
        source: std::io::Error,
    },
    #[error("Cannot add directory `{path}`: not a directory")]
    NotDirectory { path: String },
    #[error("{0}")]
    AlreadyAccessible(String),
}

pub async fn set_permission_mode(
    runtime: Option<SessionHandle>,
    mode: coco_types::PermissionMode,
) -> Result<SetPermissionModeResult, SessionControlError> {
    let runtime = require_runtime(runtime, "control/setPermissionMode")?;
    let change = runtime.set_permission_mode(mode).await;
    Ok(SetPermissionModeResult {
        session_id: runtime.session_id().clone(),
        changed: change.changed,
    })
}

pub async fn permission_mode_status(
    runtime: Option<SessionHandle>,
) -> Result<PermissionModeStatus, SessionControlError> {
    let runtime = require_runtime(runtime, "permission mode status")?;
    Ok(PermissionModeStatus {
        session_id: runtime.session_id().clone(),
        current: runtime.effective_permission_mode().await,
        bypass_available: runtime.bypass_permissions_available().await,
    })
}

pub async fn apply_permission_update(
    runtime: Option<SessionHandle>,
    update: coco_types::PermissionUpdate,
) -> Result<(), SessionControlError> {
    let runtime = require_runtime(runtime, "control/applyPermissionUpdate")?;
    runtime
        .apply_permission_updates_everywhere(std::slice::from_ref(&update))
        .await;
    Ok(())
}

pub async fn prepare_directory_access_update(
    session: &SessionHandle,
    raw_path: &str,
) -> Result<PreparedDirectoryAccess, DirectoryAccessPreparationError> {
    let raw_path = raw_path.trim();
    let current_cwd = session.current_cwd().read().await.clone();
    let candidate = if Path::new(raw_path).is_absolute() {
        PathBuf::from(raw_path)
    } else {
        current_cwd.join(raw_path)
    };
    let absolute = candidate.canonicalize().map_err(|source| {
        DirectoryAccessPreparationError::Canonicalize {
            raw_path: raw_path.to_string(),
            source,
        }
    })?;
    if !absolute.is_dir() {
        return Err(DirectoryAccessPreparationError::NotDirectory {
            path: absolute.display().to_string(),
        });
    }

    let current = canonicalize_or_self(current_cwd);
    let additional_dirs: Vec<PathBuf> = session
        .additional_working_dirs()
        .await
        .into_iter()
        .map(canonicalize_or_self)
        .collect();

    if let Some(message) =
        directory_already_accessible_message(&absolute, &current, &additional_dirs)
    {
        return Err(DirectoryAccessPreparationError::AlreadyAccessible(message));
    }

    Ok(PreparedDirectoryAccess { path: absolute })
}

fn canonicalize_or_self(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

pub fn directory_already_accessible_message(
    directory_path: &Path,
    current_cwd: &Path,
    additional_dirs: &[PathBuf],
) -> Option<String> {
    if directory_path == current_cwd {
        return Some(format!(
            "{} is already the current working directory.",
            directory_path.display()
        ));
    }
    for working_dir in additional_dirs {
        if directory_path == working_dir {
            return Some(format!(
                "{} is already added as a working directory.",
                directory_path.display()
            ));
        }
    }
    if directory_path.starts_with(current_cwd) {
        return Some(format!(
            "{} is already accessible within the current working directory {}.",
            directory_path.display(),
            current_cwd.display()
        ));
    }
    for working_dir in additional_dirs {
        if directory_path.starts_with(working_dir) {
            return Some(format!(
                "{} is already accessible within the additional working directory {}.",
                directory_path.display(),
                working_dir.display()
            ));
        }
    }
    None
}

pub fn parse_permissions_mutation(args: &str) -> Option<PermissionsMutation> {
    let trimmed = args.trim();
    if trimmed == "reset" {
        return Some(PermissionsMutation::Reset);
    }
    if let Some(tool) = trimmed.strip_prefix("allow ") {
        let tool = tool.trim();
        if tool.is_empty() {
            return None;
        }
        return Some(PermissionsMutation::Allow(tool.to_string()));
    }
    if let Some(tool) = trimmed.strip_prefix("deny ") {
        let tool = tool.trim();
        if tool.is_empty() {
            return None;
        }
        return Some(PermissionsMutation::Deny(tool.to_string()));
    }
    None
}

pub fn permission_mutation_action(args: &str) -> Option<PermissionMutationAction> {
    use coco_types::{
        PermissionBehavior, PermissionRule, PermissionRuleSource, PermissionRuleValue,
    };

    let mutation = parse_permissions_mutation(args)?;
    Some(match mutation {
        PermissionsMutation::Allow(tool) => {
            let rule = PermissionRule {
                source: PermissionRuleSource::Session,
                behavior: PermissionBehavior::Allow,
                value: PermissionRuleValue {
                    tool_pattern: tool.clone(),
                    rule_content: None,
                },
            };
            PermissionMutationAction::Apply {
                update: coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::Session,
                },
                confirmation: format!(
                    "Added allow rule for `{tool}`.\n\nSource: Session (highest priority — \
                     active until end of session or `/permissions reset`)."
                ),
            }
        }
        PermissionsMutation::Deny(tool) => {
            let rule = PermissionRule {
                source: PermissionRuleSource::Session,
                behavior: PermissionBehavior::Deny,
                value: PermissionRuleValue {
                    tool_pattern: tool.clone(),
                    rule_content: None,
                },
            };
            PermissionMutationAction::Apply {
                update: coco_types::PermissionUpdate::AddRules {
                    rules: vec![rule],
                    destination: coco_types::PermissionUpdateDestination::Session,
                },
                confirmation: format!(
                    "Added deny rule for `{tool}`.\n\nSource: Session (highest priority — \
                     active until end of session or `/permissions reset`)."
                ),
            }
        }
        PermissionsMutation::Reset => {
            let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
            PermissionMutationAction::Reset {
                confirmation: format!(
                    "Session permission rules reset. Custom session allow/deny entries were cleared; \
                     built-in read-only tools remain allowed by the active permission mode. File-based rules \
                     ({config_dir}/settings.json, ~/{config_dir}/settings.json) are unchanged — \
                     edit those files directly to modify persistent rules."
                ),
            }
        }
    })
}

pub async fn reset_permission_rules(
    runtime: Option<SessionHandle>,
) -> Result<PermissionRuleResetResult, SessionControlError> {
    let runtime = require_runtime(runtime, "control/resetSessionPermissionRules")?;
    let (cleared_allow_rules, cleared_deny_rules) = runtime.reset_session_permission_rules().await;
    Ok(PermissionRuleResetResult {
        cleared_allow_rules,
        cleared_deny_rules,
    })
}
