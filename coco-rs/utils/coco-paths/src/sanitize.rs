//! Path-segment sanitization helpers.
//!
//! Two sanitizers:
//! - `sanitize_path` — general-purpose sanitizer used for project paths,
//!   memory base, and any other arbitrary string used as a directory name.
//! - `sanitize_agent_type_for_path` — only the `:` separator (used by
//!   plugin-namespaced agent ids like `my-plugin:my-agent`) is replaced;
//!   the rest of the agent type is already constrained upstream and passes
//!   through untouched.

use crate::djb2::simple_hash;

/// Max length of a sanitized path segment before the djb2 hash suffix
/// kicks in. TS: `MAX_SANITIZED_LENGTH = 200`
/// (`utils/sessionStoragePortable.ts:293`).
pub const MAX_SANITIZED_LENGTH: usize = 200;

/// Sanitize an arbitrary string for use as a single filesystem path
/// segment, mirroring TS `sanitizePath` exactly.
///
/// Algorithm:
/// 1. Iterate the input as UTF-16 code units (matching JS string
///    semantics) and replace every code unit outside `[a-zA-Z0-9]`
///    with `'-'`. A char outside the BMP (e.g. an emoji) therefore
///    becomes two `-` chars — same as JS regex behaviour.
/// 2. If the result has at most [`MAX_SANITIZED_LENGTH`] bytes
///    (which equals char count here since every surviving byte is
///    ASCII), return it.
/// 3. Otherwise truncate to [`MAX_SANITIZED_LENGTH`] and append
///    `-{simple_hash(original_input)}` where `simple_hash` is djb2
///    formatted base36.
///
/// Hash strategy note: TS picks `Bun.hash` on Bun and `simpleHash`
/// (djb2) on Node. We match Node — djb2 is a deterministic pure
/// function that ports cleanly. Long paths therefore round-trip
/// identically between coco-rs and TS-on-Node, but **may diverge
/// from TS-on-Bun**; this is the documented trade-off.
pub fn sanitize_path(name: &str) -> String {
    let mut sanitized = String::with_capacity(name.len());
    for code_unit in name.encode_utf16() {
        if is_ascii_alphanumeric_u16(code_unit) {
            // Safe: alphanumeric ASCII code units round-trip back as
            // single-byte chars.
            sanitized.push(code_unit as u8 as char);
        } else {
            sanitized.push('-');
        }
    }
    if sanitized.len() <= MAX_SANITIZED_LENGTH {
        return sanitized;
    }
    let hash = simple_hash(name);
    // `sanitized` is pure ASCII at this point so byte-slicing is safe.
    let prefix = &sanitized[..MAX_SANITIZED_LENGTH];
    format!("{prefix}-{hash}")
}

/// Sanitize a `plugin:agent` style identifier for use as a directory
/// name. TS: `sanitizeAgentTypeForPath` — only the `:` separator is
/// touched. Other characters pass through (agent type identifiers
/// are already validated upstream).
pub fn sanitize_agent_type_for_path(agent_type: &str) -> String {
    agent_type.replace(':', "-")
}

#[inline]
fn is_ascii_alphanumeric_u16(c: u16) -> bool {
    matches!(c, 0x30..=0x39 | 0x41..=0x5A | 0x61..=0x7A)
}

#[cfg(test)]
#[path = "sanitize.test.rs"]
mod tests;
