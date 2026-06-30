use super::*;
use coco_config::AutoCompactConfig;
use coco_llm_types::AssistantContentPart;
use coco_llm_types::SharedV4FileData;
use coco_llm_types::StopReason;
use coco_llm_types::ToolResultContentPart;
use coco_messages::AssistantMessage;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::ToolContent;
use coco_messages::ToolResultContent;
use coco_messages::ToolResultMessage;
use coco_messages::UserContent;
use coco_messages::UserMessage;
use coco_types::InputTokens;
use coco_types::OutputTokens;
use coco_types::ToolId;
use coco_types::ToolName;
use uuid::Uuid;

// Threshold formula: effectiveWindow = contextWindow - min(maxOutput, 20K)
//                    threshold = effectiveWindow - 13K
// For 200K window, 16K max output:
//   effective = 200K - 16K = 184K
//   threshold = 184K - 13K = 171K

const CTX: i64 = 200_000;
const MAX_OUT: i64 = 16_384;

fn cfg_default() -> AutoCompactConfig {
    AutoCompactConfig::default()
}

fn cfg_disabled() -> AutoCompactConfig {
    AutoCompactConfig {
        enabled: false,
        ..AutoCompactConfig::default()
    }
}

fn cfg_with_pct(pct: f64) -> AutoCompactConfig {
    AutoCompactConfig {
        pct_override: Some(pct),
        ..AutoCompactConfig::default()
    }
}

fn window_inputs() -> AutoCompactWindowInputs<'static> {
    AutoCompactWindowInputs {
        hard_cap: 1_000_000,
        ..AutoCompactWindowInputs::default()
    }
}

fn user_with_media() -> Message {
    Message::User(UserMessage {
        message: LlmMessage::User {
            content: vec![
                UserContent::text("see attached files"),
                UserContent::file(SharedV4FileData::data_base64("abc"), "image/png"),
                UserContent::file(SharedV4FileData::data_base64("pdf"), "application/pdf"),
            ],
            provider_options: None,
        },
        uuid: Uuid::new_v4(),
        timestamp: String::new(),
        is_visible_in_transcript_only: false,
        is_virtual: false,
        is_compact_summary: false,
        permission_mode: None,
        origin: None,
        parent_tool_use_id: None,
    })
}

fn tool_result_with_media() -> Message {
    Message::ToolResult(ToolResultMessage {
        uuid: Uuid::new_v4(),
        source_assistant_uuid: None,
        display_data: None,
        message: LlmMessage::Tool {
            content: vec![ToolContent::ToolResult(ToolResultContent {
                tool_call_id: "tool-1".to_string(),
                tool_name: ToolName::Read.as_str().to_string(),
                output: coco_llm_types::ToolResultContent::content_parts(vec![
                    ToolResultContentPart::text("text"),
                    ToolResultContentPart::file_data("abc", "image/jpeg"),
                    ToolResultContentPart::file_data("pdf", "application/pdf"),
                ]),
                is_error: false,
                provider_metadata: None,
            })],
            provider_options: None,
        },
        tool_use_id: "tool-1".to_string(),
        tool_id: ToolId::Builtin(ToolName::Read),
        is_error: false,
    })
}

fn assistant_with_usage(input_total: i64, cache_read: i64, cache_write: i64) -> Message {
    Message::Assistant(AssistantMessage {
        message: LlmMessage::assistant(vec![AssistantContentPart::text("done")]),
        uuid: Uuid::new_v4(),
        model: "mock".to_string(),
        stop_reason: Some(StopReason::EndTurn),
        usage: Some(coco_types::TokenUsage {
            input_tokens: InputTokens {
                total: input_total,
                no_cache: input_total
                    .saturating_sub(cache_read)
                    .saturating_sub(cache_write),
                cache_read,
                cache_write,
            },
            output_tokens: OutputTokens::default(),
        }),
        cost_usd: None,
        request_id: None,
        api_error: None,
    })
}

#[test]
fn test_effective_context_window() {
    let cfg = cfg_default();
    assert_eq!(effective_context_window(CTX, MAX_OUT, &cfg), CTX - MAX_OUT);
    // Max output capped at 20K
    assert_eq!(effective_context_window(CTX, 30_000, &cfg), CTX - 20_000);
}

#[test]
fn test_auto_compact_threshold_formula() {
    let cfg = cfg_default();
    let threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg);
    // 200K - 16384 - 13000 = 170616
    assert_eq!(threshold, CTX - MAX_OUT - 13_000);
}

#[test]
fn test_should_compact_at_threshold() {
    let cfg = cfg_default();
    let threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg);
    assert!(should_auto_compact(threshold, CTX, MAX_OUT, &cfg));
    assert!(should_auto_compact(threshold + 1, CTX, MAX_OUT, &cfg));
}

#[test]
fn test_should_not_compact_below_threshold() {
    let cfg = cfg_default();
    let threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg);
    assert!(!should_auto_compact(threshold - 1, CTX, MAX_OUT, &cfg));
    assert!(!should_auto_compact(0, CTX, MAX_OUT, &cfg));
}

#[test]
fn test_zero_context_window() {
    assert!(!should_auto_compact(100, 0, MAX_OUT, &cfg_default()));
}

#[test]
fn test_pct_override_caps_threshold() {
    // 50% override gives a lower threshold than the default formula —
    // the percentage path applies a `.min(default)` floor so it never
    // exceeds the legacy threshold.
    let cfg = cfg_with_pct(50.0);
    let threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg);
    let default_threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg_default());
    assert!(threshold < default_threshold);
}

#[test]
fn test_calculate_token_warning_state() {
    let cfg = cfg_default();
    let state = calculate_token_warning_state(170_000, CTX, MAX_OUT, &cfg);
    let _effective = effective_context_window(CTX, MAX_OUT, &cfg);
    // 170K is close to effective (~184K)
    assert!(state.percent_left < 10, "should have <10% left");
    assert!(state.is_above_warning_threshold, "above warning threshold");

    // Well below threshold
    let state_low = calculate_token_warning_state(50_000, CTX, MAX_OUT, &cfg);
    assert!(state_low.percent_left > 50);
    assert!(!state_low.is_above_warning_threshold);
    assert!(!state_low.is_above_auto_compact_threshold);
}

#[test]
fn test_warning_state_auto_compact_disabled() {
    let cfg_off = cfg_disabled();
    let threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg_off);
    let state = calculate_token_warning_state(threshold + 1000, CTX, MAX_OUT, &cfg_off);
    assert!(
        !state.is_above_auto_compact_threshold,
        "auto compact disabled should not trigger"
    );
}

#[test]
fn test_warning_state_blocking_limit_override() {
    let cfg = AutoCompactConfig {
        blocking_limit_override: Some(100_000),
        ..AutoCompactConfig::default()
    };
    let state = calculate_token_warning_state(100_000, CTX, MAX_OUT, &cfg);
    assert!(state.is_at_blocking_limit);
    let state = calculate_token_warning_state(99_999, CTX, MAX_OUT, &cfg);
    assert!(!state.is_at_blocking_limit);
}

#[test]
fn test_time_based_mc_defaults() {
    let config = TimeBasedMcConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.gap_threshold_minutes, 60);
    assert_eq!(config.keep_recent, 5);
}

#[test]
fn test_recursion_guard_session_memory() {
    let cfg = cfg_default();
    // session_memory and compact must not auto-compact (forked-agent deadlock).
    assert!(!should_auto_compact_guarded(
        i64::MAX / 2,
        CTX,
        MAX_OUT,
        &cfg,
        CompactQuerySource::SessionMemory,
    ));
    assert!(!should_auto_compact_guarded(
        i64::MAX / 2,
        CTX,
        MAX_OUT,
        &cfg,
        CompactQuerySource::Compact,
    ));
}

#[test]
fn test_recursion_guard_other_passes_through() {
    let cfg = cfg_default();
    let threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg);
    assert!(should_auto_compact_guarded(
        threshold + 1,
        CTX,
        MAX_OUT,
        &cfg,
        CompactQuerySource::Other,
    ));
}

#[test]
fn test_disabled_config_blocks_guarded() {
    let cfg = cfg_disabled();
    let threshold = auto_compact_threshold(CTX, MAX_OUT, &cfg);
    assert!(!should_auto_compact_guarded(
        threshold + 1,
        CTX,
        MAX_OUT,
        &cfg,
        CompactQuerySource::Other,
    ));
}

#[test]
fn test_prefix_overflow_returns_none_without_usage() {
    let messages = vec![user_with_media()];
    assert_eq!(
        prefix_overflow_check(&messages, CTX, MAX_OUT, &cfg_default(), 0),
        None
    );
}

#[test]
fn test_prefix_overflow_returns_none_when_fixed_prefix_fits() {
    let messages = vec![
        user_with_media(),
        tool_result_with_media(),
        assistant_with_usage(
            175_000, /*cache_read*/ 10_000, /*cache_write*/ 5_000,
        ),
    ];
    assert_eq!(
        prefix_overflow_check(&messages, CTX, MAX_OUT, &cfg_default(), 0),
        None
    );
}

#[test]
fn test_prefix_overflow_reports_fixed_prefix_and_media_counts() {
    let messages = vec![
        user_with_media(),
        tool_result_with_media(),
        assistant_with_usage(
            190_000, /*cache_read*/ 10_000, /*cache_write*/ 5_000,
        ),
    ];
    let report = prefix_overflow_check(&messages, CTX, MAX_OUT, &cfg_default(), 500)
        .expect("fixed prefix should exceed auto-compact threshold");

    assert_eq!(report.total_input_tokens, 190_000);
    assert_eq!(report.snip_tokens_freed, 500);
    assert_eq!(
        report.threshold_tokens,
        auto_compact_threshold(CTX, MAX_OUT, &cfg_default())
    );
    assert_eq!(report.document_block_count, 2);
    assert_eq!(report.image_block_count, 2);
    assert_eq!(
        report.prefix_tokens,
        report
            .total_input_tokens
            .saturating_sub(report.snip_tokens_freed)
            .saturating_sub(report.messages_estimate)
    );
    assert!(report.prefix_tokens > report.threshold_tokens);
}

#[test]
fn test_env_kill_switches_block_is_active() {
    let cfg = AutoCompactConfig {
        enabled: true,
        disabled_by_env: true,
        ..AutoCompactConfig::default()
    };
    assert!(!cfg.is_active());
    let cfg = AutoCompactConfig {
        enabled: true,
        auto_disabled_by_env: true,
        ..AutoCompactConfig::default()
    };
    assert!(!cfg.is_active());
}

#[test]
fn test_evaluate_time_based_trigger() {
    let cfg = TimeBasedMcConfig {
        enabled: true,
        gap_threshold_minutes: 60,
        keep_recent: 5,
    };
    let now = 1_700_000_000_000_i64;
    // Last assistant 30 min ago — below threshold, no trigger.
    let no_fire = evaluate_time_based_trigger(&cfg, now, Some(now - 30 * 60_000), true);
    assert!(no_fire.is_none());
    // Last assistant 90 min ago — above threshold, fires.
    let fire = evaluate_time_based_trigger(&cfg, now, Some(now - 90 * 60_000), true);
    assert!(fire.is_some());
    assert!(fire.unwrap().gap_minutes >= 60.0);
    // Subagent (not main thread): no fire even when gap exceeded.
    let subagent = evaluate_time_based_trigger(&cfg, now, Some(now - 90 * 60_000), false);
    assert!(subagent.is_none());
    // Disabled: no fire.
    let disabled = TimeBasedMcConfig {
        enabled: false,
        ..cfg
    };
    assert!(evaluate_time_based_trigger(&disabled, now, Some(now - 90 * 60_000), true).is_none());
}

#[test]
fn test_clamp_to_model_max_caps_oversized_window() {
    // Configured window exceeds the model's authoritative max → clamped down.
    assert_eq!(clamp_to_model_max(1_000_000, Some(CTX)), CTX);
}

#[test]
fn test_clamp_to_model_max_keeps_smaller_window() {
    // Configured window already under the model max → unchanged.
    assert_eq!(clamp_to_model_max(128_000, Some(CTX)), 128_000);
}

#[test]
fn test_clamp_to_model_max_unknown_max_passes_through() {
    // No model max known → configured value passes through untouched.
    assert_eq!(clamp_to_model_max(1_000_000, None), 1_000_000);
    // Non-positive model max is ignored (never widens, never zeroes).
    assert_eq!(clamp_to_model_max(1_000_000, Some(0)), 1_000_000);
    assert_eq!(clamp_to_model_max(1_000_000, Some(-1)), 1_000_000);
}

#[test]
fn test_clamp_to_model_max_then_override_min_wins() {
    // The override and the model-max clamp compose: the tightest bound wins.
    // model_max = 200K caps the configured 1M; an even tighter 180K override
    // then applied via `apply_context_window_override` wins.
    let clamped = clamp_to_model_max(1_000_000, Some(CTX));
    assert_eq!(clamped, CTX);
    assert_eq!(
        apply_context_window_override(clamped, Some(180_000)),
        180_000
    );
    // When the override is larger than the model max, the model max stays.
    assert_eq!(apply_context_window_override(clamped, Some(500_000)), CTX);
}

#[test]
fn test_auto_compact_window_configured_override_wins() {
    let resolution = resolve_auto_compact_window(AutoCompactWindowInputs {
        configured_override: Some(ConfiguredAutoCompactWindow {
            window: 250_000,
            source: AutoCompactWindowSource::Env,
        }),
        clientdata: ClientDataAutoCompactWindow {
            window: Some(300_000),
            replaces_model_default: false,
        },
        experiment_window: Some(350_000),
        model_id: Some("claude-sonnet-4-6"),
        ..window_inputs()
    });

    assert_eq!(
        resolution,
        AutoCompactWindowResolution {
            window: 250_000,
            configured: 250_000,
            source: AutoCompactWindowSource::Env,
        }
    );
    assert!(resolution.source.is_configured());
}

#[test]
fn test_auto_compact_window_clientdata_wins_over_experiment() {
    let resolution = resolve_auto_compact_window(AutoCompactWindowInputs {
        clientdata: ClientDataAutoCompactWindow {
            window: Some(300_000),
            replaces_model_default: false,
        },
        experiment_window: Some(350_000),
        ..window_inputs()
    });

    assert_eq!(resolution.window, 300_000);
    assert_eq!(resolution.configured, 300_000);
    assert_eq!(resolution.source, AutoCompactWindowSource::ClientData);
}

#[test]
fn test_auto_compact_window_ignores_invalid_clientdata() {
    let resolution = resolve_auto_compact_window(AutoCompactWindowInputs {
        clientdata: ClientDataAutoCompactWindow {
            window: Some(AUTO_COMPACT_WINDOW_MIN_TOKENS - 1),
            replaces_model_default: false,
        },
        experiment_window: Some(350_000),
        ..window_inputs()
    });

    assert_eq!(resolution.window, 350_000);
    assert_eq!(resolution.source, AutoCompactWindowSource::Experiment);
    assert!(!resolution.source.is_configured());
}

#[test]
fn test_auto_compact_window_ignores_out_of_range_experiment() {
    let resolution = resolve_auto_compact_window(AutoCompactWindowInputs {
        experiment_window: Some(AUTO_COMPACT_WINDOW_MIN_TOKENS - 1),
        model_default_window: Some(300_000),
        ..window_inputs()
    });

    assert_eq!(resolution.window, 300_000);
    assert_eq!(resolution.configured, 300_000);
    assert_eq!(resolution.source, AutoCompactWindowSource::ModelDefault);
}

#[test]
fn test_auto_compact_window_model_default_static_set_is_configured() {
    let resolution = resolve_auto_compact_window(AutoCompactWindowInputs {
        hard_cap: CTX,
        model_id: Some("claude-sonnet-4-6"),
        ..AutoCompactWindowInputs::default()
    });

    assert_eq!(
        resolution,
        AutoCompactWindowResolution {
            window: CTX,
            configured: MODEL_DEFAULT_AUTO_COMPACT_WINDOW_TOKENS,
            source: AutoCompactWindowSource::ModelDefault,
        }
    );
    assert!(resolution.source.is_configured());
}

#[test]
fn test_auto_compact_window_clientdata_presence_replaces_default_map() {
    let resolution = resolve_auto_compact_window(AutoCompactWindowInputs {
        clientdata: ClientDataAutoCompactWindow {
            window: None,
            replaces_model_default: true,
        },
        model_default_window: Some(300_000),
        ..window_inputs()
    });

    assert_eq!(resolution.window, 1_000_000);
    assert_eq!(resolution.source, AutoCompactWindowSource::Auto);
}

#[test]
fn test_auto_compact_window_default_map_when_not_replaced() {
    let resolution = resolve_auto_compact_window(AutoCompactWindowInputs {
        model_default_window: Some(300_000),
        ..window_inputs()
    });

    assert_eq!(resolution.window, 300_000);
    assert_eq!(resolution.configured, 300_000);
    assert_eq!(resolution.source, AutoCompactWindowSource::ModelDefault);
}

#[test]
fn test_auto_compact_window_auto_fallback_is_not_configured() {
    let resolution = resolve_auto_compact_window(window_inputs());

    assert_eq!(
        resolution,
        AutoCompactWindowResolution {
            window: 1_000_000,
            configured: 1_000_000,
            source: AutoCompactWindowSource::Auto,
        }
    );
    assert!(!resolution.source.is_configured());
}

#[test]
fn test_precompute_arm_absent_table_uses_scalar_fraction() {
    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Repl,
        scalar_fraction: Some(0.15),
        arm_table: None,
    });

    assert_eq!(
        resolution,
        PrecomputeArmResolution {
            fraction: 0.15,
            source: PrecomputeArmSource::Scalar,
            matched_window_key: None,
            malformed_payload_type: None,
        }
    );
}

#[test]
fn test_precompute_arm_exact_window_uses_surface_fraction() {
    let table = serde_json::json!({
        "200000": { "repl": 0.11, "sdk": 0.07 },
        "default": { "repl": 0.2, "sdk": 0.18 }
    });

    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Sdk,
        scalar_fraction: Some(0.3),
        arm_table: Some(&table),
    });

    assert_eq!(
        resolution,
        PrecomputeArmResolution {
            fraction: 0.07,
            source: PrecomputeArmSource::TableExact,
            matched_window_key: Some(200_000),
            malformed_payload_type: None,
        }
    );
}

#[test]
fn test_precompute_arm_accepts_js_number_integer_window_keys() {
    let table = serde_json::json!({
        "2e5": { "repl": 0.11, "sdk": 0.07 },
        "300000.0": { "repl": 0.13, "sdk": 0.09 },
        "default": { "repl": 0.2, "sdk": 0.18 }
    });

    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Sdk,
        scalar_fraction: Some(0.3),
        arm_table: Some(&table),
    });

    assert_eq!(resolution.fraction, 0.07);
    assert_eq!(resolution.source, PrecomputeArmSource::TableExact);
    assert_eq!(resolution.matched_window_key, Some(200_000));
}

#[test]
fn test_precompute_arm_rejects_fractional_window_keys() {
    let table = serde_json::json!({
        "200000.5": { "repl": 0.11, "sdk": 0.07 },
        "default": { "repl": 0.2, "sdk": 0.18 }
    });

    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Sdk,
        scalar_fraction: Some(0.3),
        arm_table: Some(&table),
    });

    assert_eq!(resolution.fraction, 0.3);
    assert_eq!(resolution.source, PrecomputeArmSource::Malformed);
    assert_eq!(resolution.malformed_payload_type, Some("object"));
}

#[test]
fn test_precompute_arm_default_used_when_no_exact_match() {
    let table = serde_json::json!({
        "1000000": { "repl": 0.05, "sdk": 0.03 },
        "default": { "repl": 0.19, "sdk": 0.17 }
    });

    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Repl,
        scalar_fraction: Some(0.3),
        arm_table: Some(&table),
    });

    assert_eq!(resolution.fraction, 0.19);
    assert_eq!(resolution.source, PrecomputeArmSource::TableDefault);
    assert_eq!(resolution.matched_window_key, None);
    assert_eq!(resolution.malformed_payload_type, None);
}

#[test]
fn test_precompute_arm_valid_table_without_match_falls_back_to_scalar() {
    let table = serde_json::json!({
        "1000000": { "repl": 0.05, "sdk": 0.03 }
    });

    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Repl,
        scalar_fraction: Some(0.31),
        arm_table: Some(&table),
    });

    assert_eq!(resolution.fraction, 0.31);
    assert_eq!(resolution.source, PrecomputeArmSource::TableNoMatch);
}

#[test]
fn test_precompute_arm_malformed_table_is_all_or_nothing() {
    let table = serde_json::json!({
        "200000": { "repl": 0.11, "sdk": 1.0 },
        "default": { "repl": 0.19, "sdk": 0.17 }
    });

    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Repl,
        scalar_fraction: Some(0.25),
        arm_table: Some(&table),
    });

    assert_eq!(resolution.fraction, 0.25);
    assert_eq!(resolution.source, PrecomputeArmSource::Malformed);
    assert_eq!(resolution.matched_window_key, None);
    assert_eq!(resolution.malformed_payload_type, Some("object"));
}

#[test]
fn test_precompute_arm_malformed_array_reports_payload_type() {
    let table = serde_json::json!([]);

    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Repl,
        scalar_fraction: Some(0.25),
        arm_table: Some(&table),
    });

    assert_eq!(resolution.fraction, 0.25);
    assert_eq!(resolution.source, PrecomputeArmSource::Malformed);
    assert_eq!(resolution.malformed_payload_type, Some("array"));
}

#[test]
fn test_precompute_arm_invalid_scalar_uses_default_fraction() {
    let resolution = resolve_precompute_arm(PrecomputeArmInputs {
        resolved_window: 200_000,
        surface: PrecomputeSurface::Repl,
        scalar_fraction: Some(1.0),
        arm_table: None,
    });

    assert_eq!(resolution.fraction, DEFAULT_PRECOMPUTE_BUFFER_FRACTION);
    assert_eq!(resolution.source, PrecomputeArmSource::Scalar);
}
