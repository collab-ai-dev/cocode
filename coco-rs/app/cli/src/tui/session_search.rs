//! Debounce and cancellation ownership for `/resume` transcript search.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use coco_query::CoreEvent;
use coco_types::TuiOnlyEvent;
use tokio::sync::mpsc;

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(120);

#[derive(Clone, Default)]
pub(super) struct SessionSearchGate {
    generation: Arc<AtomicU64>,
    scan_lock: Arc<std::sync::Mutex<()>>,
}

impl SessionSearchGate {
    pub(super) fn start(&self) -> SessionSearchRequest {
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        SessionSearchRequest {
            generations: Arc::clone(&self.generation),
            scan_lock: Arc::clone(&self.scan_lock),
            generation,
        }
    }

    /// Spawn the complete `/resume` content-search lane: debounce, serialized
    /// blocking disk scan, bounded batches, and one terminal completion batch.
    /// Keeping this orchestration beside the gate makes the driver arm a thin,
    /// exhaustively matched dispatch boundary and lets the whole lane be tested.
    pub(super) fn spawn(
        &self,
        manager: Arc<coco_session::SessionManager>,
        event_tx: mpsc::Sender<CoreEvent>,
        query: String,
        request_id: u64,
    ) -> tokio::task::JoinHandle<()> {
        let request = self.start();
        tokio::spawn(async move {
            if !request.wait_for_debounce().await {
                return;
            }
            if query.trim().is_empty() {
                let _ = event_tx
                    .send(CoreEvent::Tui(TuiOnlyEvent::SessionSearchResults {
                        query,
                        request_id,
                        hits: Vec::new(),
                        complete: true,
                    }))
                    .await;
                return;
            }

            let task = tokio::task::spawn_blocking(move || {
                const BATCH_SIZE: usize = 20;
                let mut batch = Vec::with_capacity(BATCH_SIZE);
                let send_batch = |hits: &mut Vec<coco_types::SessionSearchHit>, complete| {
                    let _ = event_tx.blocking_send(CoreEvent::Tui(
                        TuiOnlyEvent::SessionSearchResults {
                            query: query.clone(),
                            request_id,
                            hits: std::mem::take(hits),
                            complete,
                        },
                    ));
                };
                let result = request.run_if_current(|| {
                    manager.search_content(
                        &query,
                        || !request.is_current(),
                        |hit| {
                            let Ok(session_id) = coco_types::SessionId::try_new(hit.session_id)
                            else {
                                return;
                            };
                            batch.push(coco_types::SessionSearchHit {
                                session_id,
                                snippet: hit.snippet,
                            });
                            if batch.len() == BATCH_SIZE {
                                send_batch(&mut batch, /*complete*/ false);
                            }
                        },
                    )
                });
                if let Some(Err(error)) = result {
                    tracing::warn!(%error, "session content search failed");
                }
                if request.is_current() {
                    send_batch(&mut batch, /*complete*/ true);
                }
            });
            if let Err(error) = task.await {
                tracing::warn!(%error, "session content search task failed");
            }
        })
    }
}

pub(super) struct SessionSearchRequest {
    generations: Arc<AtomicU64>,
    scan_lock: Arc<std::sync::Mutex<()>>,
    generation: u64,
}

impl SessionSearchRequest {
    pub(super) async fn wait_for_debounce(&self) -> bool {
        tokio::time::sleep(SEARCH_DEBOUNCE).await;
        self.is_current()
    }

    pub(super) fn is_current(&self) -> bool {
        self.generations.load(Ordering::Acquire) == self.generation
    }

    /// Serialize blocking scans and re-check freshness after acquiring the
    /// slot. Cancelled searches therefore cannot overlap whole-file I/O with
    /// the current request even if the blocking pool schedules them late.
    pub(super) fn run_if_current<T>(&self, run: impl FnOnce() -> T) -> Option<T> {
        let _scan = self
            .scan_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.is_current().then(run)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_matching_transcript(memory_base: &std::path::Path, session_id: &str) {
        let paths = Arc::new(coco_paths::ProjectPaths::new(
            memory_base.to_path_buf(),
            std::path::Path::new("/session-search-test"),
        ));
        let store = coco_session::TranscriptStore::new(paths);
        store
            .append_message(
                session_id,
                &coco_session::TranscriptEntry {
                    entry_type: "user".to_string(),
                    uuid: format!("{session_id}-user"),
                    parent_uuid: None,
                    logical_parent_uuid: None,
                    session_id: Some(
                        coco_types::SessionId::try_new(session_id.to_string()).expect("session id"),
                    ),
                    cwd: "/session-search-test".to_string(),
                    timestamp: "2026-07-18T00:00:00Z".to_string(),
                    version: Some("test".to_string()),
                    git_branch: None,
                    is_sidechain: false,
                    agent_id: None,
                    message: Some(serde_json::json!({
                        "role": "user",
                        "content": [{"type": "text", "text": "batch needle"}],
                    })),
                    usage: None,
                    model: None,
                    request_id: None,
                    cost_usd: None,
                    extra: serde_json::Map::new(),
                },
            )
            .expect("seed transcript");
    }

    #[tokio::test]
    async fn newer_request_cancels_older_debounce_generation() {
        let gate = SessionSearchGate::default();
        let stale = gate.start();
        let current = gate.start();

        let (stale_ready, current_ready) =
            tokio::join!(stale.wait_for_debounce(), current.wait_for_debounce());

        assert!(!stale_ready);
        assert!(current_ready);
    }

    #[test]
    fn running_scan_observes_a_new_generation() {
        let gate = SessionSearchGate::default();
        let running = gate.start();
        assert!(running.is_current());

        let replacement = gate.start();

        assert!(!running.is_current());
        assert!(replacement.is_current());
    }

    #[test]
    fn blocking_scans_are_single_flight() {
        let gate = SessionSearchGate::default();
        let held_scan = gate
            .scan_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let second = gate.start();
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran_worker = Arc::clone(&ran);
        let (attempted_tx, attempted_rx) = std::sync::mpsc::channel();
        let second_worker = std::thread::spawn(move || {
            attempted_tx.send(()).expect("attempted scan");
            second.run_if_current(|| {
                ran_worker.store(true, Ordering::Release);
            })
        });

        attempted_rx.recv().expect("worker reached scan boundary");
        assert!(
            !ran.load(Ordering::Acquire),
            "the held scan lock must prevent a second scan from entering"
        );
        drop(held_scan);
        second_worker.join().expect("second worker");

        assert!(ran.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn driver_lane_streams_twenty_hit_batches_and_one_completion() {
        let dir = tempfile::tempdir().expect("tempdir");
        for index in 0..21 {
            seed_matching_transcript(dir.path(), &format!("search-{index:02}"));
        }
        let manager = Arc::new(coco_session::SessionManager::new(dir.path().to_path_buf()));
        let (event_tx, mut event_rx) = mpsc::channel(4);

        SessionSearchGate::default()
            .spawn(manager, event_tx, "needle".to_string(), 77)
            .await
            .expect("search task");

        let mut batches = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            let CoreEvent::Tui(TuiOnlyEvent::SessionSearchResults {
                query,
                request_id,
                hits,
                complete,
            }) = event
            else {
                panic!("unexpected event: {event:?}");
            };
            assert_eq!(query, "needle");
            assert_eq!(request_id, 77);
            batches.push((hits.len(), complete));
        }

        assert_eq!(batches, vec![(20, false), (1, true)]);
    }
}
