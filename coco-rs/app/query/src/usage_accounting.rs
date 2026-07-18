use std::sync::Arc;

use coco_inference::ModelRuntimeSnapshot;
use coco_messages::CostTracker;
use coco_types::CoreEvent;
use coco_types::ServerNotification;
use coco_types::SessionId;
use coco_types::SessionUsageSnapshot;
use coco_types::TokenUsage;
use coco_types::UsageAttribution;
use coco_types::UsageSource;
use tokio::sync::Mutex;
use tokio::sync::RwLock;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct UsageAccounting {
    session_id: Arc<RwLock<SessionId>>,
    tracker: Arc<Mutex<CostTracker>>,
    write_lock: Arc<Mutex<()>>,
    transcript_store: Option<Arc<dyn coco_session::SessionStore>>,
    persist_session: bool,
    snapshot_tx: Arc<RwLock<Option<mpsc::Sender<SessionUsageSnapshot>>>>,
    base_attribution: UsageAttribution,
    mirror: Arc<RwLock<Option<UsageMirror>>>,
}

#[derive(Clone)]
struct UsageMirror {
    accounting: UsageAccounting,
    source: UsageSource,
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
    pub fn new(session_id: SessionId, base_attribution: UsageAttribution) -> Self {
        Self::with_tracker(
            session_id,
            Arc::new(Mutex::new(CostTracker::default())),
            Arc::new(Mutex::new(())),
            base_attribution,
        )
    }

    fn with_tracker(
        session_id: SessionId,
        tracker: Arc<Mutex<CostTracker>>,
        write_lock: Arc<Mutex<()>>,
        base_attribution: UsageAttribution,
    ) -> Self {
        Self {
            session_id: Arc::new(RwLock::new(session_id)),
            tracker,
            write_lock,
            transcript_store: None,
            persist_session: false,
            snapshot_tx: Arc::new(RwLock::new(None)),
            base_attribution,
            mirror: Arc::new(RwLock::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn for_static_session(
        session_id: SessionId,
        tracker: Arc<Mutex<CostTracker>>,
        write_lock: Arc<Mutex<()>>,
        base_attribution: UsageAttribution,
    ) -> Self {
        Self::with_tracker(session_id, tracker, write_lock, base_attribution)
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

    /// Install the typed sink for usage produced outside a turn surface.
    ///
    /// The host owns conversion into transport- or UI-specific envelopes so
    /// query accounting never emits an unscoped event into a multi-session UI.
    pub async fn install_snapshot_tx(&self, snapshot_tx: mpsc::Sender<SessionUsageSnapshot>) {
        *self.snapshot_tx.write().await = Some(snapshot_tx);
    }

    /// Mirror future records into another session's authoritative accounting.
    ///
    /// Sidechat uses this to keep its own ephemeral totals for the active child
    /// view while charging the same calls to the durable parent session.
    pub async fn install_mirror(&self, accounting: UsageAccounting, source: UsageSource) {
        *self.mirror.write().await = Some(UsageMirror { accounting, source });
    }

    pub fn with_base_attribution(mut self, attribution: UsageAttribution) -> Self {
        self.base_attribution = attribution;
        self
    }

    pub async fn load_current_session_tracker_from_store(&self) {
        let _write_guard = self.write_lock.lock().await;
        let session_id = self.session_id.read().await.clone();
        let tracker = self.load_tracker_from_store(&session_id).await;
        self.replace_tracker(tracker).await;
    }

    pub async fn retarget_to_loaded_session(&self, session_id: SessionId) {
        let _write_guard = self.write_lock.lock().await;
        let tracker = self.load_tracker_from_store(&session_id).await;
        self.retarget_session_id(session_id).await;
        self.replace_tracker(tracker).await;
    }

    pub async fn retarget_to_empty_session(&self, session_id: SessionId) {
        let _write_guard = self.write_lock.lock().await;
        self.retarget_session_id(session_id).await;
        self.reset_tracker().await;
    }

    async fn retarget_session_id(&self, session_id: SessionId) {
        *self.session_id.write().await = session_id;
    }

    async fn replace_tracker(&self, tracker: CostTracker) {
        *self.tracker.lock().await = tracker;
    }

    async fn reset_tracker(&self) {
        self.replace_tracker(CostTracker::new()).await;
    }

    async fn load_tracker_from_store(&self, session_id: &SessionId) -> CostTracker {
        let Some(store) = &self.transcript_store else {
            return CostTracker::new();
        };

        let store = Arc::clone(store);
        let session_id_for_load = session_id.to_string();
        let session_id_for_log = session_id_for_load.clone();
        match tokio::task::spawn_blocking(move || store.load_usage_snapshot(&session_id_for_load))
            .await
        {
            Ok(Ok(Some(snapshot))) => CostTracker::from_snapshot(snapshot),
            Ok(Ok(None)) => CostTracker::new(),
            Ok(Err(e)) => {
                tracing::warn!(
                    error = %e,
                    session_id = %session_id_for_log,
                    "failed to load session usage snapshot"
                );
                CostTracker::new()
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    session_id = %session_id_for_log,
                    "session usage load task failed"
                );
                CostTracker::new()
            }
        }
    }

    pub async fn snapshot(&self) -> SessionUsageSnapshot {
        let session_id = self.session_id.read().await.clone();
        self.tracker.lock().await.snapshot(session_id)
    }

    pub async fn flush_snapshot(&self) {
        if !self.persist_session {
            return;
        }

        let Some(store) = &self.transcript_store else {
            return;
        };

        let _write_guard = self.write_lock.lock().await;
        let session_id = self.session_id.read().await.clone();
        let snapshot = self.tracker.lock().await.snapshot(session_id.clone());
        let store = Arc::clone(store);
        let session_id_for_write = session_id.clone();
        match tokio::task::spawn_blocking(move || {
            store.write_usage_snapshot(session_id_for_write.as_str(), &snapshot)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                tracing::warn!(error = %e, session_id = %session_id, "failed to flush usage snapshot");
            }
            Err(e) => {
                tracing::warn!(error = %e, session_id = %session_id, "usage snapshot flush task failed");
            }
        }
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
        let mirror = self.mirror.read().await.clone();
        let mirrored_usage = record.usage;
        let mirrored_duration_ms = record.duration_ms;
        let mirrored_provider = record.provider;
        let mirrored_model_id = record.model_id;
        let mirrored_auto_compact_threshold = record.auto_compact_threshold;
        // The parent is authoritative and durable. Charge it before touching
        // the ephemeral child so cancellation can never leave provider spend
        // visible only in a child ledger that is about to be discarded.
        if let Some(mirror) = mirror {
            mirror
                .accounting
                .record_usage_locally(UsageRecord {
                    provider: mirrored_provider,
                    model_id: mirrored_model_id,
                    usage: mirrored_usage,
                    duration_ms: mirrored_duration_ms,
                    source: mirror.source,
                    auto_compact_threshold: mirrored_auto_compact_threshold,
                    event_tx: None,
                })
                .await;
        }
        self.record_usage_locally(record).await;
    }

    async fn record_usage_locally(&self, record: UsageRecord<'_>) {
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
            let mut snapshot = guard.snapshot(session_id.clone());
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
                store.write_usage_snapshot(session_id_for_write.as_str(), &snapshot_for_write)
            })
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        error = %e,
                        session_id = %session_id,
                        "failed to write usage snapshot"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        session_id = %session_id,
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

        if let Some(tx) = self.snapshot_tx.read().await.clone() {
            let _ = tx.send(usage_snapshot).await;
        }
    }

    fn attribution_for(&self, source: UsageSource) -> UsageAttribution {
        let mut attribution = self.base_attribution.clone();
        attribution.source = source;
        attribution
    }
}

#[cfg(test)]
#[path = "usage_accounting.test.rs"]
mod tests;
