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

#[tokio::test]
async fn mirror_keeps_child_totals_separate_and_charges_parent_as_side_query() {
    let parent = UsageAccounting::new(
        SessionId::try_new("parent-session").unwrap(),
        UsageAttribution::session(UsageSource::Main),
    );
    let child = UsageAccounting::new(
        SessionId::try_new("child-session").unwrap(),
        UsageAttribution::session(UsageSource::Main),
    );
    child
        .install_mirror(parent.clone(), UsageSource::SideQuery)
        .await;
    let (parent_snapshot_tx, mut parent_snapshot_rx) = tokio::sync::mpsc::channel(1);
    parent.install_snapshot_tx(parent_snapshot_tx).await;
    let (child_snapshot_tx, mut child_snapshot_rx) = tokio::sync::mpsc::channel(1);
    child.install_snapshot_tx(child_snapshot_tx).await;

    child
        .record_usage(UsageRecord {
            provider: "test-provider",
            model_id: "test-model",
            usage: TokenUsage {
                input_tokens: InputTokens {
                    total: 30,
                    cache_read: 20,
                    ..Default::default()
                },
                output_tokens: OutputTokens {
                    total: 7,
                    ..Default::default()
                },
            },
            duration_ms: 2,
            source: UsageSource::Main,
            auto_compact_threshold: Some(1_000),
            event_tx: None,
        })
        .await;

    let child_snapshot = child.snapshot().await;
    let parent_snapshot = parent.snapshot().await;
    assert_eq!(child_snapshot.totals, parent_snapshot.totals);
    assert_eq!(child_snapshot.session_id.as_str(), "child-session");
    assert_eq!(parent_snapshot.session_id.as_str(), "parent-session");
    assert_eq!(child_snapshot.source_records[0].source, UsageSource::Main);
    assert_eq!(
        parent_snapshot.source_records[0].source,
        UsageSource::SideQuery
    );
    let parent_event_snapshot = parent_snapshot_rx
        .try_recv()
        .expect("parent usage snapshot");
    assert_eq!(parent_event_snapshot.session_id, parent_snapshot.session_id);
    let child_event_snapshot = child_snapshot_rx.try_recv().expect("child usage snapshot");
    assert_eq!(child_event_snapshot.session_id, child_snapshot.session_id);
}

#[tokio::test]
async fn mirror_charges_parent_before_a_blocked_child_event_can_be_cancelled() {
    let parent = UsageAccounting::new(
        SessionId::try_new("parent-cancellation").unwrap(),
        UsageAttribution::session(UsageSource::Main),
    );
    let child = UsageAccounting::new(
        SessionId::try_new("child-cancellation").unwrap(),
        UsageAttribution::session(UsageSource::Main),
    );
    child
        .install_mirror(parent.clone(), UsageSource::SideQuery)
        .await;

    let (child_snapshot_tx, _child_snapshot_rx) = tokio::sync::mpsc::channel(1);
    child_snapshot_tx
        .send(coco_types::SessionUsageSnapshot::empty(
            SessionId::try_new("filled-channel").unwrap(),
        ))
        .await
        .unwrap();
    child.install_snapshot_tx(child_snapshot_tx).await;

    let child_for_task = child.clone();
    let record_task = tokio::spawn(async move {
        child_for_task
            .record_usage(UsageRecord {
                provider: "test-provider",
                model_id: "test-model",
                usage: TokenUsage {
                    input_tokens: InputTokens {
                        total: 11,
                        ..Default::default()
                    },
                    output_tokens: OutputTokens {
                        total: 3,
                        ..Default::default()
                    },
                },
                duration_ms: 1,
                source: UsageSource::Main,
                auto_compact_threshold: None,
                event_tx: None,
            })
            .await;
    });

    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if child.snapshot().await.totals.input_tokens == 11 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("child reaches its blocked snapshot send");
    record_task.abort();
    let _ = record_task.await;

    let parent_snapshot = parent.snapshot().await;
    assert_eq!(parent_snapshot.totals.input_tokens, 11);
    assert_eq!(parent_snapshot.totals.output_tokens, 3);
    assert_eq!(
        parent_snapshot.source_records[0].source,
        UsageSource::SideQuery
    );
}
