use super::*;
use crate::rtk::detect::RtkVersion;

// ── rr-rtk prefix fixup (pure) ──────────────────────────────────────────────

#[test]
fn fixup_simple_prefix() {
    assert_eq!(
        fixup_rr_rtk_prefixes("rtk git status").as_deref(),
        Some("rr-rtk git status")
    );
}

#[test]
fn fixup_compound_and_or() {
    // Both segments were prefixed by the engine.
    assert_eq!(
        fixup_rr_rtk_prefixes("rtk git status && rtk cargo test").as_deref(),
        Some("rr-rtk git status && rr-rtk cargo test")
    );
}

#[test]
fn fixup_pipe_only_left_prefixed() {
    // For a pipe, rtk rewrites only the left command; the right stays raw.
    assert_eq!(
        fixup_rr_rtk_prefixes("rtk grep foo | head").as_deref(),
        Some("rr-rtk grep foo | head")
    );
}

#[test]
fn fixup_ignores_separator_inside_quotes() {
    // A `;` inside a quoted commit message must not split the segment.
    assert_eq!(
        fixup_rr_rtk_prefixes(r#"rtk git commit -m "a; b""#).as_deref(),
        Some(r#"rr-rtk git commit -m "a; b""#)
    );
}

#[test]
fn fixup_mixed_prefixed_and_raw_segments() {
    // Second segment is a raw command the engine left untouched.
    assert_eq!(
        fixup_rr_rtk_prefixes("rtk git status ; echo done").as_deref(),
        Some("rr-rtk git status ; echo done")
    );
}

#[test]
fn fixup_utf8_safe_in_arguments() {
    assert_eq!(
        fixup_rr_rtk_prefixes("rtk git commit -m \"日本語 — ok\"").as_deref(),
        Some("rr-rtk git commit -m \"日本語 — ok\"")
    );
}

#[test]
fn fixup_no_prefix_is_shape_mismatch() {
    // A rewrite (exit 0) that carries no `rtk` head is a shape we can't
    // account for → None → ShapeMismatch passthrough.
    assert_eq!(fixup_rr_rtk_prefixes("git status"), None);
    assert_eq!(fixup_rr_rtk_prefixes("rtkfoo bar"), None);
}

#[test]
fn fixup_does_not_touch_rtk_in_non_head_position() {
    // `rtk` appearing as an argument (not a segment head) is left alone.
    assert_eq!(
        fixup_rr_rtk_prefixes("rtk cat rtk-notes.txt").as_deref(),
        Some("rr-rtk cat rtk-notes.txt")
    );
}

#[test]
fn fixup_escaped_quote_keeps_quoted_separator_inert() {
    // `\"` must NOT close the double quote, so the `;` stays inside the string
    // and the literal `rtk foo` in the argument is not rewritten (regression:
    // a naive quote toggle desynced here and corrupted the quoted text).
    assert_eq!(
        fixup_rr_rtk_prefixes(r#"rtk git commit -m "fix\" thing; rtk foo""#).as_deref(),
        Some(r#"rr-rtk git commit -m "fix\" thing; rtk foo""#)
    );
}

#[test]
fn fixup_newline_separator_fixes_each_segment() {
    // A newline is a top-level separator, so every segment head gets fixed —
    // else the second `rtk` would execute as a missing binary.
    assert_eq!(
        fixup_rr_rtk_prefixes("rtk grep foo\nrtk cat bar").as_deref(),
        Some("rr-rtk grep foo\nrr-rtk cat bar")
    );
}

#[test]
fn fixup_escaped_separator_outside_quotes_is_literal() {
    // `\;` is an escaped literal (a `find -exec` terminator), not a segment
    // separator — the trailing `rtk` is an argument, not a new command.
    assert_eq!(
        fixup_rr_rtk_prefixes(r"rtk find . -exec foo \; rtk bar").as_deref(),
        Some(r"rr-rtk find . -exec foo \; rtk bar")
    );
}

// ── exit-code mapping (pure classifier) ─────────────────────────────────────

use crate::rtk::detect::RtkFlavor;

#[test]
fn map_exit_0_returns_rewrite() {
    assert_eq!(
        map_exit_result(Some(0), b"rtk git status\n", RtkFlavor::Rtk),
        RewriteOutcome::Rewritten("rtk git status".to_string())
    );
}

#[test]
fn map_exit_3_returns_rewrite() {
    // Exit 3 = "rewrite, host should still prompt"; coco already ran its own
    // permission engine, so it's treated identically to exit 0.
    assert_eq!(
        map_exit_result(Some(3), b"rtk git diff\n", RtkFlavor::Rtk),
        RewriteOutcome::Rewritten("rtk git diff".to_string())
    );
}

#[test]
fn map_exit_1_is_no_equivalent() {
    assert_eq!(
        map_exit_result(Some(1), b"", RtkFlavor::Rtk),
        RewriteOutcome::Passthrough(PassthroughReason::NoEquivalent)
    );
}

#[test]
fn map_exit_2_is_host_deny() {
    assert_eq!(
        map_exit_result(Some(2), b"", RtkFlavor::Rtk),
        RewriteOutcome::Passthrough(PassthroughReason::HostDeny)
    );
}

#[test]
fn map_exit_0_empty_stdout_is_no_equivalent() {
    assert_eq!(
        map_exit_result(Some(0), b"   \n", RtkFlavor::Rtk),
        RewriteOutcome::Passthrough(PassthroughReason::NoEquivalent)
    );
}

#[test]
fn map_unknown_exit_is_no_equivalent() {
    // Killed by signal (no exit code) or an out-of-contract code.
    assert_eq!(
        map_exit_result(None, b"whatever", RtkFlavor::Rtk),
        RewriteOutcome::Passthrough(PassthroughReason::NoEquivalent)
    );
    assert_eq!(
        map_exit_result(Some(42), b"whatever", RtkFlavor::Rtk),
        RewriteOutcome::Passthrough(PassthroughReason::NoEquivalent)
    );
}

#[test]
fn map_rr_rtk_flavor_fixes_prefix() {
    assert_eq!(
        map_exit_result(Some(0), b"rtk git status\n", RtkFlavor::RrRtk),
        RewriteOutcome::Rewritten("rr-rtk git status".to_string())
    );
}

#[test]
fn map_rr_rtk_shape_mismatch_passes_through() {
    // Exit 0 asserts a rewrite, but no `rtk` head → can't account for it.
    assert_eq!(
        map_exit_result(Some(0), b"git status\n", RtkFlavor::RrRtk),
        RewriteOutcome::Passthrough(PassthroughReason::ShapeMismatch)
    );
}

// ── subprocess paths (deterministic; no freshly-written scripts) ─────────────

#[tokio::test]
async fn run_rewrite_spawn_error_passes_through() {
    use crate::rtk::detect::RtkBinary;
    let binary = RtkBinary {
        path: "/nonexistent/path/to/rtk".into(),
        flavor: RtkFlavor::Rtk,
        version: RtkVersion {
            major: 0,
            minor: 42,
            patch: 4,
        },
    };
    assert_eq!(
        run_rewrite(&binary, "git status", 500).await,
        RewriteOutcome::Passthrough(PassthroughReason::SpawnError)
    );
}
