use super::*;
use pretty_assertions::assert_eq;

fn usage(input: i64, output: i64) -> TokenUsage {
    TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: input,
            no_cache: input,
            ..Default::default()
        },
        output_tokens: coco_types::OutputTokens {
            total: output,
            ..Default::default()
        },
    }
}

fn test_session_id(value: &str) -> coco_types::SessionId {
    match coco_types::SessionId::try_new(value) {
        Ok(id) => id,
        Err(_) => unreachable!("test session id should be valid"),
    }
}

#[test]
fn pricing_resolves_for_anthropic() {
    let pricing = get_model_pricing(Some("anthropic"), "claude-sonnet-4-5").unwrap();
    assert_eq!(pricing.input_per_mtok, 3.0);
    assert_eq!(pricing.output_per_mtok, 15.0);
}

#[test]
fn pricing_resolves_for_openai() {
    let pricing = get_model_pricing(Some("openai"), "gpt-5-codex").unwrap();
    assert_eq!(pricing.input_per_mtok, 1.25);
    assert_eq!(pricing.output_per_mtok, 10.0);
}

#[test]
fn unknown_pricing_accumulates_tokens_without_cost() {
    let mut tracker = CostTracker::new();
    tracker.record_usage("unknown-provider", "unknown-model", usage(100, 20), 7);

    let snapshot = tracker.snapshot_at(test_session_id("s1"), 123);
    assert_eq!(snapshot.totals.input_tokens, 100);
    assert_eq!(snapshot.totals.output_tokens, 20);
    assert_eq!(snapshot.totals.total_cost_usd, 0.0);
    assert_eq!(
        snapshot.unpriced_models,
        vec![coco_types::ProviderModelSelection {
            provider: "unknown-provider".into(),
            model_id: "unknown-model".into(),
        }]
    );
    assert_eq!(snapshot.totals.unpriced_request_count, 1);
    assert_eq!(snapshot.totals.unpriced_input_tokens, 100);
    assert_eq!(snapshot.totals.unpriced_output_tokens, 20);
    assert_eq!(snapshot.models[0].unpriced_request_count, 1);
    assert!(!snapshot.models[0].priced);
}

#[test]
fn cache_buckets_are_not_double_counted_as_input() {
    let usage = TokenUsage {
        input_tokens: coco_types::InputTokens {
            total: 1_000,
            no_cache: 200,
            cache_read: 700,
            cache_write: 100,
        },
        output_tokens: coco_types::OutputTokens {
            total: 50,
            ..Default::default()
        },
    };

    let cost = calculate_cost_usd(Some("anthropic"), "claude-sonnet-4-5", &usage);
    let expected = (200.0 * 3.0 + 50.0 * 15.0 + 700.0 * 0.3 + 100.0 * 3.75) / 1_000_000.0;
    assert!((cost - expected).abs() < 0.000001);
}

#[test]
fn same_model_id_on_different_providers_stays_separate() {
    let mut tracker = CostTracker::new();
    tracker.record_usage("openai", "shared-model", usage(10, 1), 1);
    tracker.record_usage("anthropic", "shared-model", usage(20, 2), 1);

    let snapshot = tracker.snapshot_at(test_session_id("s1"), 123);
    assert_eq!(snapshot.models.len(), 2);
    assert!(
        snapshot
            .models
            .iter()
            .any(|entry| entry.provider == "openai" && entry.input_tokens == 10)
    );
    assert!(
        snapshot
            .models
            .iter()
            .any(|entry| entry.provider == "anthropic" && entry.input_tokens == 20)
    );
}

#[test]
fn source_records_split_same_provider_model_by_attribution() {
    let mut tracker = CostTracker::new();
    tracker.record_usage_attributed(
        "anthropic",
        "claude-sonnet-4-5",
        usage(10, 1),
        11,
        coco_types::UsageAttribution::session(coco_types::UsageSource::Main),
    );
    tracker.record_usage_attributed(
        "anthropic",
        "claude-sonnet-4-5",
        usage(20, 2),
        13,
        coco_types::UsageAttribution::agent_tool_subagent(
            coco_types::UsageSource::Compact,
            Some("task-abc".to_string()),
        ),
    );

    let snapshot = tracker.snapshot_at(test_session_id("s1"), 123);
    assert_eq!(snapshot.models.len(), 1);
    assert_eq!(snapshot.models[0].input_tokens, 30);
    assert_eq!(snapshot.source_records.len(), 2);
    assert!(snapshot.source_records.iter().any(|entry| {
        entry.group == coco_types::UsageSourceGroup::Session
            && entry.source == coco_types::UsageSource::Main
            && entry.agent_task_id.is_none()
            && entry.input_tokens == 10
            && entry.duration_ms == 11
    }));
    assert!(snapshot.source_records.iter().any(|entry| {
        entry.group == coco_types::UsageSourceGroup::AgentToolSubagent
            && entry.source == coco_types::UsageSource::Compact
            && entry.agent_task_id.as_deref() == Some("task-abc")
            && entry.input_tokens == 20
            && entry.duration_ms == 13
    }));
}

#[test]
fn merge_preserves_source_record_counts_duration_cost_and_unpriced() {
    let attribution = coco_types::UsageAttribution::agent_tool_subagent(
        coco_types::UsageSource::HookAgent,
        Some("task-abc".to_string()),
    );
    let mut left = CostTracker::new();
    left.record_usage_attributed(
        "anthropic",
        "claude-sonnet-4-5",
        usage(10, 1),
        11,
        attribution.clone(),
    );
    let mut right = CostTracker::new();
    right.record_usage_attributed(
        "anthropic",
        "claude-sonnet-4-5",
        usage(20, 2),
        13,
        attribution.clone(),
    );
    right.record_usage_attributed(
        "unknown-provider",
        "unknown-model",
        usage(30, 3),
        17,
        attribution,
    );

    left.merge_from(&right);
    let snapshot = left.snapshot_at(test_session_id("s1"), 123);

    let priced = snapshot
        .source_records
        .iter()
        .find(|entry| entry.provider == "anthropic")
        .expect("priced source record");
    assert_eq!(priced.request_count, 2);
    assert_eq!(priced.duration_ms, 24);
    assert_eq!(priced.input_tokens, 30);
    assert!(priced.total_cost_usd > 0.0);

    let unpriced = snapshot
        .source_records
        .iter()
        .find(|entry| entry.provider == "unknown-provider")
        .expect("unpriced source record");
    assert_eq!(unpriced.request_count, 1);
    assert_eq!(unpriced.duration_ms, 17);
    assert_eq!(unpriced.unpriced_request_count, 1);
    assert_eq!(unpriced.unpriced_input_tokens, 30);
    assert!(!unpriced.priced);
    assert_eq!(snapshot.totals.request_count, 3);
    assert_eq!(snapshot.totals.unpriced_request_count, 1);
}

#[test]
fn partially_unpriced_bucket_remains_marked_unpriced() {
    let mut tracker = CostTracker::from_snapshot(coco_types::SessionUsageSnapshot {
        session_id: test_session_id("s1"),
        models: vec![coco_types::SessionModelUsageEntry {
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4-5".into(),
            input_tokens: 50,
            output_tokens: 5,
            request_count: 1,
            unpriced_request_count: 1,
            unpriced_input_tokens: 50,
            unpriced_output_tokens: 5,
            priced: false,
            ..Default::default()
        }],
        ..coco_types::SessionUsageSnapshot::empty(test_session_id("s1"))
    });
    tracker.record_usage("anthropic", "claude-sonnet-4-5", usage(100, 10), 1);

    let snapshot = tracker.snapshot_at(test_session_id("s1"), 123);
    let sonnet = snapshot.models.first().unwrap();
    assert_eq!(sonnet.request_count, 2);
    assert_eq!(sonnet.unpriced_request_count, 1);
    assert!(!sonnet.priced);
    assert_eq!(snapshot.totals.unpriced_request_count, 1);
    assert_eq!(snapshot.unpriced_models.len(), 1);
}

#[test]
fn legacy_snapshot_models_rehydrate_as_session_main_source_records() {
    let tracker = CostTracker::from_snapshot(coco_types::SessionUsageSnapshot {
        session_id: test_session_id("s1"),
        models: vec![coco_types::SessionModelUsageEntry {
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4-5".into(),
            input_tokens: 50,
            output_tokens: 5,
            request_count: 1,
            priced: true,
            ..Default::default()
        }],
        ..coco_types::SessionUsageSnapshot::empty(test_session_id("s1"))
    });

    let snapshot = tracker.snapshot_at(test_session_id("s1"), 123);
    assert_eq!(snapshot.source_records.len(), 1);
    let entry = &snapshot.source_records[0];
    assert_eq!(entry.group, coco_types::UsageSourceGroup::Session);
    assert_eq!(entry.source, coco_types::UsageSource::Main);
    assert_eq!(entry.input_tokens, 50);
}

#[test]
fn snapshot_totals_include_web_search_requests_from_loaded_entries() {
    let snapshot = coco_types::SessionUsageSnapshot {
        session_id: test_session_id("s1"),
        models: vec![coco_types::SessionModelUsageEntry {
            provider: "anthropic".into(),
            model_id: "claude-sonnet-4-5".into(),
            web_search_requests: 3,
            ..Default::default()
        }],
        ..coco_types::SessionUsageSnapshot::empty(test_session_id("s1"))
    };
    let tracker = CostTracker::from_snapshot(snapshot);

    assert_eq!(
        tracker
            .snapshot_at(test_session_id("s1"), 123)
            .totals
            .web_search_requests,
        3
    );
}

#[test]
fn format_cost_threshold_at_half_dollar() {
    // Strict `> 0.5` => 2 decimals, else 4.
    assert_eq!(format_cost(1.23), "$1.23");
    assert_eq!(format_cost(0.6), "$0.60");
    assert_eq!(format_cost(0.51), "$0.51");
    // 0.50 boundary takes the 4-decimal branch (strict `>`).
    assert_eq!(format_cost(0.50), "$0.5000");
    assert_eq!(format_cost(0.30), "$0.3000");
    assert_eq!(format_cost(0.005), "$0.0050");
}

#[test]
fn format_session_cost_empty_reports_no_usage() {
    let snap = coco_types::SessionUsageSnapshot::empty(test_session_id("s1"));
    let out = format_session_cost(&snap);
    assert!(out.contains("No API usage recorded yet"));
}

#[test]
fn format_session_cost_renders_per_model_and_total() {
    let snap = coco_types::SessionUsageSnapshot {
        session_id: test_session_id("s1"),
        totals: coco_types::SessionUsageTotals {
            input_tokens: 1_500,
            output_tokens: 500,
            total_cost_usd: 0.42,
            request_count: 2,
            ..Default::default()
        },
        models: vec![
            coco_types::SessionModelUsageEntry {
                provider: "openai".into(),
                model_id: "gpt-5".into(),
                input_tokens: 1_000,
                output_tokens: 300,
                total_cost_usd: 0.30,
                request_count: 1,
                priced: true,
                ..Default::default()
            },
            coco_types::SessionModelUsageEntry {
                provider: "local".into(),
                model_id: "mystery".into(),
                input_tokens: 500,
                output_tokens: 200,
                request_count: 1,
                priced: false,
                ..Default::default()
            },
        ],
        ..coco_types::SessionUsageSnapshot::empty(test_session_id("s1"))
    };
    let out = format_session_cost(&snap);
    // Multi-provider: both buckets present, keyed by (provider, model_id).
    assert!(out.contains("openai / gpt-5"));
    assert!(out.contains("local / mystery"));
    // Priced model shows its cost; unpriced model is flagged, not mispriced.
    // 0.30 <= 0.5 => 4-decimal branch.
    assert!(out.contains("$0.3000"));
    assert!(out.contains("unpriced model"));
    // Thousands grouping + total (0.42 <= 0.5 => 4 decimals).
    assert!(out.contains("1,000"));
    assert!(out.contains("**Total cost:  $0.4200**"));
}
