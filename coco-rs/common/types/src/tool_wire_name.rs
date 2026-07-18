//! Provider-facing tool-name handle.
//!
//! A [`WireToolName`] is the string a model sees and calls. It is a **pure
//! function of [`ToolId`]** ([`WireToolName::for_tool_id`]): built-in and custom
//! tools use their canonical name when it is provider-safe; MCP tools use the readable
//! `mcp__<server>__<tool>` spelling when it satisfies the strictest provider
//! constraint. Invalid or overlong names fall back to a deterministic,
//! namespace-preserving truncated-prefix + hash handle. Determinism guarantees
//! the same tool always maps
//! to the same wire name across turns — the property prompt-cache stability and
//! Google function-response name pairing both rely on.
//!
//! No execution path parses a wire name back into a `ToolId`; the registry owns
//! the `WireToolName <-> ToolId` mapping, so the historical `__`-in-components
//! ambiguity does not arise here.

use crate::MCP_TOOL_PREFIX;
use crate::MCP_TOOL_SEPARATOR;
use crate::ToolId;
use sha2::{Digest, Sha256};
use std::borrow::Borrow;
use std::fmt;

/// Strictest provider function-name length (OpenAI / Google: 64 bytes).
pub const MAX_WIRE_TOOL_NAME_BYTES: usize = 64;

/// Validated provider-facing tool name.
///
/// Produced only from a [`ToolId`] via [`WireToolName::for_tool_id`]. The inner
/// string always satisfies the provider charset (`[A-Za-z0-9_-]`) and length
/// (`MAX_WIRE_TOOL_NAME_BYTES`) constraints.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WireToolName(String);

impl WireToolName {
    /// Deterministic provider-facing name for a tool identity.
    ///
    /// Every identity keeps its canonical name when it fits the provider
    /// constraint, otherwise it falls back to a deterministic hashed handle.
    pub fn for_tool_id(id: &ToolId) -> Self {
        let natural = id.to_string();
        if is_wire_valid(&natural) {
            return Self(natural);
        }
        let readable = match id {
            ToolId::Builtin(name) => name.as_str(),
            ToolId::Custom(name) => name,
            ToolId::Mcp { tool, .. } => tool,
        };
        Self(hashed_handle(readable, id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

/// A name is wire-safe if it is non-empty, within the length budget, and uses
/// only the provider-allowed charset.
fn is_wire_valid(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_WIRE_TOOL_NAME_BYTES
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `<namespace><sanitized-truncated-prefix>__<hash16>`, guaranteed `<= 64`
/// bytes. MCP handles keep `mcp__`; other identities use `tool__` so search and
/// policy code cannot misclassify a malformed custom name as MCP.
///
/// The suffix is a truncated SHA-256 digest of the full `ToolId` string, so distinct
/// servers with the same bare tool name (`github`/`gitlab` `create_issue`) get
/// distinct handles. The readable prefix is sanitized to ASCII and truncated to
/// whatever budget remains.
fn hashed_handle(readable: &str, id: &ToolId) -> String {
    let hash = stable_hash_hex(&id.to_string());
    let namespace = if matches!(id, ToolId::Mcp { .. }) {
        MCP_TOOL_PREFIX
    } else {
        "tool__"
    };
    let fixed = namespace.len() + MCP_TOOL_SEPARATOR.len() + hash.len();
    let prefix_budget = MAX_WIRE_TOOL_NAME_BYTES.saturating_sub(fixed);
    // Each retained char is ASCII (1 byte), so `take(prefix_budget)` bounds the
    // byte length without any char-boundary hazard.
    let prefix: String = readable
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(prefix_budget)
        .collect();
    format!("{namespace}{prefix}{MCP_TOOL_SEPARATOR}{hash}")
}

/// First 64 bits of SHA-256 as lowercase hex. The registry still rejects an
/// actual collision, while the collision-resistant suffix prevents an MCP
/// server from cheaply manufacturing one and denying another registration.
fn stable_hash_hex(input: &str) -> String {
    let digest = Sha256::digest(input.as_bytes());
    let mut output = String::with_capacity(16);
    for byte in &digest[..8] {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        _ => char::from(b'a' + (nibble - 10)),
    }
}

impl fmt::Display for WireToolName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for WireToolName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for WireToolName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
#[path = "tool_wire_name.test.rs"]
mod tests;
