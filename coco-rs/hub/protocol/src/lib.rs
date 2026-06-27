use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

pub const SUBPROTOCOL_V1: &str = "coco-event-hub.v1";
pub const SCHEMA_VERSION_V1: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HubFrame {
    Announce(AnnounceFrame),
    AnnounceAck(AnnounceAckFrame),
    Batch(BatchFrame),
    BatchAck(BatchAckFrame),
    Error(ErrorFrame),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnounceFrame {
    pub instance_id: Uuid,
    pub host: String,
    pub cwd: String,
    pub pid: i64,
    pub started_at: DateTime<Utc>,
    pub version: String,
    pub instance_kind: String,
    pub entrypoint: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnnounceAckFrame {
    pub first_seen: bool,
    pub hub_version: String,
    pub resume_from: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatchFrame {
    pub events: Vec<EventEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BatchAckFrame {
    pub up_to_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ErrorFrame {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EventEnvelope {
    pub instance_id: Uuid,
    pub session_id: String,
    pub seq: u64,
    pub ts: DateTime<Utc>,
    pub schema_version: u32,
    pub payload: EventPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventPayload {
    Protocol {
        value: serde_json::Value,
    },
    ToolUseQueued {
        value: serde_json::Value,
    },
    ToolUseStarted {
        value: serde_json::Value,
    },
    ToolUseCompleted {
        value: serde_json::Value,
    },
    McpToolCallBegin {
        value: serde_json::Value,
    },
    McpToolCallEnd {
        value: serde_json::Value,
    },
    TextBlockCompleted {
        value: serde_json::Value,
    },
    ThinkingBlockCompleted {
        value: serde_json::Value,
    },
    EventsDropped {
        count: i64,
        since_seq: u64,
        until_seq: u64,
        reason: String,
    },
    Unknown {
        value: serde_json::Value,
    },
}
