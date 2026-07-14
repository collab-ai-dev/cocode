use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use chrono::Utc;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchAckFrame;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::EventEnvelope;
use coco_hub_protocol::EventPayload;
use coco_types::SessionEnvelope;
use coco_types::SessionId;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio::time::interval;
use tokio::time::sleep;

use crate::EnvelopeEgressError;
use crate::HubConnectorClient;
use crate::HubConnectorError;
use crate::event_envelope_from_session_envelope;

#[derive(Debug, Clone)]
pub struct HubConnectorWorkerConfig {
    pub url: String,
    pub announce: AnnounceFrame,
    pub channel_capacity: usize,
    pub pending_capacity: usize,
    pub batch_max_events: usize,
    pub batch_max_bytes: usize,
    pub flush_interval: Duration,
    pub reconnect_initial_delay: Duration,
    pub reconnect_max_delay: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HubConnectorWorkerStats {
    pub shipped_events: i64,
    pub skipped_ephemeral_events: i64,
    pub dropped_durable_events: i64,
    /// Pending events skipped at announce time because `announce_ack.resume_from`
    /// showed the hub already durably stored them (not data loss).
    pub trimmed_resumed_events: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum HubConnectorWorkerError {
    #[error("invalid hub connector worker config: {0}")]
    InvalidConfig(String),
    #[error("failed to convert envelope for hub egress: {0}")]
    Envelope(#[from] EnvelopeEgressError),
    #[error("hub connector worker task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

#[derive(Debug, thiserror::Error)]
pub enum HubConnectorQueueError {
    #[error("hub connector queue is closed")]
    Closed,
    #[error("hub connector queue is full")]
    Full,
}

#[derive(Debug, Clone)]
pub struct HubConnectorSender {
    tx: mpsc::Sender<HubConnectorCommand>,
    dropped: Arc<Mutex<DroppedEventRanges>>,
}

impl HubConnectorSender {
    pub async fn enqueue(&self, envelope: SessionEnvelope) -> Result<(), HubConnectorQueueError> {
        self.tx
            .send(HubConnectorCommand::Envelope(Box::new(envelope)))
            .await
            .map_err(|_| HubConnectorQueueError::Closed)
    }

    pub async fn update_announce(
        &self,
        announce: AnnounceFrame,
    ) -> Result<(), HubConnectorQueueError> {
        self.tx
            .send(HubConnectorCommand::UpdateAnnounce(announce))
            .await
            .map_err(|_| HubConnectorQueueError::Closed)
    }

    pub fn try_enqueue(&self, envelope: SessionEnvelope) -> Result<(), HubConnectorQueueError> {
        self.tx
            .try_send(HubConnectorCommand::Envelope(Box::new(envelope)))
            .map_err(|err| match err {
                mpsc::error::TrySendError::Full(HubConnectorCommand::Envelope(envelope)) => {
                    record_dropped_envelope(&self.dropped, &envelope);
                    HubConnectorQueueError::Full
                }
                mpsc::error::TrySendError::Full(HubConnectorCommand::UpdateAnnounce(_)) => {
                    HubConnectorQueueError::Full
                }
                mpsc::error::TrySendError::Closed(_) => HubConnectorQueueError::Closed,
            })
    }
}

#[derive(Debug)]
enum HubConnectorCommand {
    Envelope(Box<SessionEnvelope>),
    UpdateAnnounce(AnnounceFrame),
}

pub struct HubConnectorWorker {
    sender: HubConnectorSender,
    shutdown_tx: oneshot::Sender<()>,
    join: JoinHandle<Result<HubConnectorWorkerStats, HubConnectorWorkerError>>,
}

impl HubConnectorWorker {
    pub fn spawn(config: HubConnectorWorkerConfig) -> Result<Self, HubConnectorWorkerError> {
        validate_config(&config)?;
        let (tx, rx) = mpsc::channel(config.channel_capacity);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let dropped = Arc::new(Mutex::new(DroppedEventRanges::default()));
        let join = tokio::spawn(run_worker(config, rx, shutdown_rx, Arc::clone(&dropped)));
        Ok(Self {
            sender: HubConnectorSender { tx, dropped },
            shutdown_tx,
            join,
        })
    }

    pub fn sender(&self) -> HubConnectorSender {
        self.sender.clone()
    }

    pub async fn shutdown_and_flush(
        self,
    ) -> Result<HubConnectorWorkerStats, HubConnectorWorkerError> {
        let _ = self.shutdown_tx.send(());
        self.join.await?
    }
}

fn validate_config(config: &HubConnectorWorkerConfig) -> Result<(), HubConnectorWorkerError> {
    if config.url.is_empty() {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "url must not be empty".to_string(),
        ));
    }
    if config.channel_capacity == 0 {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "channel_capacity must be greater than zero".to_string(),
        ));
    }
    if config.pending_capacity == 0 {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "pending_capacity must be greater than zero".to_string(),
        ));
    }
    if config.batch_max_events == 0 {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "batch_max_events must be greater than zero".to_string(),
        ));
    }
    if config.batch_max_bytes == 0 {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "batch_max_bytes must be greater than zero".to_string(),
        ));
    }
    if config.flush_interval.is_zero() {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "flush_interval must be greater than zero".to_string(),
        ));
    }
    if config.reconnect_initial_delay.is_zero() {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "reconnect_initial_delay must be greater than zero".to_string(),
        ));
    }
    if config.reconnect_max_delay < config.reconnect_initial_delay {
        return Err(HubConnectorWorkerError::InvalidConfig(
            "reconnect_max_delay must be at least reconnect_initial_delay".to_string(),
        ));
    }
    Ok(())
}

async fn run_worker(
    mut config: HubConnectorWorkerConfig,
    mut rx: mpsc::Receiver<HubConnectorCommand>,
    mut shutdown_rx: oneshot::Receiver<()>,
    dropped: Arc<Mutex<DroppedEventRanges>>,
) -> Result<HubConnectorWorkerStats, HubConnectorWorkerError> {
    let mut pending = VecDeque::with_capacity(config.pending_capacity);
    let mut stats = HubConnectorWorkerStats::default();
    let mut client = None;
    let mut retry_delay = config.reconnect_initial_delay;
    let mut flush_tick = interval(config.flush_interval);
    flush_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut shutting_down = false;

    loop {
        if !shutting_down && client.is_none() {
            drain_ready(
                &mut config,
                &mut client,
                &mut rx,
                &mut pending,
                &mut stats,
                &dropped,
            )?;
            if pending.is_empty() && rx.is_empty() {
                push_all_dropped_markers(&config, &dropped, &mut pending, &mut stats);
            }
            deliver_or_backoff(
                &config,
                &mut client,
                &mut pending,
                &mut retry_delay,
                &mut stats,
            )
            .await;
            continue;
        }
        if pending.is_empty() && rx.is_empty() {
            push_all_dropped_markers(&config, &dropped, &mut pending, &mut stats);
        }
        if shutting_down {
            drain_ready(
                &mut config,
                &mut client,
                &mut rx,
                &mut pending,
                &mut stats,
                &dropped,
            )?;
            push_all_dropped_markers(&config, &dropped, &mut pending, &mut stats);
            if pending.is_empty() {
                return Ok(stats);
            }
            deliver_or_backoff(
                &config,
                &mut client,
                &mut pending,
                &mut retry_delay,
                &mut stats,
            )
            .await;
            continue;
        }

        if pending.len() >= config.batch_max_events {
            deliver_or_backoff(
                &config,
                &mut client,
                &mut pending,
                &mut retry_delay,
                &mut stats,
            )
            .await;
            continue;
        }

        tokio::select! {
            _ = &mut shutdown_rx => {
                shutting_down = true;
            }
            maybe_envelope = rx.recv(), if pending.len() < config.pending_capacity => {
                let Some(command) = maybe_envelope else {
                    shutting_down = true;
                    continue;
                };
                handle_command(&mut config, &mut client, &mut pending, &mut stats, &dropped, command)?;
            }
            _ = flush_tick.tick(), if !pending.is_empty() => {
                deliver_or_backoff(
                    &config,
                    &mut client,
                    &mut pending,
                    &mut retry_delay,
                    &mut stats,
                )
                .await;
            }
        }
    }
}

fn drain_ready(
    config: &mut HubConnectorWorkerConfig,
    client: &mut Option<HubConnectorClient>,
    rx: &mut mpsc::Receiver<HubConnectorCommand>,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    dropped: &Arc<Mutex<DroppedEventRanges>>,
) -> Result<(), HubConnectorWorkerError> {
    while pending.len() < config.pending_capacity {
        match rx.try_recv() {
            Ok(command) => handle_command(config, client, pending, stats, dropped, command)?,
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                break;
            }
        }
    }
    Ok(())
}

fn handle_command(
    config: &mut HubConnectorWorkerConfig,
    client: &mut Option<HubConnectorClient>,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    dropped: &Arc<Mutex<DroppedEventRanges>>,
    command: HubConnectorCommand,
) -> Result<(), HubConnectorWorkerError> {
    match command {
        HubConnectorCommand::Envelope(envelope) => {
            push_envelope(config, pending, stats, dropped, *envelope)
        }
        HubConnectorCommand::UpdateAnnounce(announce) => {
            config.announce = announce;
            *client = None;
            Ok(())
        }
    }
}

fn push_envelope(
    config: &HubConnectorWorkerConfig,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    dropped: &Arc<Mutex<DroppedEventRanges>>,
    envelope: SessionEnvelope,
) -> Result<(), HubConnectorWorkerError> {
    if let Some(event) =
        event_envelope_from_session_envelope(config.announce.instance_id, Utc::now(), envelope)?
    {
        push_dropped_marker_before(config, dropped, pending, stats, &event);
        pending.push_back(event);
    } else {
        stats.skipped_ephemeral_events += 1;
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct DroppedEventRange {
    count: i64,
    since_seq: i64,
    until_seq: i64,
}

#[derive(Debug, Default)]
struct DroppedEventRanges {
    by_session: HashMap<SessionId, DroppedEventRange>,
}

fn record_dropped_envelope(dropped: &Arc<Mutex<DroppedEventRanges>>, envelope: &SessionEnvelope) {
    let Some(session_seq) = envelope.session_seq else {
        return;
    };
    let mut dropped = dropped
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    dropped
        .by_session
        .entry(envelope.session_id.clone())
        .and_modify(|range| {
            range.count = range.count.saturating_add(1);
            range.since_seq = range.since_seq.min(session_seq);
            range.until_seq = range.until_seq.max(session_seq);
        })
        .or_insert(DroppedEventRange {
            count: 1,
            since_seq: session_seq,
            until_seq: session_seq,
        });
}

fn push_dropped_marker_before(
    config: &HubConnectorWorkerConfig,
    dropped: &Arc<Mutex<DroppedEventRanges>>,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    next_event: &EventEnvelope,
) {
    let marker = {
        let mut dropped = dropped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(range) = dropped.by_session.get(&next_event.session_id) else {
            return;
        };
        if range.until_seq >= next_event.session_seq {
            return;
        }
        dropped.by_session.remove(&next_event.session_id)
    };
    if let Some(marker) = marker {
        push_dropped_marker(
            config,
            pending,
            stats,
            next_event.session_id.clone(),
            marker,
        );
    }
}

fn push_all_dropped_markers(
    config: &HubConnectorWorkerConfig,
    dropped: &Arc<Mutex<DroppedEventRanges>>,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
) {
    if pending.len() >= config.pending_capacity {
        return;
    }
    let ranges = {
        let mut dropped = dropped
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut dropped.by_session)
    };
    for (session_id, range) in ranges {
        push_dropped_marker(config, pending, stats, session_id, range);
    }
}

fn push_dropped_marker(
    config: &HubConnectorWorkerConfig,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    session_id: SessionId,
    range: DroppedEventRange,
) {
    push_dropped_marker_with_reason(
        config,
        pending,
        stats,
        session_id,
        range,
        "hub_connector_backlog_full",
    );
}

fn push_dropped_marker_with_reason(
    config: &HubConnectorWorkerConfig,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    session_id: SessionId,
    range: DroppedEventRange,
    reason: &str,
) {
    stats.dropped_durable_events = stats.dropped_durable_events.saturating_add(range.count);
    pending.push_back(EventEnvelope {
        instance_id: config.announce.instance_id,
        session_id,
        agent_id: None,
        session_seq: range.until_seq,
        ts: Utc::now(),
        schema_version: coco_hub_protocol::SCHEMA_VERSION_V2,
        payload: EventPayload::EventsDropped {
            count: range.count,
            since_seq: range.since_seq,
            until_seq: range.until_seq,
            reason: reason.to_string(),
        },
    });
}

/// A hub error the connector must NOT retry: the same batch will fail again,
/// so retrying it forever wedges all later events behind it. The offending
/// batch is dropped with `events_dropped` markers so per-session cursors can
/// still advance.
fn is_non_retriable(error: &HubConnectorError) -> bool {
    match error {
        HubConnectorError::HubError { code, .. } => matches!(
            code.as_str(),
            "invalid_json" | "instance_mismatch" | "unsupported_frame"
        ),
        HubConnectorError::Serialize(_) => true,
        _ => false,
    }
}

/// Drop the front `batch_len` events (the batch a non-retriable error just
/// rejected) and record them as `events_dropped` so cursors advance. Existing
/// dropped-markers in that range are re-queued rather than re-dropped.
fn drop_front_batch(
    config: &HubConnectorWorkerConfig,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    batch_len: usize,
    reason: &str,
) {
    let mut ranges: std::collections::HashMap<SessionId, DroppedEventRange> =
        std::collections::HashMap::new();
    for _ in 0..batch_len {
        let Some(event) = pending.pop_front() else {
            break;
        };
        if matches!(event.payload, EventPayload::EventsDropped { .. }) {
            pending.push_back(event);
            continue;
        }
        ranges
            .entry(event.session_id.clone())
            .and_modify(|range| {
                range.count = range.count.saturating_add(1);
                range.since_seq = range.since_seq.min(event.session_seq);
                range.until_seq = range.until_seq.max(event.session_seq);
            })
            .or_insert(DroppedEventRange {
                count: 1,
                since_seq: event.session_seq,
                until_seq: event.session_seq,
            });
    }
    for (session_id, range) in ranges {
        push_dropped_marker_with_reason(config, pending, stats, session_id, range, reason);
    }
}

async fn deliver_or_backoff(
    config: &HubConnectorWorkerConfig,
    client: &mut Option<HubConnectorClient>,
    pending: &mut VecDeque<EventEnvelope>,
    retry_delay: &mut Duration,
    stats: &mut HubConnectorWorkerStats,
) {
    match deliver_once(config, client, pending, stats).await {
        Ok(()) => {
            *retry_delay = config.reconnect_initial_delay;
        }
        Err(_) => {
            *client = None;
            sleep(jittered_retry_delay(
                *retry_delay,
                config.reconnect_max_delay,
            ))
            .await;
            *retry_delay = (*retry_delay)
                .saturating_mul(2)
                .min(config.reconnect_max_delay);
        }
    }
}

async fn deliver_once(
    config: &HubConnectorWorkerConfig,
    client: &mut Option<HubConnectorClient>,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
) -> Result<(), HubConnectorError> {
    if client.is_none() {
        let mut connected = HubConnectorClient::connect(&config.url).await?;
        let ack = connected.announce(config.announce.clone()).await?;
        trim_pending_already_stored(pending, &ack.resume_from, stats);
        *client = Some(connected);
    }

    let events = batch_events_within_limits(config, pending)?;
    if events.is_empty() {
        return Ok(());
    }

    let Some(active_client) = client.as_mut() else {
        return Err(HubConnectorError::Protocol(
            "connector client was not initialized".to_string(),
        ));
    };
    let batch_len = events.len();
    match active_client.send_batch(BatchFrame { events }).await {
        Ok(ack) => {
            let shipped = pop_acked_front(pending, &ack);
            stats.shipped_events += i64::try_from(shipped).map_err(|_| {
                HubConnectorError::Protocol("shipped event count overflowed i64".to_string())
            })?;
            Ok(())
        }
        Err(error) if is_non_retriable(&error) => {
            tracing::warn!(%error, batch_len, "dropping hub batch rejected as non-retriable");
            drop_front_batch(
                config,
                pending,
                stats,
                batch_len,
                "hub_rejected_non_retriable",
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn batch_events_within_limits(
    config: &HubConnectorWorkerConfig,
    pending: &VecDeque<EventEnvelope>,
) -> Result<Vec<EventEnvelope>, HubConnectorError> {
    let mut events = Vec::new();
    for event in pending.iter().take(config.batch_max_events) {
        let mut candidate = events.clone();
        candidate.push(event.clone());
        let candidate_bytes = serde_json::to_vec(&BatchFrame {
            events: candidate.clone(),
        })?
        .len();
        if !events.is_empty() && candidate_bytes > config.batch_max_bytes {
            break;
        }
        events = candidate;
        if candidate_bytes >= config.batch_max_bytes {
            break;
        }
    }
    Ok(events)
}

/// Drop pending events the hub already durably stored per `announce_ack.resume_from`.
///
/// Replay is `seq > cursor`: an event at or below its session cursor was
/// persisted by a previous connection, so re-sending it only burns bandwidth on
/// hub-side dedup. Sessions absent from the map keep all their events. These
/// drops are not data loss and must never produce `events_dropped` markers.
fn trim_pending_already_stored(
    pending: &mut VecDeque<EventEnvelope>,
    resume_from: &HashMap<SessionId, i64>,
    stats: &mut HubConnectorWorkerStats,
) {
    if resume_from.is_empty() || pending.is_empty() {
        return;
    }
    let before = pending.len();
    pending.retain(|event| {
        resume_from
            .get(&event.session_id)
            .is_none_or(|cursor| event.session_seq > *cursor)
    });
    let trimmed = (before - pending.len()) as i64;
    stats.trimmed_resumed_events = stats.trimmed_resumed_events.saturating_add(trimmed);
}

fn pop_acked_front(pending: &mut VecDeque<EventEnvelope>, ack: &BatchAckFrame) -> usize {
    let mut shipped = 0;
    while let Some(event) = pending.front() {
        let Some(up_to_seq) = ack.up_to_seq.get(&event.session_id) else {
            break;
        };
        if *up_to_seq < event.session_seq {
            break;
        }
        pending.pop_front();
        shipped += 1;
    }
    shipped
}

fn jittered_retry_delay(delay: Duration, max_delay: Duration) -> Duration {
    let delay_nanos = delay.as_nanos();
    if delay_nanos == 0 {
        return delay;
    }
    let jitter_span = (delay_nanos / 5).max(1);
    let bucket_count = jitter_span.saturating_mul(2).saturating_add(1);
    let jitter_bucket = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() % bucket_count)
        .unwrap_or(jitter_span);
    let adjusted = delay_nanos
        .saturating_add(jitter_bucket)
        .saturating_sub(jitter_span)
        .max(1);
    nanos_to_duration(adjusted).min(max_delay)
}

fn nanos_to_duration(nanos: u128) -> Duration {
    let capped = nanos.min(u128::from(u64::MAX));
    Duration::from_nanos(capped as u64)
}

#[cfg(test)]
#[path = "worker.test.rs"]
mod tests;
