//! Claude Code hints protocol — parser + pending-hint store.
//!
//! CLIs and SDKs running under coco can emit a self-closing
//! `<claude-code-hint />` tag to stderr (merged into stdout by the shell
//! tools). The harness scans tool output for these tags, strips them
//! before the output reaches the model, and surfaces an install prompt to
//! the user — no inference, no proactive execution.
//!
//! This module provides both the parser and a small process-level store
//! for the pending hint. The store is a single slot (not a queue) — we
//! surface at most one prompt per session, so there's no reason to
//! accumulate. The TUI polls the snapshot via [`pending_hint_snapshot`].
//!

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;
use serde::Serialize;

/// Hint discriminator. v1 defines only `plugin`.
pub const HINT_TYPE_PLUGIN: &str = "plugin";

/// Spec versions this harness understands.
const SUPPORTED_VERSIONS: &[i64] = &[1];

/// A parsed `<claude-code-hint />` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeCodeHint {
    /// Spec version declared by the emitter. Unknown versions are dropped.
    pub v: i64,
    /// Hint discriminator. v1 defines only `plugin`.
    #[serde(rename = "type")]
    pub hint_type: String,
    /// Hint payload. For `type: "plugin"`: a `name@marketplace` slug
    /// matching the form accepted by `parse_plugin_identifier`.
    pub value: String,
    /// First token of the shell command that produced this hint. Shown in
    /// the install prompt so the user can spot a mismatch between the tool
    /// that emitted the hint and the plugin it recommends.
    pub source_command: String,
}

/// Outer tag match. Anchored to whole lines (multiline mode) so that a
/// hint marker buried in a larger line — e.g. a log statement quoting the
/// tag — is ignored. Leading and trailing whitespace on the line is
/// tolerated since some SDKs pad stderr.
fn hint_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*<claude-code-hint\s+([^>]*?)\s*/>[ \t]*$")
            .unwrap_or_else(|e| unreachable!("static hint-tag regex failed to compile: {e}"))
    })
}

/// Attribute matcher. Accepts `key="value"` and `key=value` (terminated by
/// whitespace or `/>` closing sequence). Values containing whitespace or
/// `"` must use the quoted form. The quoted form does not support escape
/// sequences; raise the spec version if that becomes necessary.
fn attr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(\w+)=(?:"([^"]*)"|([^\s/>]+))"#)
            .unwrap_or_else(|e| unreachable!("static attr regex failed to compile: {e}"))
    })
}

/// Collapse runs of 3+ newlines down to exactly 2.
fn collapse_blank_runs_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\n{3,}")
            .unwrap_or_else(|e| unreachable!("static blank-run regex failed to compile: {e}"))
    })
}

/// Scan shell tool output for hint tags, returning the parsed hints and
/// the output with hint lines removed. The stripped output is what the
/// model sees — hints are a harness-only side channel.
///
/// `output` is the raw command output (stdout with stderr interleaved).
/// `command` is the command that produced it; its first
/// whitespace-separated token is recorded as `source_command`.
pub fn extract_claude_code_hints(output: &str, command: &str) -> (Vec<ClaudeCodeHint>, String) {
    // Fast path: no tag open sequence → no work, no allocation.
    if !output.contains("<claude-code-hint") {
        return (Vec::new(), output.to_string());
    }

    let source_command = first_command_token(command);
    let mut hints = Vec::new();

    // Replace every matched tag line with the empty string, accumulating
    // valid hints.
    let stripped = hint_tag_re().replace_all(output, |caps: &regex::Captures<'_>| {
        let body = caps.get(1).map_or("", |m| m.as_str());
        let attrs = parse_attrs(body);

        let v = attrs.get("v").and_then(|s| s.parse::<i64>().ok());
        let Some(v) = v.filter(|v| SUPPORTED_VERSIONS.contains(v)) else {
            tracing::debug!(
                v = attrs.get("v"),
                "claudeCodeHints: dropped unsupported version"
            );
            return String::new();
        };

        let hint_type = attrs.get("type").cloned().unwrap_or_default();
        if hint_type != HINT_TYPE_PLUGIN {
            tracing::debug!(hint_type, "claudeCodeHints: dropped unsupported type");
            return String::new();
        }

        let value = attrs.get("value").cloned().unwrap_or_default();
        if value.is_empty() {
            tracing::debug!("claudeCodeHints: dropped hint with empty value");
            return String::new();
        }

        hints.push(ClaudeCodeHint {
            v,
            hint_type,
            value,
            source_command: source_command.clone(),
        });
        String::new()
    });

    // Dropping a matched line leaves a blank line (the surrounding newlines
    // remain). Collapse runs of blank lines introduced by the replace so
    // the model-visible output doesn't grow vertical whitespace.
    let stripped_changed = matches!(&stripped, std::borrow::Cow::Owned(_));
    let collapsed = if !hints.is_empty() || stripped_changed {
        collapse_blank_runs_re()
            .replace_all(&stripped, "\n\n")
            .into_owned()
    } else {
        stripped.into_owned()
    };

    (hints, collapsed)
}

/// Parse the attribute body of a tag into a key→value map.
fn parse_attrs(tag_body: &str) -> std::collections::HashMap<String, String> {
    let mut attrs = std::collections::HashMap::new();
    for caps in attr_re().captures_iter(tag_body) {
        let Some(key) = caps.get(1) else { continue };
        // Quoted value (group 2) wins; else unquoted (group 3); else empty.
        let value = caps
            .get(2)
            .or_else(|| caps.get(3))
            .map_or(String::new(), |m| m.as_str().to_string());
        attrs.insert(key.as_str().to_string(), value);
    }
    attrs
}

/// First whitespace-separated token of a command.
fn first_command_token(command: &str) -> String {
    command.split_whitespace().next().unwrap_or("").to_string()
}

// ============================================================================
// Pending-hint store
//
// Single-slot: write wins if the slot is already full (a CLI that emits on
// every invocation would otherwise pile up). The dialog is shown at most
// once per session; after that, `set_pending_hint` becomes a no-op.
//
// Callers should gate before writing (installed? already shown? cap hit?) —
// see `maybe_record_plugin_hint` in marketplace.rs for the plugin-type gate.
// This module stays plugin-agnostic so future hint types can reuse it.
// ============================================================================

struct HintStore {
    pending: Option<ClaudeCodeHint>,
    shown_this_session: bool,
    /// Slugs already gated this session (bounds repeat lookups on the same
    /// slug from a CLI that emits on every invocation).
    tried_this_session: HashSet<String>,
}

fn store() -> &'static Mutex<HintStore> {
    static STORE: OnceLock<Mutex<HintStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(HintStore {
            pending: None,
            shown_this_session: false,
            tried_this_session: HashSet::new(),
        })
    })
}

fn with_store<R>(f: impl FnOnce(&mut HintStore) -> R) -> R {
    let mut guard = store()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(&mut guard)
}

/// Raw store write. Callers should gate first (see module comment).
/// No-op once a dialog has been shown this session.
pub fn set_pending_hint(hint: ClaudeCodeHint) {
    with_store(|s| {
        if s.shown_this_session {
            return;
        }
        s.pending = Some(hint);
    });
}

/// Clear the slot without flipping the session flag — for rejected hints.
pub fn clear_pending_hint() {
    with_store(|s| s.pending = None);
}

/// Flip the once-per-session flag. Call only when a dialog is actually shown.
pub fn mark_shown_this_session() {
    with_store(|s| s.shown_this_session = true);
}

/// Snapshot the pending hint, if any.
pub fn pending_hint_snapshot() -> Option<ClaudeCodeHint> {
    with_store(|s| s.pending.clone())
}

/// Whether a dialog has already been shown this session.
pub fn has_shown_hint_this_session() -> bool {
    with_store(|s| s.shown_this_session)
}

/// Record that this slug was gated; returns `false` if it was already
/// recorded (the caller should drop the hint).
pub(crate) fn record_tried(slug: &str) -> bool {
    with_store(|s| s.tried_this_session.insert(slug.to_string()))
}

/// Test-only reset of the process-level store.
#[cfg(test)]
pub fn reset_store_for_testing() {
    with_store(|s| {
        s.pending = None;
        s.shown_this_session = false;
        s.tried_this_session.clear();
    });
}

#[cfg(test)]
#[path = "hints.test.rs"]
mod tests;
