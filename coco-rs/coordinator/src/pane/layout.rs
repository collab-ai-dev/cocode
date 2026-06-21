//! Teammate / agent display-color assignment.
//!
//! Each teammate (`name@team`) and each agent type maps to one color from a
//! fixed palette via a **deterministic hash** — stateless, so the same id
//! always yields the same color, in any process, with no shared mutable state
//! to lock, reset, or race.
//!
//! TS uses a stateful round-robin allocator (a `Map` + `colorIndex++`); the
//! hash trades guaranteed first-N distinctness for a pure function. The upside
//! beyond simplicity: a leader and a teammate's own pane compute the *same*
//! color for an id without sharing state (the cross-process lookup in
//! `app/cli::leader_permission`). Color collisions among unrelated agents are
//! possible but purely cosmetic.

use crate::constants::AgentColorName;

/// Color palette — the hash's codomain.
const AGENT_COLORS: &[AgentColorName] = &[
    AgentColorName::Blue,
    AgentColorName::Green,
    AgentColorName::Yellow,
    AgentColorName::Purple,
    AgentColorName::Orange,
    AgentColorName::Pink,
    AgentColorName::Cyan,
    AgentColorName::Red,
];

/// FNV-1a over the key bytes → palette index. Deterministic and stable across
/// processes and runs (fixed offset basis + prime). The `u64` wrapping
/// arithmetic is the hash's defining bit pattern, not a counter.
fn palette_color(key: &str) -> AgentColorName {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    AGENT_COLORS[(hash % AGENT_COLORS.len() as u64) as usize]
}

/// Stable display color for a teammate (`name@team`).
pub fn assign_teammate_color(teammate_id: &str) -> AgentColorName {
    palette_color(teammate_id)
}

/// Display color for a teammate. The mapping is total (every id has a color),
/// so this is always `Some`; the `Option` is kept for call-site ergonomics —
/// callers `.map` it into an `Option<String>` color field.
pub fn get_teammate_color(teammate_id: &str) -> Option<AgentColorName> {
    Some(palette_color(teammate_id))
}

/// Display color for an agent type, so every `Explore` copy renders in the same
/// color regardless of how many run. Pure — the consumer (`presentation`
/// renderers) computes it on demand; nothing needs pre-populating at spawn.
pub fn get_agent_type_color(agent_type: &coco_types::AgentTypeId) -> Option<AgentColorName> {
    Some(palette_color(&agent_type.to_string()))
}

#[cfg(test)]
#[path = "layout.test.rs"]
mod tests;
