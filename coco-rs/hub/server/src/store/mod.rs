use async_trait::async_trait;
use coco_error::ErrorExt;
use coco_error::StackError;
use coco_error::StatusCode;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchFrame;
use coco_types::SessionId;
use coco_types::TurnId;
use serde::Deserialize;
use serde::Serialize;

pub type Cursor = String;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<Cursor>,
    pub estimated_total: Option<i64>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
            estimated_total: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRow {
    pub instance_id: String,
    pub host: String,
    pub cwd: String,
    pub pid: Option<i64>,
    pub started_at: i64,
    pub version: Option<String>,
    pub kind: String,
    pub entrypoint: Option<String>,
    pub name: Option<String>,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub status: String,
    pub session_count: usize,
    pub source_kind: String,
    pub synthetic_identity: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub instance_id: String,
    pub session_id: SessionId,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub model: Option<String>,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_usd: f64,
    pub last_seq: i64,
    pub last_event_ts: i64,
    pub discovered_via: String,
    pub title: Option<String>,
    pub first_prompt: String,
    pub message_count: i32,
    pub cwd: Option<String>,
    pub file_size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub instance_id: String,
    pub session_id: SessionId,
    pub event_id: String,
    pub session_seq: i64,
    pub line_index: i64,
    pub block_index: Option<i64>,
    pub ts: i64,
    pub ts_display: String,
    pub received_at: i64,
    pub schema_version: u32,
    pub kind: String,
    pub turn_id: Option<TurnId>,
    pub agent_id: Option<String>,
    pub item_id: Option<String>,
    pub tool_name: Option<String>,
    pub call_id: Option<String>,
    pub is_error: Option<bool>,
    pub inner_kind: Option<String>,
    pub payload: serde_json::Value,
    pub block_payload: Option<serde_json::Value>,
    pub payload_size: usize,
    pub parse_status: String,
    pub preview: Option<String>,
    pub display_text: Option<String>,
    pub display_mode: String,
    pub display_language: String,
    pub role: String,
    pub msg_type: String,
    pub lane: String,
    pub lane_class: String,
    pub action: String,
    pub file_refs: Vec<String>,
    pub searchable: String,
    pub default_open: bool,
}

pub mod event_kind {
    pub const TRANSCRIPT: &str = "transcript";
    pub const METADATA: &str = "metadata";
    pub const UNKNOWN: &str = "unknown";
}

pub mod msg_type {
    pub const METADATA: &str = "metadata";
    pub const REASONING: &str = "reasoning";
    pub const TOOL_RESULT: &str = "tool_result";
    pub const TOOL_USE: &str = "tool_use";
}

pub mod lane {
    pub const EVENT: &str = "event";
    pub const INTENT: &str = "intent";
    pub const MESSAGE: &str = "message";
    pub const METADATA: &str = "metadata";
    pub const READ: &str = "read";
    pub const REASONING: &str = "reasoning";
    pub const SEARCH: &str = "search";
    pub const SHELL: &str = "shell";
    pub const SUBAGENT: &str = "subagent";
    pub const TOOL: &str = "tool";
    pub const TOOL_RESULT: &str = "tool-result";
    pub const WRITE: &str = "write";
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventFilter {
    pub kind: Option<String>,
    pub inner_kind: Option<String>,
    pub tool: Option<String>,
    pub error: Option<bool>,
    pub agent: Option<String>,
    pub msg_type: Option<String>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventQuery {
    pub instance_id: String,
    pub session_id: Option<SessionId>,
    pub before: Option<Cursor>,
    pub limit: usize,
    pub filter: EventFilter,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchQuery {
    pub instance: Option<String>,
    pub session: Option<String>,
    pub agent: Option<String>,
    pub kind: Option<String>,
    pub inner_kind: Option<String>,
    pub tool: Option<String>,
    pub error: Option<bool>,
    pub q: Option<String>,
    #[serde(rename = "from")]
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

impl SearchQuery {
    pub fn filter(&self, from_ms: Option<i64>, to_ms: Option<i64>) -> EventFilter {
        EventFilter {
            kind: self.kind.clone(),
            inner_kind: self.inner_kind.clone(),
            tool: self.tool.clone(),
            error: self.error,
            agent: self.agent.clone(),
            msg_type: None,
            from_ms,
            to_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub event: EventRow,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentEdge {
    pub instance_id: String,
    pub session_id: SessionId,
    pub parent_agent_id: String,
    pub child_agent_id: String,
    pub agent_type: Option<String>,
    pub spawned_at: i64,
    pub completed_at: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    pub ok: bool,
    pub mode: &'static str,
    pub read_only: bool,
    pub ingest_supported: bool,
    pub live_supported: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GoneReason {
    GracefulClose,
    Reset,
    Timeout,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UpsertInstanceOutcome {
    pub first_seen: bool,
    pub previous_last_seen_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct IngestStats {
    pub accepted: usize,
    /// Byte-identical retries of an already-stored `(instance, session, seq)`.
    /// Expected and benign — the connector re-sends unacked batches.
    pub duplicates: usize,
    pub parse_failures: usize,
    /// A different event arriving under an already-stored
    /// `(instance, session, seq)` — a per-session seq regression. Rejected as
    /// corruption rather than overwriting (multi-session plan D-47). Non-zero
    /// here means a producer re-issued a seq without skip-ahead.
    pub rejected_conflicts: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RetentionPolicy {
    pub retention_days: i64,
    pub retention_max_bytes: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SweepStats {
    pub deleted_events: usize,
    pub deleted_sessions: usize,
    pub freed_bytes: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum EventStoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("session error: {0}")]
    Session(#[from] coco_session::SessionError),
    #[error("free_text_not_supported")]
    FreeTextNotSupported,
    #[error("not supported: {0}")]
    NotSupported(&'static str),
    #[error("not found: {0}")]
    NotFound(&'static str),
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    #[error("invalid project directory name: {0}")]
    InvalidProjectDir(std::path::PathBuf),
    #[error("task join error: {0}")]
    TaskJoin(String),
}

impl StackError for EventStoreError {
    fn debug_fmt(&self, layer: usize, buf: &mut Vec<String>) {
        buf.push(format!("{layer}: {self}"));
    }

    fn next(&self) -> Option<&dyn StackError> {
        None
    }
}

impl ErrorExt for EventStoreError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Io { .. } => StatusCode::IoError,
            Self::Json { .. } => StatusCode::InvalidJson,
            Self::Sqlite { .. } => StatusCode::Internal,
            Self::Session { .. } => StatusCode::Internal,
            Self::FreeTextNotSupported | Self::NotSupported(_) => StatusCode::Unsupported,
            Self::NotFound(_) => StatusCode::FileNotFound,
            Self::InvalidQuery(_) => StatusCode::InvalidArguments,
            Self::InvalidProjectDir(_) => StatusCode::InvalidArguments,
            Self::TaskJoin(_) => StatusCode::Internal,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub(crate) fn event_matches_filter(event: &EventRow, filter: &EventFilter) -> bool {
    filter
        .kind
        .as_deref()
        .filter(|value| !value.is_empty())
        .is_none_or(|kind| event.kind == kind)
        && filter
            .inner_kind
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|inner_kind| event.inner_kind.as_deref() == Some(inner_kind))
        && filter
            .tool
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|tool| event.tool_name.as_deref() == Some(tool))
        && filter
            .error
            .is_none_or(|error| event.is_error == Some(error))
        && filter
            .agent
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|agent| event.agent_id.as_deref() == Some(agent))
        && filter
            .msg_type
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|msg_type| {
                event.msg_type == msg_type || event.lane == msg_type || event.role == msg_type
            })
        && filter.from_ms.is_none_or(|from_ms| event.ts >= from_ms)
        && filter.to_ms.is_none_or(|to_ms| event.ts <= to_ms)
}

#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    fn mode(&self) -> &'static str;
    fn source_label(&self) -> String;

    async fn upsert_instance(
        &self,
        _announce: &AnnounceFrame,
    ) -> Result<UpsertInstanceOutcome, EventStoreError> {
        Err(EventStoreError::NotSupported(
            "local session json store is read-only",
        ))
    }

    async fn mark_instance_gone(
        &self,
        _instance_id: &str,
        _reason: GoneReason,
    ) -> Result<(), EventStoreError> {
        Err(EventStoreError::NotSupported(
            "local session json store is read-only",
        ))
    }

    async fn ingest_batch(
        &self,
        _instance_id: &str,
        _batch: BatchFrame,
    ) -> Result<IngestStats, EventStoreError> {
        Err(EventStoreError::NotSupported(
            "local session json store is read-only",
        ))
    }

    async fn list_instances(
        &self,
        params: ListInstancesParams,
    ) -> Result<Page<InstanceRow>, EventStoreError>;
    async fn get_instance(&self, instance_id: &str)
    -> Result<Option<InstanceRow>, EventStoreError>;
    async fn list_sessions(
        &self,
        instance_id: &str,
        params: ListSessionsParams,
    ) -> Result<Page<SessionRow>, EventStoreError>;
    async fn get_session(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, EventStoreError>;
    async fn list_events(&self, query: EventQuery) -> Result<Page<EventRow>, EventStoreError>;
    async fn get_event(
        &self,
        instance_id: &str,
        session_id: &str,
        session_seq: i64,
    ) -> Result<Option<EventRow>, EventStoreError>;
    async fn search(&self, query: SearchQuery) -> Result<Page<SearchHit>, EventStoreError>;
    async fn list_agent_edges(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Vec<AgentEdge>, EventStoreError>;

    async fn run_retention_sweep(
        &self,
        _policy: &RetentionPolicy,
    ) -> Result<SweepStats, EventStoreError> {
        Err(EventStoreError::NotSupported(
            "local session json store has no derived retention state",
        ))
    }

    async fn health(&self) -> Result<HealthSnapshot, EventStoreError>;
}

#[derive(Debug, Clone, Default)]
pub struct ListInstancesParams {
    pub limit: Option<usize>,
    pub cursor: Option<Cursor>,
}

#[derive(Debug, Clone, Default)]
pub struct ListSessionsParams {
    pub limit: Option<usize>,
    pub cursor: Option<Cursor>,
}
