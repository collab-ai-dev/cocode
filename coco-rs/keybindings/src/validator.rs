//! Keybinding list validator.
//!
//! Emits typed warnings with severity (`error` / `warning`) across five
//! categories: `parse_error`, `duplicate`, `reserved`, `invalid_context`,
//! `invalid_action`.
//!
//! Works against the JSON shape ([`KeybindingsConfig`]) rather than parsed
//! [`Keybinding`]s — that's the layer where users make mistakes (typos in
//! chord strings, wrong-context bindings, etc.).

use std::collections::HashMap;

use crate::KeybindingAction;
use crate::KeybindingContext;
use crate::KeybindingsConfig;
use crate::parser;
use crate::parser::KeyChord;
use crate::reserved::lookup_reserved;

/// A warning or error about a keybinding configuration issue.
/// `severity` distinguishes "won't load" from "will load but might surprise".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub kind: ValidationKind,
    pub severity: Severity,
    pub message: String,
    pub context: Option<KeybindingContext>,
    /// The chord string from the user's config, when applicable.
    pub chord: Option<String>,
    /// Suggestion text — typically how the user can fix the issue.
    pub suggestion: Option<String>,
}

/// Severity of a [`ValidationIssue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Hard failure: the binding (or the entire config) won't load.
    Error,
    /// Soft warning: the binding loads but probably isn't doing what
    /// the user expected.
    Warning,
}

/// Category of a [`ValidationIssue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationKind {
    /// Chord string failed to parse, JSON shape was wrong, etc.
    ParseError,
    /// Two bindings target the same `(context, chord)` with different
    /// actions; the later one silently wins.
    Duplicate,
    /// Chord targets a shortcut the OS or terminal will intercept
    /// before it reaches the application (filled in by the `reserved`
    /// module — P5).
    Reserved,
    /// Block targets an internal-only context (`Scroll`,
    /// `MessageActions`) or a context unknown to the schema.
    InvalidContext,
    /// `command:foo` bound outside `Chat`, `voice:pushToTalk` bound to
    /// a bare letter, etc.
    InvalidAction,
}

/// Validate a parsed [`KeybindingsConfig`].
///
/// Returns every issue found across all blocks. Empty `Vec` means the
/// config is clean.
pub fn validate(config: &KeybindingsConfig) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let mut seen: HashMap<(KeybindingContext, String), KeybindingAction> = HashMap::new();

    for block in &config.bindings {
        // Reject blocks targeting internal-only contexts when they
        // come from user config.
        if !block.context.is_user_rebindable() {
            issues.push(ValidationIssue {
                kind: ValidationKind::InvalidContext,
                severity: Severity::Error,
                message: format!(
                    "Context `{}` is internal-only and cannot be user-rebound",
                    block.context
                ),
                context: Some(block.context),
                chord: None,
                suggestion: Some(format!(
                    "Use one of: {}",
                    KeybindingContext::ALL_USER
                        .iter()
                        .map(|c| c.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            });
            // Continue inspecting the bindings so we surface every
            // problem in one pass.
        }

        for (chord_str, action) in &block.bindings {
            // Parse error → record and skip the rest of the checks
            // for this entry.
            let chord = match parser::parse_chord(chord_str) {
                Ok(c) => c,
                Err(err) => {
                    issues.push(ValidationIssue {
                        kind: ValidationKind::ParseError,
                        severity: Severity::Error,
                        message: format!("Could not parse chord `{chord_str}`: {err}"),
                        context: Some(block.context),
                        chord: Some(chord_str.clone()),
                        suggestion: None,
                    });
                    continue;
                }
            };

            // Action-shape checks.
            if let Some(action) = action.as_ref()
                && let Some(issue) = check_action_shape(action, chord_str, block.context, &chord)
            {
                issues.push(issue);
            }

            // Reserved-shortcut checks — only flag user-bound chords;
            // `null` unbinds are fine.
            if action.is_some()
                && let Some(reserved) = lookup_reserved(chord_str)
            {
                issues.push(ValidationIssue {
                    kind: ValidationKind::Reserved,
                    severity: reserved.severity,
                    message: format!("`{chord_str}` may not work: {}", reserved.reason),
                    context: Some(block.context),
                    chord: Some(chord_str.clone()),
                    suggestion: None,
                });
            }

            // Duplicate detection — `(context, canonical_chord) →
            // action`. Different action under the same key is a silent-
            // override that should be flagged.
            let canonical = canonical_chord(&chord);
            let key = (block.context, canonical);
            if let Some(prior) = seen.get(&key)
                && Some(prior) != action.as_ref()
            {
                issues.push(ValidationIssue {
                    kind: ValidationKind::Duplicate,
                    severity: Severity::Warning,
                    message: format!(
                        "Duplicate binding `{chord_str}` in {} context",
                        block.context
                    ),
                    context: Some(block.context),
                    chord: Some(chord_str.clone()),
                    suggestion: Some(format!(
                        "Previously bound to `{prior}`. Only the last binding wins."
                    )),
                });
            }
            if let Some(action) = action.as_ref() {
                seen.insert(key, action.clone());
            }
        }
    }

    issues
}

/// Action-shape checks not covered by the type system itself:
/// * `command:foo` must be in `Chat`.
/// * `voice:pushToTalk` bound to a bare letter triggers the
///   warm-up-types-into-input footgun.
fn check_action_shape(
    action: &KeybindingAction,
    chord_str: &str,
    context: KeybindingContext,
    chord: &KeyChord,
) -> Option<ValidationIssue> {
    if action.is_command() && context != KeybindingContext::Chat {
        return Some(ValidationIssue {
            kind: ValidationKind::InvalidAction,
            severity: Severity::Warning,
            message: format!(
                "Command binding `{action}` must be in `Chat` context, not `{context}`"
            ),
            context: Some(context),
            chord: Some(chord_str.to_string()),
            suggestion: Some(
                "Move this binding to a block with `\"context\": \"Chat\"`".to_string(),
            ),
        });
    }

    if matches!(action, KeybindingAction::VoicePushToTalk)
        && let Some(combo) = chord.0.first()
        && chord.0.len() == 1
    {
        // Single-key, no modifiers, lowercase ASCII letter → warn.
        // Parser lowercases at parse time, so an uppercase user-entry has
        // already been normalized.
        let bare_letter = !combo.ctrl
            && !combo.alt
            && !combo.shift
            && !combo.meta
            && !combo.super_key
            && combo.key.chars().count() == 1
            && combo
                .key
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_lowercase());
        if bare_letter {
            return Some(ValidationIssue {
                kind: ValidationKind::InvalidAction,
                severity: Severity::Warning,
                message: format!(
                    "Binding `{chord_str}` to `voice:pushToTalk` prints into the input \
                     during warm-up; use `space` or a modifier combo (e.g. `meta+k`)"
                ),
                context: Some(context),
                chord: Some(chord_str.to_string()),
                suggestion: None,
            });
        }
    }

    None
}

/// Canonical string for duplicate detection. Modifiers in the same
/// fixed order (`ctrl+shift+alt+meta`) so `Ctrl+A` and `ctrl+a` collide.
fn canonical_chord(chord: &KeyChord) -> String {
    chord
        .0
        .iter()
        .map(|c| {
            let mut parts: Vec<&str> = Vec::new();
            if c.ctrl {
                parts.push("ctrl");
            }
            if c.shift {
                parts.push("shift");
            }
            if c.alt {
                parts.push("alt");
            }
            if c.meta {
                parts.push("meta");
            }
            parts.push(c.key.as_str());
            parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Format an issue for terminal display.
///
/// Returns multi-line output: the first line is the headline, the
/// (optional) second line is the suggestion. Use [`format_issue_oneline`]
/// for surfaces (toasts, status bar) that can't render newlines.
pub fn format_issue(issue: &ValidationIssue) -> String {
    let icon = match issue.severity {
        Severity::Error => "✗",
        Severity::Warning => "⚠",
    };
    let mut out = format!(
        "{icon} Keybinding {}: {}",
        match issue.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        },
        issue.message
    );
    if let Some(s) = &issue.suggestion {
        out.push_str("\n  ");
        out.push_str(s);
    }
    out
}

/// Single-line variant of [`format_issue`] for toast / status-bar
/// surfaces. Suggestion (if present) is appended in parentheses.
pub fn format_issue_oneline(issue: &ValidationIssue) -> String {
    let icon = match issue.severity {
        Severity::Error => "✗",
        Severity::Warning => "⚠",
    };
    let label = match issue.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    match &issue.suggestion {
        Some(s) => format!("{icon} Keybinding {label}: {} ({s})", issue.message),
        None => format!("{icon} Keybinding {label}: {}", issue.message),
    }
}

#[cfg(test)]
#[path = "validator.test.rs"]
mod tests;
