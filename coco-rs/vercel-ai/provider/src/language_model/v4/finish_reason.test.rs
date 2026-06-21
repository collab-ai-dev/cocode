use super::*;

#[test]
fn default_is_end_turn() {
    let reason = UnifiedFinishReason::default();
    assert!(reason.is_normal());
    assert_eq!(reason, UnifiedFinishReason::EndTurn);
}

#[test]
fn is_normal_covers_happy_path() {
    assert!(UnifiedFinishReason::EndTurn.is_normal());
    assert!(UnifiedFinishReason::StopSequence.is_normal());
    assert!(UnifiedFinishReason::ToolUse.is_normal());

    for abnormal in [
        UnifiedFinishReason::MaxTokens,
        UnifiedFinishReason::ContextWindowExceeded,
        UnifiedFinishReason::ContentFilter,
        UnifiedFinishReason::Error,
        UnifiedFinishReason::Other,
    ] {
        assert!(!abnormal.is_normal(), "{abnormal:?} must be abnormal");
        assert!(abnormal.is_abnormal());
    }
}

#[test]
fn display_uses_snake_case_wire_format() {
    assert_eq!(format!("{}", UnifiedFinishReason::EndTurn), "end_turn");
    assert_eq!(
        format!("{}", UnifiedFinishReason::StopSequence),
        "stop_sequence"
    );
    assert_eq!(format!("{}", UnifiedFinishReason::ToolUse), "tool_use");
    assert_eq!(format!("{}", UnifiedFinishReason::MaxTokens), "max_tokens");
    assert_eq!(
        format!("{}", UnifiedFinishReason::ContextWindowExceeded),
        "model_context_window_exceeded"
    );
    assert_eq!(
        format!("{}", UnifiedFinishReason::ContentFilter),
        "content_filter"
    );
    assert_eq!(format!("{}", UnifiedFinishReason::Error), "error");
    assert_eq!(format!("{}", UnifiedFinishReason::Other), "other");
}

#[test]
fn serde_round_trip_uses_snake_case() {
    let pairs = [
        (UnifiedFinishReason::EndTurn, r#""end_turn""#),
        (UnifiedFinishReason::StopSequence, r#""stop_sequence""#),
        (UnifiedFinishReason::ToolUse, r#""tool_use""#),
        (UnifiedFinishReason::MaxTokens, r#""max_tokens""#),
        (
            UnifiedFinishReason::ContextWindowExceeded,
            r#""model_context_window_exceeded""#,
        ),
        (UnifiedFinishReason::ContentFilter, r#""content_filter""#),
        (UnifiedFinishReason::Error, r#""error""#),
        (UnifiedFinishReason::Other, r#""other""#),
    ];
    for (variant, expected) in pairs {
        let json = serde_json::to_string(&variant).unwrap();
        assert_eq!(json, expected, "serialize {variant:?}");
        let round: UnifiedFinishReason = serde_json::from_str(&json).unwrap();
        assert_eq!(round, variant, "round-trip {variant:?}");
    }
}

#[test]
fn finish_reason_constructors() {
    assert_eq!(
        FinishReason::end_turn().unified,
        UnifiedFinishReason::EndTurn
    );
    assert_eq!(
        FinishReason::tool_use().unified,
        UnifiedFinishReason::ToolUse
    );
    assert_eq!(FinishReason::error().unified, UnifiedFinishReason::Error);
    assert_eq!(FinishReason::other().unified, UnifiedFinishReason::Other);
}

#[test]
fn finish_reason_with_raw_preserves_provenance() {
    let reason = FinishReason::with_raw(UnifiedFinishReason::ContentFilter, "refusal");
    assert!(reason.is_abnormal());
    assert_eq!(reason.unified, UnifiedFinishReason::ContentFilter);
    assert_eq!(reason.raw.as_deref(), Some("refusal"));
}

#[test]
fn finish_reason_serializes_as_bare_unified_string() {
    // Wire is shielded: a `FinishReason` serializes as just the bare
    // `unified` snake_case string — NOT a `{unified, raw}` object — so
    // transcripts / SDK payloads keep their pre-existing format and
    // old string-form transcripts still deserialize.
    let reason = FinishReason::with_raw(UnifiedFinishReason::Other, "compaction");
    let json = serde_json::to_string(&reason).unwrap();
    assert_eq!(json, r#""other""#, "raw must not leak onto the wire");

    // Round-trip recovers `unified`; `raw` is a live diagnostic, not
    // persisted, so it is `None` after deserialize.
    let parsed: FinishReason = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.unified, UnifiedFinishReason::Other);
    assert!(parsed.raw.is_none());

    // A bare string (old transcript form) deserializes cleanly too.
    let from_old: FinishReason = serde_json::from_str(r#""end_turn""#).unwrap();
    assert_eq!(from_old.unified, UnifiedFinishReason::EndTurn);
    assert!(from_old.raw.is_none());
}

#[test]
fn finish_reason_from_unified() {
    let reason: FinishReason = UnifiedFinishReason::ToolUse.into();
    assert_eq!(reason.unified, UnifiedFinishReason::ToolUse);
    assert!(reason.raw.is_none());
}

#[test]
fn finish_reason_display_annotates_raw_when_informative() {
    // Raw that the projection drops → surfaced in parens for log audit.
    assert_eq!(
        FinishReason::with_raw(UnifiedFinishReason::Other, "compaction").to_string(),
        "other(compaction)"
    );
    assert_eq!(
        FinishReason::with_raw(UnifiedFinishReason::ContentFilter, "refusal").to_string(),
        "content_filter(refusal)"
    );
    // Raw equal to the unified wire string adds nothing → no parens.
    assert_eq!(
        FinishReason::with_raw(UnifiedFinishReason::EndTurn, "end_turn").to_string(),
        "end_turn"
    );
    // Synthetic (no provider raw) → bare unified wire string.
    assert_eq!(FinishReason::tool_use().to_string(), "tool_use");
}
