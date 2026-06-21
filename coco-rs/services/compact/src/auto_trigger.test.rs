use super::*;
use coco_config::AutoCompactConfig;

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
