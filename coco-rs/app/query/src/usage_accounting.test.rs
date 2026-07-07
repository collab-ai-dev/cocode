use std::sync::Arc;

use coco_messages::CostTracker;
use coco_types::InputTokens;
use coco_types::OutputTokens;
use coco_types::SessionId;
use coco_types::TokenUsage;
use coco_types::UsageAttribution;
use coco_types::UsageSource;
use tokio::sync::Mutex;

use super::UsageAccounting;
use super::UsageRecord;

#[tokio::test]
async fn retarget_to_empty_session_updates_snapshot_identity() {
    let accounting = UsageAccounting::for_static_session(
        SessionId::try_new("session-old").unwrap(),
        Arc::new(Mutex::new(CostTracker::default())),
        Arc::new(Mutex::new(())),
        UsageAttribution::session(UsageSource::Main),
    );

    assert_eq!(
        accounting.snapshot().await.session_id.as_str(),
        "session-old"
    );

    accounting
        .retarget_to_empty_session(SessionId::try_new("session-new").unwrap())
        .await;

    assert_eq!(
        accounting.snapshot().await.session_id.as_str(),
        "session-new"
    );
}

#[tokio::test]
async fn reset_tracker_clears_snapshot_totals() {
    let accounting = UsageAccounting::for_static_session(
        SessionId::try_new("session-usage").unwrap(),
        Arc::new(Mutex::new(CostTracker::default())),
        Arc::new(Mutex::new(())),
        UsageAttribution::session(UsageSource::Main),
    );

    accounting
        .record_usage(UsageRecord {
            provider: "test-provider",
            model_id: "test-model",
            usage: TokenUsage {
                input_tokens: InputTokens {
                    total: 10,
                    ..Default::default()
                },
                output_tokens: OutputTokens {
                    total: 5,
                    ..Default::default()
                },
            },
            duration_ms: 1,
            source: UsageSource::Main,
            auto_compact_threshold: None,
            event_tx: None,
        })
        .await;

    let recorded = accounting.snapshot().await;
    assert_eq!(recorded.totals.input_tokens, 10);
    assert_eq!(recorded.totals.output_tokens, 5);

    accounting
        .retarget_to_empty_session(SessionId::try_new("session-usage").unwrap())
        .await;

    let reset = accounting.snapshot().await;
    assert_eq!(reset.totals.input_tokens, 0);
    assert_eq!(reset.totals.output_tokens, 0);
}
