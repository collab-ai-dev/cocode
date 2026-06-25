//! Implicit session-team bootstrap.
//!
//! A non-teammate (leader) session deterministically owns exactly one
//! team, created at CLI startup rather than by a model tool call. The
//! team is named `session-<sessionId[:8]>`, with the leader as the sole
//! member. Teammate spawning then routes on the runtime-owned team
//! context existing, not on a model-supplied `team_name` parameter.
//!
//! This mirrors the upstream `initializeSessionTeam`: it is idempotent
//! (a resumed session that already wrote its team file short-circuits),
//! and the call site swallows errors with a `tracing::warn!` so a
//! swarm-init failure never blocks REPL boot.

/// Prefix for the deterministic implicit session-team name.
pub const SESSION_TEAM_PREFIX: &str = "session";

/// Derive the deterministic implicit team name for a session id:
/// `session-<sessionId[:8]>`. Short ids (< 8 chars) are used whole. The
/// 8-char prefix is taken by `char` so a multibyte boundary can never
/// panic (session ids are ASCII UUIDs, but the slice stays safe).
pub fn session_team_name(session_id: &str) -> String {
    let prefix: String = session_id.chars().take(8).collect();
    format!("{SESSION_TEAM_PREFIX}-{prefix}")
}

#[cfg(test)]
#[path = "session_team.test.rs"]
mod tests;
