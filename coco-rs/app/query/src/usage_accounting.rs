use std::sync::Arc;

use coco_inference::ModelRuntimeSnapshot;
use coco_messages::CostTracker;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::SessionUsageSnapshot;
use coco_types::TokenUsage;
use coco_types::UsageAttribution;
use coco_types::UsageSource;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct UsageAccounting {
    session_id: Arc<RwLock<String>>,
    tracker: Arc<Mutex<CostTracker>>,
    write_lock: Arc<Mutex<()>>,
    transcript_store: Option<Arc<dyn coco_session::SessionStore>>,
    persist_session: bool,
    event_tx: Option<Arc<RwLock<Option<mpsc::Sender<CoreEvent>>>>>,
    base_attribution: UsageAttribution,
}

pub struct UsageRecord<'a> {
    pub provider: &'a str,
    pub model_id: &'a str,
    pub usage: TokenUsage,
    pub duration_ms: i64,
    pub source: UsageSource,
    pub auto_compact_threshold: Option<i64>,
    pub event_tx: Option<&'a mpsc::Sender<CoreEvent>>,
}

impl std::fmt::Debug for UsageAccounting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsageAccounting")
            .field("base_attribution", &self.base_attribution)
            .finish_non_exhaustive()
    }
}

impl UsageAccounting {
    pub fn new(
        session_id: Arc<RwLock<String>>,
        tracker: Arc<Mutex<CostTracker>>,
        write_lock: Arc<Mutex<()>>,
        base_attribution: UsageAttribution,
    ) -> Self {
        Self {
            session_id,
            tracker,
            write_lock,
            transcript_store: None,
            persist_session: false,
            event_tx: None,
            base_attribution,
        }
    }

    pub fn for_static_session(
        session_id: impl Into<String>,
        tracker: Arc<Mutex<CostTracker>>,
        write_lock: Arc<Mutex<()>>,
        base_attribution: UsageAttribution,
    ) -> Self {
        Self::new(
            Arc::new(RwLock::new(session_id.into())),
            tracker,
            write_lock,
            base_attribution,
        )
    }

    pub fn with_persistence(
        mut self,
        transcript_store: Arc<dyn coco_session::SessionStore>,
        persist_session: bool,
    ) -> Self {
        self.transcript_store = Some(transcript_store);
        self.persist_session = persist_session;
        self
    }

    pub fn with_event_tx(mut self, event_tx: Arc<RwLock<Option<mpsc::Sender<CoreEvent>>>>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    pub fn with_base_attribution(mut self, attribution: UsageAttribution) -> Self {
        self.base_attribution = attribution;
        self
    }

    pub fn tracker(&self) -> Arc<Mutex<CostTracker>> {
        self.tracker.clone()
    }

    pub fn write_lock(&self) -> Arc<Mutex<()>> {
        self.write_lock.clone()
    }

    pub async fn snapshot(&self) -> SessionUsageSnapshot {
        let session_id = self.session_id.read().await.clone();
        self.tracker.lock().await.snapshot(session_id)
    }

    pub async fn record_snapshot_usage(
        &self,
        snapshot: &ModelRuntimeSnapshot,
        usage: TokenUsage,
        duration_ms: i64,
        source: UsageSource,
    ) {
        self.record_usage(UsageRecord {
            provider: &snapshot.provider,
            model_id: &snapshot.model_id,
            usage,
            duration_ms,
            source,
            auto_compact_threshold: None,
            event_tx: None,
        })
        .await;
    }

    pub async fn record_usage(&self, record: UsageRecord<'_>) {
        let UsageRecord {
            provider,
            model_id,
            usage,
            duration_ms,
            source,
            auto_compact_threshold,
            event_tx,
        } = record;
        if usage.total() <= 0 {
            return;
        }

        let _write_guard = self.write_lock.lock().await;
        let session_id = self.session_id.read().await.clone();
        let usage_snapshot = {
            let mut guard = self.tracker.lock().await;
            guard.record_usage_attributed(
                provider,
                model_id,
                usage,
                duration_ms,
                self.attribution_for(source),
            );
            let mut snapshot = guard.snapshot(&session_id);
            snapshot.auto_compact_threshold = auto_compact_threshold;
            snapshot
        };

        if self.persist_session
            && let Some(store) = &self.transcript_store
        {
            let store = Arc::clone(store);
            let session_id_for_write = session_id.clone();
            let snapshot_for_write = usage_snapshot.clone();
            match tokio::task::spawn_blocking(move || {
                store.write_usage_snapshot(&session_id_for_write, &snapshot_for_write)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        session_id,
                        "failed to write usage snapshot"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session_id,
                        "usage snapshot write task failed"
                    );
                }
            }
        }

        if let Some(tx) = event_tx {
            let _ = tx
                .send(CoreEvent::Protocol(
                    ServerNotification::SessionUsageUpdated(Box::new(usage_snapshot)),
                ))
                .await;
            return;
        }

        if let Some(event_tx) = &self.event_tx
            && let Some(tx) = event_tx.read().await.clone()
        {
            let _ = tx
                .send(CoreEvent::Protocol(
                    ServerNotification::SessionUsageUpdated(Box::new(usage_snapshot)),
                ))
                .await;
        }
    }

    fn attribution_for(&self, source: UsageSource) -> UsageAttribution {
        let mut attribution = self.base_attribution.clone();
        attribution.source = source;
        attribution
    }
}
