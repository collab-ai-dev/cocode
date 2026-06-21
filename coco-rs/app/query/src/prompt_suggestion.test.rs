use super::*;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::StopReason;
use coco_messages::TextContent;
use coco_types::TokenUsage;
use uuid::Uuid;

#[test]
fn test_system_prompt_byte_faithful_with_ts() {
    // Length pin guards against accidental drift.
    let prompt = SUGGESTION_PROMPT;
    assert!(
        prompt.starts_with(
            "[SUGGESTION MODE: Suggest what the user might naturally type next into Claude Code.]"
        ),
        "first line must match TS"
    );
    assert!(prompt.contains("2-12 words"));
    assert!(prompt.contains("Be specific:"));
    assert!(prompt.contains("NEVER SUGGEST:"));
    assert!(prompt.contains("Claude-voice"));
    assert!(prompt.contains("Reply with ONLY the suggestion"));
}

#[test]
fn test_constants_match_ts() {
    // Parent uncached threshold is 10k.
    assert_eq!(MAX_PARENT_UNCACHED_TOKENS, 10_000);
    // 17 single-word allow-list entries.
    assert_eq!(ALLOWED_SINGLE_WORDS.len(), 17);
    assert!(ALLOWED_SINGLE_WORDS.contains(&"yes"));
    assert!(ALLOWED_SINGLE_WORDS.contains(&"commit"));
    assert!(ALLOWED_SINGLE_WORDS.contains(&"no"));
}

// ── 9-step guard sequence tests ────────────────────────────────

fn ctx_default() -> SuggestionContext {
    SuggestionContext {
        assistant_turn_count: 5,
        last_response_was_api_error: false,
        parent_uncached_tokens: 1_000,
        disabled: false,
        pending_permission: false,
        elicitation_active: false,
        plan_mode: false,
        rate_limit: false,
        bare_mode: false,
        non_interactive: false,
        is_teammate: false,
        awaiting_plan_approval: false,
    }
}

fn good_generation() -> Option<GenerationResult> {
    Some(GenerationResult {
        text: "run the tests".into(),
        prompt_id: "user_intent".into(),
        request_id: Some("req-1".into()),
    })
}

fn assistant_msg(text: &str, request_id: Option<&str>) -> std::sync::Arc<Message> {
    std::sync::Arc::new(Message::Assistant(AssistantMessage {
        message: LlmMessage::Assistant {
            content: vec![coco_messages::AssistantContent::Text(TextContent {
                text: text.into(),
                provider_metadata: None,
            })],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        model: "test".into(),
        stop_reason: Some(StopReason::EndTurn),
        usage: Some(TokenUsage::default()),
        cost_usd: None,
        request_id: request_id.map(str::to_string),
        api_error: None,
    }))
}

#[test]
fn test_extract_suggestion_generation_uses_first_assistant_text_and_request_id() {
    let messages = vec![
        assistant_msg("  first useful prompt  ", Some("req-first")),
        assistant_msg("second prompt", Some("req-second")),
    ];

    let generation = extract_suggestion_generation(&messages);

    assert_eq!(
        generation,
        ExtractedSuggestion {
            text: "first useful prompt".into(),
            request_id: Some("req-first".into()),
        }
    );
}

#[test]
fn test_extract_suggestion_generation_skips_empty_text_but_keeps_first_request_id() {
    let messages = vec![
        assistant_msg("   ", Some("req-first")),
        assistant_msg("run cargo check", Some("req-second")),
    ];

    let generation = extract_suggestion_generation(&messages);

    assert_eq!(
        generation,
        ExtractedSuggestion {
            text: "run cargo check".into(),
            request_id: Some("req-first".into()),
        }
    );
}

#[test]
fn test_guard_step1_aborted_before() {
    let outcome = try_generate_suggestion(&ctx_default(), true, good_generation(), false);
    assert_eq!(outcome, SuggestionOutcome::Aborted);
}

#[test]
fn test_guard_step2_too_few_turns() {
    let mut ctx = ctx_default();
    ctx.assistant_turn_count = 1;
    let outcome = try_generate_suggestion(&ctx, false, good_generation(), false);
    assert!(matches!(
        outcome,
        SuggestionOutcome::Suppressed {
            reason: SuppressReason::TooFewTurns
        }
    ));
}

#[test]
fn test_guard_step3_api_error() {
    let mut ctx = ctx_default();
    ctx.last_response_was_api_error = true;
    let outcome = try_generate_suggestion(&ctx, false, good_generation(), false);
    assert!(matches!(
        outcome,
        SuggestionOutcome::Suppressed {
            reason: SuppressReason::ApiError
        }
    ));
}

#[test]
fn test_guard_step4_cache_cold_threshold() {
    let mut ctx = ctx_default();
    ctx.parent_uncached_tokens = MAX_PARENT_UNCACHED_TOKENS;
    let outcome = try_generate_suggestion(&ctx, false, good_generation(), false);
    assert!(
        !matches!(
            outcome,
            SuggestionOutcome::Suppressed {
                reason: SuppressReason::CacheCold
            }
        ),
        "exactly 10_000 must NOT trigger cache_cold (uses > not >=)"
    );

    ctx.parent_uncached_tokens = MAX_PARENT_UNCACHED_TOKENS + 1;
    let outcome = try_generate_suggestion(&ctx, false, good_generation(), false);
    assert!(matches!(
        outcome,
        SuggestionOutcome::Suppressed {
            reason: SuppressReason::CacheCold
        }
    ));
}

#[test]
fn test_guard_step5_suppress_reasons() {
    type Mutator = fn(&mut SuggestionContext);
    let cases: &[(Mutator, SuppressReason)] = &[
        (|c| c.disabled = true, SuppressReason::Disabled),
        (|c| c.non_interactive = true, SuppressReason::NonInteractive),
        (|c| c.is_teammate = true, SuppressReason::SwarmTeammate),
        (|c| c.bare_mode = true, SuppressReason::BareMode),
        (
            |c| c.pending_permission = true,
            SuppressReason::PendingPermission,
        ),
        (
            |c| c.awaiting_plan_approval = true,
            SuppressReason::AwaitingPlanApproval,
        ),
        (
            |c| c.elicitation_active = true,
            SuppressReason::ElicitationActive,
        ),
        (|c| c.plan_mode = true, SuppressReason::PlanMode),
        (|c| c.rate_limit = true, SuppressReason::RateLimit),
    ];
    for (mutate, expected) in cases {
        let mut ctx = ctx_default();
        mutate(&mut ctx);
        let outcome = try_generate_suggestion(&ctx, false, good_generation(), false);
        match outcome {
            SuggestionOutcome::Suppressed { reason } => assert_eq!(reason, *expected),
            other => panic!("expected Suppressed({expected:?}), got {other:?}"),
        }
    }
}

#[test]
fn test_guard_step7_aborted_after_generation() {
    let outcome = try_generate_suggestion(&ctx_default(), false, good_generation(), true);
    assert_eq!(outcome, SuggestionOutcome::Aborted);
}

#[test]
fn test_guard_step8_empty_text() {
    let outcome = try_generate_suggestion(
        &ctx_default(),
        false,
        Some(GenerationResult {
            text: "  ".into(),
            prompt_id: "p".into(),
            request_id: None,
        }),
        false,
    );
    assert_eq!(outcome, SuggestionOutcome::Empty);

    let outcome = try_generate_suggestion(
        &ctx_default(),
        false,
        Some(GenerationResult {
            text: "NONE".into(),
            prompt_id: "p".into(),
            request_id: None,
        }),
        false,
    );
    assert_eq!(outcome, SuggestionOutcome::Empty);
}

#[test]
fn test_guard_step9_filter_runs_after_other_guards() {
    let outcome = try_generate_suggestion(
        &ctx_default(),
        false,
        Some(GenerationResult {
            text: "Let me run the tests".into(),
            prompt_id: "p".into(),
            request_id: None,
        }),
        false,
    );
    assert!(matches!(
        outcome,
        SuggestionOutcome::Filtered {
            rule: SuggestionFilter::ClaudeVoice
        }
    ));
}

#[test]
fn test_guard_accepted_path() {
    let outcome = try_generate_suggestion(&ctx_default(), false, good_generation(), false);
    match outcome {
        SuggestionOutcome::Accepted { text, .. } => assert_eq!(text, "run the tests"),
        other => panic!("expected Accepted, got {other:?}"),
    }
}

// ── 12-rule filter tests ───────────────────────────────────────

#[test]
fn test_filter_rule_done() {
    assert_eq!(
        should_filter_suggestion("done"),
        Some(SuggestionFilter::Done)
    );
    assert_eq!(
        should_filter_suggestion("DONE"),
        Some(SuggestionFilter::Done)
    );
    assert!(should_filter_suggestion("done!").is_some()); // matched by some other rule
}

#[test]
fn test_filter_rule_meta_text() {
    // `meta_text` rule — `\bstay(s|ing)? silent\b`
    // (no past-tense "stayed silent" form).
    let cases = [
        "nothing found",
        "nothing found.",
        "Nothing to suggest now",
        "no suggestion available",
        "silence is golden",
        "stays silent",
        "staying silent now",
        "silence",
        "  silence  ",
    ];
    for case in cases {
        assert_eq!(
            should_filter_suggestion(case),
            Some(SuggestionFilter::MetaText),
            "expected MetaText for {case:?}"
        );
    }
}

#[test]
fn test_filter_rule_meta_wrapped() {
    assert_eq!(
        should_filter_suggestion("(silence — let user assess)"),
        Some(SuggestionFilter::MetaWrapped)
    );
    assert_eq!(
        should_filter_suggestion("[no suggestion]"),
        Some(SuggestionFilter::MetaWrapped)
    );
}

#[test]
fn test_filter_rule_error_message() {
    let cases = [
        "API Error: connection refused",
        "Prompt is too long",
        "request timed out after 30s",
        "Invalid api key",
        "Image was too large to process",
    ];
    for case in cases {
        assert_eq!(
            should_filter_suggestion(case),
            Some(SuggestionFilter::ErrorMessage),
            "expected ErrorMessage for {case:?}"
        );
    }
}

#[test]
fn test_filter_rule_prefixed_label() {
    assert_eq!(
        should_filter_suggestion("Suggestion: run tests"),
        Some(SuggestionFilter::PrefixedLabel)
    );
    assert_eq!(
        should_filter_suggestion("ACTION: commit changes"),
        Some(SuggestionFilter::PrefixedLabel)
    );
}

#[test]
fn test_filter_rule_too_few_words_with_allow_list() {
    // Allow-listed single words pass.
    assert!(should_filter_suggestion("yes").is_none());
    assert!(should_filter_suggestion("commit").is_none());
    assert!(should_filter_suggestion("deploy").is_none());
    assert!(
        should_filter_suggestion("/restart").is_none(),
        "slash-prefixed"
    );

    // Single words not in allow-list filter.
    assert_eq!(
        should_filter_suggestion("next"),
        Some(SuggestionFilter::TooFewWords)
    );
    assert_eq!(
        should_filter_suggestion("foo"),
        Some(SuggestionFilter::TooFewWords)
    );
}

#[test]
fn test_filter_rule_too_many_words() {
    let s = "one two three four five six seven eight nine ten eleven twelve thirteen";
    assert_eq!(
        should_filter_suggestion(s),
        Some(SuggestionFilter::TooManyWords)
    );
}

#[test]
fn test_filter_rule_too_long() {
    // Build 11 short words separated by spaces — passes word-count
    // filters (between 2 and 12), then triggers too_long on length
    // >= 100 chars.
    let s = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambdalongword";
    assert!(s.split_whitespace().count() <= 12);
    assert!(s.len() < 100);
    assert!(should_filter_suggestion(s).is_none());

    // Same word count but > 100 chars total.
    let long_word = "x".repeat(95);
    let s_long = format!("alpha beta {long_word}");
    assert!(s_long.split_whitespace().count() <= 12);
    assert!(s_long.len() >= 100);
    assert_eq!(
        should_filter_suggestion(&s_long),
        Some(SuggestionFilter::TooLong)
    );
}

#[test]
fn test_filter_rule_multiple_sentences() {
    assert_eq!(
        should_filter_suggestion("Run tests. Then commit."),
        Some(SuggestionFilter::MultipleSentences)
    );
    assert_eq!(
        should_filter_suggestion("Did it work? Try again now"),
        Some(SuggestionFilter::MultipleSentences)
    );
}

#[test]
fn test_filter_rule_has_formatting() {
    assert_eq!(
        should_filter_suggestion("**bold** suggestion"),
        Some(SuggestionFilter::HasFormatting)
    );
    assert_eq!(
        should_filter_suggestion("with newline\nhere"),
        Some(SuggestionFilter::HasFormatting)
    );
    assert_eq!(
        should_filter_suggestion("with star * in middle"),
        Some(SuggestionFilter::HasFormatting)
    );
}

#[test]
fn test_filter_rule_evaluative() {
    let cases = [
        "looks good thanks",
        "thanks for that fix",
        "perfect makes sense",
        "that works",
    ];
    for case in cases {
        assert_eq!(
            should_filter_suggestion(case),
            Some(SuggestionFilter::Evaluative),
            "expected Evaluative for {case:?}"
        );
    }
}

#[test]
fn test_filter_rule_claude_voice() {
    let cases = [
        "Let me run the tests",
        "I'll check that file",
        "Here's what to do",
        "You can try it now",
        "I would suggest running",
    ];
    for case in cases {
        assert_eq!(
            should_filter_suggestion(case),
            Some(SuggestionFilter::ClaudeVoice),
            "expected ClaudeVoice for {case:?}"
        );
    }
}

#[test]
fn test_filter_accepts_normal_user_input() {
    let cases = ["run the tests", "commit and push it", "show me the diff"];
    for case in cases {
        assert!(
            should_filter_suggestion(case).is_none(),
            "{case:?} should pass all filters"
        );
    }
}

// ── Helpers ────────────────────────────────────────────────────

#[test]
fn test_get_suggestion_suppress_reason_priority_order() {
    let mut ctx = ctx_default();
    ctx.disabled = true;
    ctx.pending_permission = true; // Both set — disabled wins (first in order).
    assert_eq!(
        get_suggestion_suppress_reason(&ctx),
        Some(SuppressReason::Disabled)
    );

    let mut ctx = ctx_default();
    ctx.bare_mode = true;
    assert_eq!(
        get_suggestion_suppress_reason(&ctx),
        Some(SuppressReason::BareMode)
    );

    let ctx = ctx_default();
    assert!(get_suggestion_suppress_reason(&ctx).is_none());
}

#[test]
fn test_parent_uncached_tokens_helper() {
    assert_eq!(
        parent_uncached_tokens(&usage_with_cache_read(
            /*input_tokens*/ 600, /*cache_read_tokens*/ 200, /*output_tokens*/ 300,
        )),
        700
    );
    assert_eq!(
        parent_uncached_tokens(&usage_with_cache_read(
            /*input_tokens*/ 0, /*cache_read_tokens*/ 0, /*output_tokens*/ 0,
        )),
        0
    );
    assert_eq!(
        parent_uncached_tokens(&usage_with_cache_read(
            /*input_tokens*/ 10_001, /*cache_read_tokens*/ 0, /*output_tokens*/ 0,
        )),
        10_001
    );
    assert_eq!(
        parent_uncached_tokens(&usage_with_cache_read(
            /*input_tokens*/ 100, /*cache_read_tokens*/ 200, /*output_tokens*/ 50,
        )),
        50
    );
}

fn usage_with_cache_read(
    input_tokens: i64,
    cache_read_tokens: i64,
    output_tokens: i64,
) -> TokenUsage {
    TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: input_tokens,
            cache_read: cache_read_tokens,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: output_tokens,
            ..Default::default()
        },
    }
}
