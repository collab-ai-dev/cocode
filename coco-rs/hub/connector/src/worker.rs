use std::collections::VecDeque;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use chrono::Utc;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchAckFrame;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::EventEnvelope;
use coco_types::SessionEnvelope;
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
    tx: mpsc::Sender<SessionEnvelope>,
}

impl HubConnectorSender {
    pub async fn enqueue(&self, envelope: SessionEnvelope) -> Result<(), HubConnectorQueueError> {
        self.tx
            .send(envelope)
            .await
            .map_err(|_| HubConnectorQueueError::Closed)
    }

    pub fn try_enqueue(&self, envelope: SessionEnvelope) -> Result<(), HubConnectorQueueError> {
        self.tx.try_send(envelope).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => HubConnectorQueueError::Full,
            mpsc::error::TrySendError::Closed(_) => HubConnectorQueueError::Closed,
        })
    }
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
        let join = tokio::spawn(run_worker(config, rx, shutdown_rx));
        Ok(Self {
            sender: HubConnectorSender { tx },
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
    config: HubConnectorWorkerConfig,
    mut rx: mpsc::Receiver<SessionEnvelope>,
    mut shutdown_rx: oneshot::Receiver<()>,
) -> Result<HubConnectorWorkerStats, HubConnectorWorkerError> {
    let mut pending = VecDeque::with_capacity(config.pending_capacity);
    let mut stats = HubConnectorWorkerStats::default();
    let mut client = None;
    let mut retry_delay = config.reconnect_initial_delay;
    let mut flush_tick = interval(config.flush_interval);
    flush_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut shutting_down = false;

    loop {
        if shutting_down {
            drain_ready(&config, &mut rx, &mut pending, &mut stats)?;
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
                let Some(envelope) = maybe_envelope else {
                    shutting_down = true;
                    continue;
                };
                push_envelope(&config, &mut pending, &mut stats, envelope)?;
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
    config: &HubConnectorWorkerConfig,
    rx: &mut mpsc::Receiver<SessionEnvelope>,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
) -> Result<(), HubConnectorWorkerError> {
    while pending.len() < config.pending_capacity {
        match rx.try_recv() {
            Ok(envelope) => push_envelope(config, pending, stats, envelope)?,
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {
                break;
            }
        }
    }
    Ok(())
}

fn push_envelope(
    config: &HubConnectorWorkerConfig,
    pending: &mut VecDeque<EventEnvelope>,
    stats: &mut HubConnectorWorkerStats,
    envelope: SessionEnvelope,
) -> Result<(), HubConnectorWorkerError> {
    if let Some(event) =
        event_envelope_from_session_envelope(config.announce.instance_id, Utc::now(), envelope)?
    {
        pending.push_back(event);
    } else {
        stats.skipped_ephemeral_events += 1;
    }
    Ok(())
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
        connected.announce(config.announce.clone()).await?;
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
    let ack = active_client.send_batch(BatchFrame { events }).await?;
    let shipped = pop_acked_front(pending, &ack);
    stats.shipped_events += i64::try_from(shipped).map_err(|_| {
        HubConnectorError::Protocol("shipped event count overflowed i64".to_string())
    })?;
    Ok(())
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
