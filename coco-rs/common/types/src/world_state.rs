//! The model-visible world state: facts about the session the model has been
//! told and should keep believing until they change.
//!
//! Distinct from a system-reminder *nudge*. A nudge is re-issued on a cadence;
//! world state is announced once and then only when it changes. The line:
//!
//! - "a session fact the model should know until it changes" → here
//! - "a prompt we re-issue on a schedule" → a `coco-system-reminder` generator
//!
//! ## Why this is one struct and not per-subsystem fields
//!
//! Every field is a baseline of the same kind: *what has the model already
//! been told?* Keeping them apart cost five hand-wired seams per subsystem
//! (a diff fn, a baseline field, a compaction gate, a `GeneratorContext`
//! field, a generator) and — because the baselines were in-memory only — none
//! of them survived process restart. A resumed session re-announced every
//! inventory on top of a restored history that already contained the
//! announcements. One serializable struct persisted beside the transcript
//! fixes that once instead of five times.
//!
//! ## Comparison data only
//!
//! Fields hold the smallest value that can decide "does the model still know
//! this?". Anything whose full text would be large is stored as a
//! [`ContentDigest`], never the text, so one snapshot stays a few hundred
//! bytes and can be written whole on every change — no merge patches, and no
//! replay ordering to get wrong.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;

/// Content fingerprint standing in for text too large to keep in a snapshot.
///
/// Equality is the only operation — the digest exists to answer "is this the
/// same text the model was shown?" and nothing else. Hex-encoded SHA-256 so
/// the persisted form is stable across processes and Rust versions
/// (`DefaultHasher` is neither).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentDigest(String);

impl ContentDigest {
    pub fn of(content: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        Self(format!("{:x}", hasher.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One MCP server as the model was last told about it. Tool count and
/// description are part of the comparison so a reconnect that changes either
/// is re-announced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerAnnouncementState {
    pub tool_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// What the model has been told, for one visibility scope.
///
/// Scope matters: the main session and each subagent see different tool sets
/// and agent catalogs, so one shared baseline would make a subagent's first
/// turn look like the main session's tools had been removed. See
/// [`ToolAppState::world_state_for_scope`](crate::ToolAppState::world_state_for_scope).
///
/// Ordered collections throughout, so `PartialEq` is the change test and the
/// serialized form is byte-stable — which is what lets the transition tests be
/// snapshots and "did anything change?" be one comparison.
///
/// Listed in `scripts/check-live-fields.sh`: because this derives `Serialize`,
/// the guard requires every field to be *constructed* somewhere outside this
/// file. A section added here but never wired into the diff fails
/// `just quick-check` instead of silently staying `None` forever.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStateSnapshot {
    /// Model id the model was last told it is running as. Identity only — the
    /// model's own instruction text is never stored here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Tool wire-names announced as reachable through `ToolSearch` but not
    /// directly callable.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub deferred_tools: BTreeSet<String>,

    /// Agent types announced for the `Agent` tool.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub agent_types: BTreeSet<String>,

    /// Connected MCP servers announced as discoverable.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_servers: BTreeMap<String, McpServerAnnouncementState>,

    /// Server name → digest of the instructions block the model was shown.
    /// Digests, not bodies: a handful of servers can carry several KB of
    /// instructions each, and this record is rewritten on every change.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_instruction_digests: BTreeMap<String, ContentDigest>,
}

impl WorldStateSnapshot {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
#[path = "world_state.test.rs"]
mod tests;
