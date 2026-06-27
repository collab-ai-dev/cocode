//! Validation diagnostics for agent definitions.
//!
//! Covers failed-definitions reporting and `/agents validate`. Validation is
//! structural only; it does not check that referenced tools exist (that needs
//! a tool registry snapshot).

use std::path::PathBuf;

use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use coco_types::AgentDefinition;
use thiserror::Error;

use crate::frontmatter::FrontmatterParseError;

/// What went wrong when parsing or validating one agent definition.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("missing `name` (frontmatter required)")]
    MissingName,
    #[error("missing `description` (frontmatter required for the AgentTool prompt list)")]
    MissingDescription,
    #[error(
        "invalid color value `{value}` (must be one of red/blue/green/yellow/purple/orange/pink/cyan)"
    )]
    InvalidColor { value: String },
    #[error("invalid isolation `{value}` (expected `none`, `worktree`, or `remote`)")]
    InvalidIsolation { value: String },
    #[error("invalid memory scope `{value}` (expected `user`, `project`, or `local`)")]
    InvalidMemoryScope { value: String },
    #[error("invalid permission_mode `{value}`")]
    InvalidPermissionMode { value: String },
    #[error(
        "invalid model_role `{value}` (expected one of main/fast/plan/explore/review/hook_agent/memory/subagent)"
    )]
    InvalidModelRole { value: String },
    #[error("invalid `max_turns` value `{value}` (expected positive integer)")]
    InvalidMaxTurns { value: String },
    #[error("invalid YAML frontmatter: {message}")]
    InvalidFrontmatter { message: String },
    #[error("invalid JSON: {message}")]
    InvalidJson { message: String },
    #[error("empty body — markdown agents need a prompt")]
    EmptyBody,
    #[error("file read error: {message}")]
    Io { message: String },
}

impl StackError for ValidationError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for ValidationError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::InvalidFrontmatter { .. } | Self::InvalidJson { .. } => StatusCode::ParseError,
            Self::Io { .. } => StatusCode::IoError,
            _ => StatusCode::InvalidArguments,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// One validation finding tied to a source location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationDiagnostic {
    pub path: PathBuf,
    pub agent_type: Option<String>,
    pub error: ValidationError,
}

impl ValidationDiagnostic {
    pub fn new(path: PathBuf, agent_type: Option<String>, error: ValidationError) -> Self {
        Self {
            path,
            agent_type,
            error,
        }
    }
}

impl From<FrontmatterParseError> for ValidationError {
    fn from(err: FrontmatterParseError) -> Self {
        match err {
            FrontmatterParseError::MissingName => ValidationError::MissingName,
            FrontmatterParseError::MissingDescription => ValidationError::MissingDescription,
            FrontmatterParseError::InvalidValue { field, message } => {
                ValidationError::InvalidFrontmatter {
                    message: format!("{field}: {message}"),
                }
            }
        }
    }
}

/// Cross-cuts a parsed `AgentDefinition`: are required fields populated?
/// Used after frontmatter parsing succeeded to catch "structurally valid but
/// semantically incomplete" definitions.
pub struct AgentDefinitionValidator;

impl AgentDefinitionValidator {
    /// Returns the list of validation errors. Empty when the definition
    /// is good enough to enter the active set.
    pub fn check(def: &AgentDefinition) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if def.name.trim().is_empty() {
            errors.push(ValidationError::MissingName);
        }
        let has_description = def
            .when_to_use
            .as_deref()
            .or(def.description.as_deref())
            .is_some_and(|s| !s.trim().is_empty());
        if !has_description {
            errors.push(ValidationError::MissingDescription);
        }
        errors
    }
}
