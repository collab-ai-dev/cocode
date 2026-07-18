//! Generic, product-neutral registration metadata for a session slot.
//!
//! `coco-app-server` deliberately knows nothing about product runtime types
//! (see the crate scope boundary). A slot instead carries
//! this immutable policy, which higher layers derive from their own topology: a
//! primary conversation maps to `Root/Public/DurableHub`; a sidechat child maps
//! to `Child/Internal/LocalOnly`. Expressing topology, visibility, and egress
//! as small enums (not booleans) keeps every new policy case compile-time
//! visible. See `docs/internal/sidechat-architecture.md`.

use coco_types::SessionId;

/// Immutable per-slot registration policy, fixed at reservation time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRegistrationPolicy {
    pub topology: SessionTopology,
    pub visibility: SessionVisibility,
    pub egress: SessionEgress,
}

/// Position of a slot in the session graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTopology {
    /// A top-level session with its own durable identity.
    Root,
    /// A child slot owned by `parent`; closed before the parent.
    Child { parent: SessionId },
}

/// Whether public/remote session-data APIs may observe the slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionVisibility {
    /// Present in `session/list` / `session/read` / `session/turns/list`.
    Public,
    /// Hidden from public catalogs and data APIs; operable only through its
    /// validated local interactive handle.
    Internal,
}

/// How the slot's events participate in durable egress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEgress {
    /// Normal replay policy, durable sequence allocation, retention, and Hub
    /// enqueue.
    DurableHub,
    /// Live-only: never allocate a durable `session_seq` and never enqueue to
    /// the Hub. Routed only to attached local surfaces.
    LocalOnly,
}

impl SessionRegistrationPolicy {
    /// Policy for a primary root conversation.
    pub fn root() -> Self {
        Self {
            topology: SessionTopology::Root,
            visibility: SessionVisibility::Public,
            egress: SessionEgress::DurableHub,
        }
    }

    /// Policy for an ephemeral sidechat child of `parent`.
    pub fn side_chat_child(parent: SessionId) -> Self {
        Self {
            topology: SessionTopology::Child { parent },
            visibility: SessionVisibility::Internal,
            egress: SessionEgress::LocalOnly,
        }
    }

    /// Parent slot id, if this is a child.
    pub fn parent(&self) -> Option<&SessionId> {
        match &self.topology {
            SessionTopology::Root => None,
            SessionTopology::Child { parent } => Some(parent),
        }
    }

    /// True when the slot must be hidden from public/remote session-data APIs.
    pub fn is_internal(&self) -> bool {
        matches!(self.visibility, SessionVisibility::Internal)
    }

    /// True when the slot's events are live-only (no durable seq, no Hub).
    pub fn is_local_only(&self) -> bool {
        matches!(self.egress, SessionEgress::LocalOnly)
    }
}

#[cfg(test)]
#[path = "registration_policy.test.rs"]
mod tests;
