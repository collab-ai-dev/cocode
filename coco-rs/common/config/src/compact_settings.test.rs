use super::*;
use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;

fn empty_env() -> EnvSnapshot {
    EnvSnapshot::default()
}

#[test]
fn default_config_matches_ts_constants() {
    let cfg = CompactConfig::default();
    assert!(cfg.auto.enabled);
    assert!(!cfg.auto.disabled_by_env);
    assert!(cfg.auto.is_active());
    assert!(cfg.micro.enabled);
    assert_eq!(cfg.micro.keep_recent, 5);
    assert!(!cfg.micro.time_based.enabled);
    assert_eq!(cfg.micro.time_based.gap_threshold_minutes, 60);
    assert!(!cfg.api_native.clear_tool_results);
    assert!(!cfg.api_native.clear_tool_uses);
    assert_eq!(cfg.api_native.max_input_tokens, 180_000);
    assert_eq!(cfg.api_native.target_input_tokens, 40_000);
    assert_eq!(cfg.post_compact.max_files_to_restore, 5);
    assert!(!cfg.session_memory.enabled);
    assert_eq!(cfg.session_memory.min_tokens, 10_000);
    assert!(!cfg.experimental.history_snip.enabled);
    assert!(!cfg.experimental.staged_compact.enabled);
    assert!(cfg.experimental.display_collapses.read_search);
    // TS-alignment: count-based MC and stub rewrite are off by default
    // (TS external `microcompactMessages` is a no-op outside
    // `feature('CACHED_MICROCOMPACT')`; stub rewrite has no TS analogue).
    assert!(!cfg.micro.count_based_enabled);
    assert!(!cfg.micro.clear_file_unchanged_stubs_enabled);
}

#[test]
fn settings_overrides_apply() {
    let mut settings = Settings::default();
    settings.compact.auto.enabled = Some(false);
    settings.compact.micro.keep_recent = Some(3);
    settings.compact.micro.time_based.keep_recent = Some(4);
    settings.compact.api_native.clear_tool_results = Some(true);
    settings.compact.api_native.max_input_tokens = Some(150_000);
    settings.compact.post_compact.max_files_to_restore = Some(2);
    settings.compact.session_memory.enabled = Some(true);
    settings.compact.session_memory.min_tokens = Some(20_000);
    settings.compact.experimental.history_snip.enabled = Some(true);
    settings.compact.experimental.history_snip.auto_pct = Some(0.5);
    settings.compact.experimental.display_collapses.read_search = Some(false);

    let cfg = CompactConfig::resolve(&settings, &empty_env());
    assert!(!cfg.auto.enabled);
    assert!(!cfg.auto.is_active());
    assert_eq!(cfg.micro.keep_recent, 3);
    assert_eq!(cfg.micro.time_based.keep_recent, 4);
    assert!(cfg.api_native.clear_tool_results);
    assert_eq!(cfg.api_native.max_input_tokens, 150_000);
    assert_eq!(cfg.post_compact.max_files_to_restore, 2);
    assert!(cfg.session_memory.enabled);
    assert_eq!(cfg.session_memory.min_tokens, 20_000);
    assert!(cfg.experimental.history_snip.enabled);
    assert!((cfg.experimental.history_snip.auto_pct - 0.5).abs() < f64::EPSILON);
    assert!(!cfg.experimental.display_collapses.read_search);
}

#[test]
fn env_micro_and_post_compact_overrides_settings() {
    let mut settings = Settings::default();
    settings.compact.micro.keep_recent = Some(3);
    settings.compact.micro.time_based.keep_recent = Some(4);
    settings.compact.post_compact.max_files_to_restore = Some(2);
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoCompactMicroKeepRecent, "8"),
        (EnvKey::CocoCompactMicroTimeBasedKeepRecent, "9"),
        (EnvKey::CocoCompactPostCompactMaxFilesToRestore, "7"),
    ]);

    let cfg = CompactConfig::resolve(&settings, &env);

    assert_eq!(cfg.micro.keep_recent, 8);
    assert_eq!(cfg.micro.time_based.keep_recent, 9);
    assert_eq!(cfg.post_compact.max_files_to_restore, 7);
}

#[test]
fn env_disable_compact_overrides_settings() {
    let mut settings = Settings::default();
    settings.compact.auto.enabled = Some(true);
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoCompactDisable, "1")]);
    let cfg = CompactConfig::resolve(&settings, &env);
    assert!(cfg.auto.enabled, "user toggle preserved");
    assert!(cfg.auto.disabled_by_env, "env kill switch active");
    assert!(!cfg.auto.is_active(), "is_active reflects env kill");
}

#[test]
fn env_disable_auto_compact_only() {
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoCompactDisableAuto, "true")]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert!(cfg.auto.auto_disabled_by_env);
    assert!(!cfg.auto.disabled_by_env);
    assert!(!cfg.auto.is_active());
}

#[test]
fn env_context_window_override() {
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoCompactAutoWindow, "180000")]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert_eq!(cfg.auto.context_window_override, Some(180_000));
}

#[test]
fn env_pct_override_clamped() {
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoCompactAutoPctOverride, "85")]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert_eq!(cfg.auto.pct_override, Some(85.0));

    // out-of-range ignored
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoCompactAutoPctOverride, "150")]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert!(cfg.auto.pct_override.is_none());
}

#[test]
fn env_api_clear_tool_overrides_settings_to_true() {
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoCompactApiClearToolResults, "yes"),
        (EnvKey::CocoCompactApiClearToolUses, "1"),
        (EnvKey::CocoCompactApiMaxInputTokens, "200000"),
        (EnvKey::CocoCompactApiTargetInputTokens, "50000"),
    ]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert!(cfg.api_native.clear_tool_results);
    assert!(cfg.api_native.clear_tool_uses);
    assert_eq!(cfg.api_native.max_input_tokens, 200_000);
    assert_eq!(cfg.api_native.target_input_tokens, 50_000);
}

#[test]
fn env_session_memory_enable_disable_priority() {
    // Disable wins when both set (matches TS check order: disable last).
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoCompactSessionMemoryEnable, "1"),
        (EnvKey::CocoCompactSessionMemoryDisable, "1"),
    ]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert!(!cfg.session_memory.enabled);

    // Enable alone.
    let env = EnvSnapshot::from_pairs([(EnvKey::CocoCompactSessionMemoryEnable, "1")]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert!(cfg.session_memory.enabled);
}

#[test]
fn staged_compact_finalize_clamps_commit_to_stage() {
    let mut settings = Settings::default();
    settings.compact.experimental.staged_compact.stage_at_pct = Some(0.8);
    settings.compact.experimental.staged_compact.commit_at_pct = Some(0.5);
    let cfg = CompactConfig::resolve(&settings, &empty_env());
    // commit ≥ stage invariant after finalize.
    let staged = &cfg.experimental.staged_compact;
    assert!((staged.stage_at_pct - 0.8).abs() < f64::EPSILON);
    assert!(staged.commit_at_pct >= staged.stage_at_pct);
}

#[test]
fn invalid_pct_settings_are_rejected() {
    let mut settings = Settings::default();
    settings.compact.experimental.history_snip.auto_pct = Some(1.5);
    let cfg = CompactConfig::resolve(&settings, &empty_env());
    // Out-of-range ignored, default kept.
    assert!(
        (cfg.experimental.history_snip.auto_pct - DEFAULT_HISTORY_SNIP_AUTO_PCT).abs()
            < f64::EPSILON
    );
}

#[test]
fn tool_result_budget_default_matches_ts_constants() {
    let cfg = CompactConfig::default();
    // Phase 0 stub: off by default to match TS feature-stripped behavior
    // (`tengu_hawthorn_steeple` gate, default off).
    assert!(!cfg.tool_result_budget.enabled);
    // TS `MAX_TOOL_RESULTS_PER_MESSAGE_CHARS` in `constants/toolLimits.ts`.
    assert_eq!(cfg.tool_result_budget.per_message_chars, 200_000);
    assert!(cfg.tool_result_budget.persist_records);
}

#[test]
fn tool_result_budget_settings_overrides_apply() {
    let mut settings = Settings::default();
    settings.compact.tool_result_budget.enabled = Some(true);
    settings.compact.tool_result_budget.per_message_chars = Some(150_000);
    settings.compact.tool_result_budget.persist_records = Some(false);
    let cfg = CompactConfig::resolve(&settings, &empty_env());
    assert!(cfg.tool_result_budget.enabled);
    assert_eq!(cfg.tool_result_budget.per_message_chars, 150_000);
    assert!(!cfg.tool_result_budget.persist_records);
}

#[test]
fn tool_result_budget_env_overrides_apply() {
    let env = EnvSnapshot::from_pairs([
        (EnvKey::CocoCompactToolResultBudgetEnable, "1"),
        (EnvKey::CocoCompactToolResultBudgetPerMessageChars, "100000"),
    ]);
    let cfg = CompactConfig::resolve(&Settings::default(), &env);
    assert!(cfg.tool_result_budget.enabled);
    assert_eq!(cfg.tool_result_budget.per_message_chars, 100_000);
    assert!(cfg.tool_result_budget.persist_records);
}

#[test]
fn tool_result_budget_invalid_per_message_chars_ignored() {
    let mut settings = Settings::default();
    settings.compact.tool_result_budget.per_message_chars = Some(0);
    let cfg = CompactConfig::resolve(&settings, &empty_env());
    assert_eq!(
        cfg.tool_result_budget.per_message_chars,
        DEFAULT_TOOL_RESULT_BUDGET_PER_MESSAGE_CHARS
    );

    settings.compact.tool_result_budget.per_message_chars = Some(-1);
    let cfg = CompactConfig::resolve(&settings, &empty_env());
    assert_eq!(
        cfg.tool_result_budget.per_message_chars,
        DEFAULT_TOOL_RESULT_BUDGET_PER_MESSAGE_CHARS
    );
}
