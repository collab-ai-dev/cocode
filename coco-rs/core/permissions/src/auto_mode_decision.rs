//! Auto-mode classifier decision integration.
//!
//! Entry point for auto-mode permission checks. Called by the query engine
//! after `PermissionEvaluator::evaluate()` returns `Ask` and the mode is
//! `Auto`. Chains: safe-tool check → heuristic → LLM classifier → denial
//! tracking.

use coco_messages::Message;
use coco_tool_runtime::DenialTracker;
use coco_types::PermissionAbortReason;
use coco_types::PermissionDecision;
use coco_types::PermissionDecisionReason;
use coco_types::ToolName;
use serde_json::Value;
use std::future::Future;

use crate::auto_mode::AutoModeDecision;
use crate::auto_mode::classify_for_auto_mode;
use crate::classifier::AutoModeRules;
use crate::classifier::ClassifyRequest;
use crate::classifier::InputProjector;
use crate::classifier::classify_yolo_action;
use crate::classifier::is_safe_tool;
use crate::evaluate::extract_file_modifying_path;
use crate::evaluate::is_file_modifying_tool;
use crate::filesystem;
use crate::filesystem::PathSafetyResult;
use crate::web_preapproved::is_preapproved_webfetch_url;

/// `decisionReason.classifier` tag for auto-mode classifier decisions.
const AUTO_MODE_CLASSIFIER: &str = "auto_mode";

/// Caller-supplied context needed to honor path-safety immunity, the
/// safe-in-cwd fast path, and headless fail-closed behavior.
#[derive(Debug, Clone, Copy)]
pub struct AutoModeContext<'a> {
    /// Effective working directory. Enables the "safe edit inside an allowed
    /// directory" fast path and symlink resolution for path safety. `None`
    /// (test / unknown cwd) disables the fast path — edits go to the
    /// classifier instead of being auto-allowed.
    pub cwd: Option<&'a str>,
    /// Additional writable roots (keys of `ToolPermissionContext.additional_dirs`).
    pub additional_dirs: &'a [String],
    /// True when the session cannot show an interactive prompt. When set, decisions
    /// that would otherwise fall back to an interactive Ask instead DENY — a
    /// headless Ask is auto-allowed downstream, so falling through would
    /// defeat the safety check. Distinct from session-level
    /// `is_non_interactive` (which drives side effects, not permissions).
    pub avoid_permission_prompts: bool,
}

/// Attempt auto-mode classification for a tool call.
///
/// Returns `None` when auto-mode is inactive (caller falls through to the
/// interactive permission dialog). Returns `Some(decision)` when auto-mode
/// handled the request.
//
// Each parameter carries a distinct piece of caller-side state (rules,
// state machines, denial cache, classifier callback). Bundling them
// would just rename the noise — kept individual to preserve clarity.
#[allow(clippy::too_many_arguments)]
pub async fn can_use_tool_in_auto_mode<M, F, Fut>(
    tool_name: &str,
    input: &Value,
    is_read_only: bool,
    auto_active: bool,
    denial_tracker: &mut DenialTracker,
    messages: &[M],
    rules: &AutoModeRules,
    auto_ctx: &AutoModeContext<'_>,
    classify_fn: F,
    projector: Option<InputProjector<'_>>,
) -> Option<PermissionDecision>
where
    M: std::borrow::Borrow<Message>,
    F: Fn(ClassifyRequest) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    // 1. Not auto for THIS call → None (fallthrough to interactive). `auto_active`
    //    is the per-call decision the caller derives from the permission
    //    context mode (TS parity), NOT a shared session-global flag — so a
    //    concurrent engine build can't race the classifier off for a subagent.
    if !auto_active {
        tracing::debug!(
            tool_name,
            "auto-mode classify gate inactive for this call — skipping classifier, prompt stands",
        );
        return None;
    }

    // 2. Safe tool allowlist → Allow immediately (skip classifier). Any
    //    allowed action also clears the consecutive-denial streak so a few
    //    safe calls between blocks don't trip the fallback.
    if is_safe_tool(tool_name) {
        denial_tracker.reset_consecutive();
        return Some(allow());
    }

    // 3. File-modifying tools: path-safety immunity + safe-in-cwd fast path.
    //    Replaces the old "relative or /tmp → allow" heuristic that
    //    auto-allowed CWD-escaping traversal and overrode non-classifier-
    //    approvable safety blocks.
    if is_file_modifying_tool(tool_name) {
        if let Some(path) = extract_file_modifying_path(tool_name, input) {
            match file_safety_decision(&path, auto_ctx) {
                FileSafety::Immune { message } => {
                    return Some(require_interactive_or_deny(
                        message.clone(),
                        PermissionDecisionReason::SafetyCheck {
                            reason: message,
                            classifier_approvable: false,
                        },
                        auto_ctx.avoid_permission_prompts,
                    ));
                }
                FileSafety::AllowInCwd => {
                    denial_tracker.reset_consecutive();
                    return Some(allow());
                }
                // Classifier-approvable block or outside-cwd safe path →
                // let the LLM classifier decide (fall through).
                FileSafety::Classify => {}
            }
        }
    } else if tool_name == ToolName::WebFetch.as_str()
        && input
            .get("url")
            .and_then(Value::as_str)
            .is_some_and(is_preapproved_webfetch_url)
    {
        denial_tracker.reset_consecutive();
        return Some(allow());
    } else if classify_for_auto_mode(tool_name, input, is_read_only) == AutoModeDecision::Allow {
        // 4. Non-file heuristic fast path (read-only tools, task/plan tools,
        //    read-only Bash). Allowed → clear the streak.
        denial_tracker.reset_consecutive();
        return Some(allow());
    }

    // 5. LLM classifier (two-stage XML)
    let result =
        classify_yolo_action(messages, tool_name, input, rules, classify_fn, projector).await;

    // 6. Classifier could not produce a usable verdict.
    //    Transcript-too-long is deterministic (retry can't help) → manual
    //    approval, or abort the turn in headless.
    if result.transcript_too_long {
        let message = "Auto-mode classifier transcript exceeded the context window — \
             manual approval required"
            .to_string();
        if auto_ctx.avoid_permission_prompts {
            return Some(PermissionDecision::Abort {
                message,
                reason: PermissionAbortReason::ClassifierTranscriptTooLong,
            });
        }
        return Some(PermissionDecision::Ask {
            message,
            suggestions: Vec::new(),
            choices: None,
            detail: None,
        });
    }
    //    Transient outage. Default posture is fail closed (deny even in
    //    interactive mode). The `classifier_unavailable_fail_open` setting
    //    (default false = fail closed) can opt into fail-open behavior,
    //    which restores a manual prompt when interactive; headless always
    //    denies regardless.
    if result.unavailable {
        let avoid_prompts =
            !rules.classifier_unavailable_fail_open || auto_ctx.avoid_permission_prompts;
        return Some(require_interactive_or_deny(
            format!(
                "Auto-mode classifier unavailable ({}) — manual approval required",
                result.reason
            ),
            PermissionDecisionReason::Classifier {
                classifier: AUTO_MODE_CLASSIFIER.into(),
                reason: "unavailable".into(),
            },
            avoid_prompts,
        ));
    }

    // 7. Interpret the verdict.
    if result.should_block {
        denial_tracker.record_denial(tool_name);

        // Denial-limit fallback: 3 consecutive OR 20 total denials → let the
        // user review (or abort the turn in headless).
        if denial_tracker.should_fallback_to_prompting() {
            let warning = denial_limit_warning(denial_tracker, &result.reason);
            // Reset both counters on the total cap so the session continues
            // past one review prompt instead of denying forever.
            if denial_tracker.hit_total_limit() {
                denial_tracker.reset_after_total_limit();
            }
            if auto_ctx.avoid_permission_prompts {
                return Some(PermissionDecision::Abort {
                    message: warning,
                    reason: PermissionAbortReason::ClassifierDenialLimit,
                });
            }
            return Some(require_interactive_or_deny(
                warning.clone(),
                PermissionDecisionReason::Classifier {
                    classifier: AUTO_MODE_CLASSIFIER.into(),
                    reason: warning,
                },
                auto_ctx.avoid_permission_prompts,
            ));
        }

        Some(PermissionDecision::Deny {
            message: result.reason,
            reason: PermissionDecisionReason::Classifier {
                classifier: AUTO_MODE_CLASSIFIER.into(),
                reason: format!("stage={} model={}", result.stage.unwrap_or(0), result.model),
            },
        })
    } else {
        denial_tracker.reset_consecutive();
        Some(allow())
    }
}

/// A plain auto-mode allow with no input rewrite.
fn allow() -> PermissionDecision {
    PermissionDecision::Allow {
        updated_input: None,
        feedback: None,
    }
}

/// Map a "needs human review" outcome to a decision: an interactive Ask
/// normally, but a Deny when no interactive prompt is reachable (an Ask would
/// be silently auto-allowed downstream).
fn require_interactive_or_deny(
    message: String,
    reason: PermissionDecisionReason,
    avoid_permission_prompts: bool,
) -> PermissionDecision {
    if avoid_permission_prompts {
        PermissionDecision::Deny { message, reason }
    } else {
        PermissionDecision::Ask {
            message,
            suggestions: Vec::new(),
            choices: None,
            detail: None,
        }
    }
}

/// Build the denial-limit warning. Reads counter values
/// before any reset.
fn denial_limit_warning(tracker: &DenialTracker, latest_reason: &str) -> String {
    let lead = if tracker.hit_total_limit() {
        format!(
            "{} actions were blocked this session.",
            tracker.total_denials
        )
    } else {
        format!(
            "{} consecutive actions were blocked.",
            tracker.consecutive_denials
        )
    };
    format!(
        "{lead} Please review the transcript before continuing.\n\nLatest blocked action: {latest_reason}"
    )
}

/// Outcome of the file-modifying path-safety gate.
enum FileSafety {
    /// Non-classifier-approvable safety block — never auto-allow, never
    /// classify (immune to all auto-approve paths).
    Immune { message: String },
    /// Path passed all safety checks and lands inside an allowed directory.
    AllowInCwd,
    /// Defer to the LLM classifier.
    Classify,
}

/// One path-safety scan outcome.
enum Scan {
    /// Non-classifier-approvable block (immune to all auto-approve paths).
    Immune(String),
    /// Classifier-approvable block — needs the LLM to decide.
    Approvable,
    /// Passed all checks.
    Safe,
}

fn scan_path(candidate: &str) -> Scan {
    match filesystem::check_path_safety_for_auto_edit(candidate) {
        PathSafetyResult::Blocked {
            message,
            classifier_approvable: false,
        } => Scan::Immune(message),
        PathSafetyResult::Blocked {
            classifier_approvable: true,
            ..
        } => Scan::Approvable,
        PathSafetyResult::Safe => Scan::Safe,
    }
}

/// Decide a file-modifying tool's auto-mode fate from path safety + cwd.
///
/// Scans the RAW path first — lexical attacks (`..` traversal, `$VAR` /
/// backtick shell expansion, `~user` tilde, NTFS ADS) are erased by path
/// normalization, so they must be caught before resolution. Then, when the
/// cwd is known, scans every symlink-resolved target so a symlink to a
/// protected file is caught. An immune
/// block on ANY candidate dominates.
fn file_safety_decision(path: &str, ctx: &AutoModeContext<'_>) -> FileSafety {
    let mut candidates: Vec<String> = vec![path.to_string()];
    if let Some(cwd) = ctx.cwd {
        candidates.extend(filesystem::get_paths_for_permission_check(path, cwd));
    }

    let mut needs_classifier = false;
    for candidate in &candidates {
        match scan_path(candidate) {
            Scan::Immune(message) => return FileSafety::Immune { message },
            Scan::Approvable => needs_classifier = true,
            Scan::Safe => {}
        }
    }

    if needs_classifier {
        return FileSafety::Classify;
    }
    // Path passed every safety check. Auto-allow only when it lands inside an
    // allowed directory; otherwise route to the classifier.
    match ctx.cwd {
        Some(cwd) if filesystem::is_path_within_allowed_dirs(path, cwd, ctx.additional_dirs) => {
            FileSafety::AllowInCwd
        }
        _ => FileSafety::Classify,
    }
}

#[cfg(test)]
#[path = "auto_mode_decision.test.rs"]
mod tests;
