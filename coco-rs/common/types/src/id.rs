use serde::Deserialize;
use serde::Serialize;
use std::borrow::Borrow;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdValidationError {
    Empty,
    DotSegment,
    PathSeparator,
    InvalidAgentId,
    InvalidAgentLabel,
    InvalidUuid,
}

impl fmt::Display for IdValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("id must not be empty"),
            Self::DotSegment => f.write_str("id must not be '.' or '..'"),
            Self::PathSeparator => f.write_str("id must not contain a path separator"),
            Self::InvalidAgentId => {
                f.write_str("agent id must match a[optional-label-][16-hex-chars]")
            }
            Self::InvalidAgentLabel => f.write_str(
                "agent id label must contain only lowercase ASCII letters, digits, '_' or '-'",
            ),
            Self::InvalidUuid => f.write_str("session id must be a UUID"),
        }
    }
}

impl std::error::Error for IdValidationError {}

fn validate_path_component(id: &str) -> Result<(), IdValidationError> {
    if id.is_empty() {
        return Err(IdValidationError::Empty);
    }
    if matches!(id, "." | "..") {
        return Err(IdValidationError::DotSegment);
    }
    if id.contains('/') || id.contains('\\') {
        return Err(IdValidationError::PathSeparator);
    }
    Ok(())
}

fn is_lower_hex(c: char) -> bool {
    matches!(c, '0'..='9' | 'a'..='f')
}

fn validate_agent_label(label: &str) -> Result<(), IdValidationError> {
    if label.is_empty() {
        return Err(IdValidationError::InvalidAgentLabel);
    }
    if label
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
    {
        Ok(())
    } else {
        Err(IdValidationError::InvalidAgentLabel)
    }
}

fn validate_generated_agent_id(id: &str) -> Result<(), IdValidationError> {
    validate_path_component(id)?;

    let rest = id
        .strip_prefix('a')
        .ok_or(IdValidationError::InvalidAgentId)?;

    if rest.len() == 16 && rest.chars().all(is_lower_hex) {
        return Ok(());
    }

    let Some((label, hex)) = rest.rsplit_once('-') else {
        return Err(IdValidationError::InvalidAgentId);
    };
    validate_agent_label(label)?;
    if hex.len() == 16 && hex.chars().all(is_lower_hex) {
        Ok(())
    } else {
        Err(IdValidationError::InvalidAgentId)
    }
}

/// Branded session identifier.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn try_new(id: impl Into<String>) -> Result<Self, IdValidationError> {
        let id = id.into();
        validate_path_component(&id)?;
        Ok(Self(id))
    }

    pub fn try_new_uuid(id: impl Into<String>) -> Result<Self, IdValidationError> {
        let id = id.into();
        validate_path_component(&id)?;
        uuid::Uuid::parse_str(&id).map_err(|_| IdValidationError::InvalidUuid)?;
        Ok(Self(id))
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for SessionId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for SessionId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<'de> serde::Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        Self::try_new(id).map_err(serde::de::Error::custom)
    }
}

/// Branded agent identifier.
/// Format: `a[optional-label-][16-hex-chars]`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct AgentId(String);

impl AgentId {
    pub fn try_new(id: impl Into<String>) -> Result<Self, IdValidationError> {
        let id = id.into();
        validate_path_component(&id)?;
        Ok(Self(id))
    }

    pub fn try_new_generated(id: impl Into<String>) -> Result<Self, IdValidationError> {
        let id = id.into();
        validate_generated_agent_id(&id)?;
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    /// Generate a new agent ID with optional label.
    pub fn generate(label: Option<&str>) -> Self {
        match Self::try_generate(label) {
            Ok(id) => id,
            Err(_) => unreachable!("AgentId label must be canonical"),
        }
    }

    pub fn try_generate(label: Option<&str>) -> Result<Self, IdValidationError> {
        if let Some(label) = label {
            validate_agent_label(label)?;
        }

        let hex: String = uuid::Uuid::new_v4()
            .as_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        let id = match label {
            Some(label) => format!("a{label}-{hex}"),
            None => format!("a{hex}"),
        };
        Self::try_new_generated(id)
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for AgentId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl<'de> serde::Deserialize<'de> for AgentId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let id = String::deserialize(deserializer)?;
        Self::try_new(id).map_err(serde::de::Error::custom)
    }
}

/// Branded task identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for TaskId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TaskId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Cross-newtype conversion: a BgAgent task's id IS its agent identity.
/// Use this when a `TaskId` known to belong to a BgAgent variant needs
/// to be reinterpreted as an `AgentId` for routing.
impl From<TaskId> for AgentId {
    fn from(t: TaskId) -> Self {
        Self(t.0)
    }
}

impl From<AgentId> for TaskId {
    fn from(a: AgentId) -> Self {
        Self(a.0)
    }
}

/// Branded turn identifier. One per logical user-prompt cycle —
/// shared between the paired `TurnStarted` and `TurnEnded` events.
/// Generated by the runner layer (`tui_runner` / `sdk_runner`),
/// not the engine, so pre-engine hook blocks still emit a complete
/// lifecycle pair.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(String);

impl TurnId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }

    /// Generate a fresh turn id. Format: `t-<16-hex>`.
    pub fn generate() -> Self {
        let hex: String = uuid::Uuid::new_v4()
            .as_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        Self(format!("t-{hex}"))
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for TurnId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for TurnId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TurnId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl PartialEq<&str> for TurnId {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl PartialEq<TurnId> for &str {
    fn eq(&self, other: &TurnId) -> bool {
        *self == other.0
    }
}

/// Branded surface attachment identifier.
///
/// A surface is a client-visible attachment to a live session. It is
/// generated by the server, used for routing/capability checks, and is
/// never persisted as transcript identity.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SurfaceId(pub String);

impl SurfaceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for SurfaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Borrow<str> for SurfaceId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl From<String> for SurfaceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SurfaceId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[cfg(test)]
#[path = "id.test.rs"]
mod tests;
