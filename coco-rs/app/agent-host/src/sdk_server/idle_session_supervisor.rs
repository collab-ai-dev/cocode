use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use coco_app_server::AppServer;
use coco_types::SessionId;
use futures::{StreamExt, stream::FuturesUnordered};
use tokio::task::JoinHandle;

use super::{
    app_server_bridge::LocalAppSessionHandle, handlers::SdkServerState,
    session_lifecycle::close_local_app_server_session,
};

/// Spawn the optional event-driven idle-session auto-archive supervisor.
///
/// A session is eligible only when it has no attached surface, no active turn,
/// and no queued cross-turn command. AppServer lifecycle/event activity, SDK
/// turn activity, and command-queue changes wake the supervisor immediately;
/// otherwise it sleeps until the earliest exact idle deadline.
pub fn spawn_idle_session_sweep(
    app_server: Arc<AppServer<LocalAppSessionHandle>>,
    state: Arc<SdkServerState>,
    idle_timeout: Duration,
    turn_drain_timeout: Duration,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Subscribe before reading state so a concurrent change is retained
            // by watch even when it lands between the snapshot and select.
            let mut app_activity = app_server.subscribe_session_activity();
            let mut sdk_activity = state.subscribe_session_activity();
            let now = Instant::now();
            let live = app_server.list_live_sessions();
            let mut queue_changes = Vec::new();
            let mut earliest_deadline = None;
            let mut to_archive = Vec::new();

            for summary in &live {
                let session_id = &summary.session_id;
                let runtime = app_server
                    .registry()
                    .get(session_id)
                    .map(|handle| handle.runtime().clone());
                let command_queue = runtime
                    .as_ref()
                    .map(|runtime| runtime.command_queue().clone());
                if let Some(queue) = &command_queue {
                    queue_changes.push(queue.subscribe_changes());
                }

                let queued = match &command_queue {
                    Some(queue) => !queue.is_empty().await,
                    None => false,
                };
                if summary.surface_counts.attached != 0
                    || runtime
                        .as_ref()
                        .is_some_and(crate::session_runtime::SessionHandle::has_active_turn)
                    || queued
                {
                    continue;
                }

                let mut last_activity = app_server
                    .session_last_activity(session_id)
                    .into_iter()
                    .chain(state.session_last_activity(session_id))
                    .max()
                    .unwrap_or(now);
                if let Some(queue) = command_queue {
                    last_activity = last_activity.max(queue.last_changed_at());
                }
                let deadline = last_activity + idle_timeout;
                if deadline <= now {
                    to_archive.push(session_id.clone());
                } else if earliest_deadline.is_none_or(|current| deadline < current) {
                    earliest_deadline = Some(deadline);
                }
            }

            if to_archive.is_empty() {
                wait_for_idle_activity(
                    earliest_deadline,
                    &mut app_activity,
                    &mut sdk_activity,
                    queue_changes,
                )
                .await;
                continue;
            }

            for session_id in to_archive {
                if !idle_session_is_due(&app_server, &state, &session_id, idle_timeout).await {
                    continue;
                }
                tracing::info!(
                    session_id = %session_id,
                    idle_timeout_secs = idle_timeout.as_secs(),
                    "auto-archiving idle session with no surfaces, active turn, or queued command"
                );
                if let Err(error) = close_local_app_server_session(
                    Arc::clone(&app_server),
                    Arc::clone(&state),
                    session_id.clone(),
                    turn_drain_timeout,
                )
                .await
                {
                    tracing::warn!(
                        session_id = %session_id,
                        ?error,
                        "idle-session auto-archive failed"
                    );
                }
            }
        }
    })
}

async fn idle_session_is_due(
    app_server: &AppServer<LocalAppSessionHandle>,
    state: &SdkServerState,
    session_id: &SessionId,
    idle_timeout: Duration,
) -> bool {
    let Some(summary) = app_server
        .list_live_sessions()
        .into_iter()
        .find(|summary| &summary.session_id == session_id)
    else {
        return false;
    };
    let runtime = app_server
        .registry()
        .get(session_id)
        .map(|handle| handle.runtime().clone());
    if summary.surface_counts.attached != 0
        || runtime
            .as_ref()
            .is_some_and(crate::session_runtime::SessionHandle::has_active_turn)
    {
        return false;
    }

    let command_queue = runtime.map(|runtime| runtime.command_queue().clone());
    if let Some(queue) = &command_queue
        && !queue.is_empty().await
    {
        return false;
    }

    let mut last_activity = app_server
        .session_last_activity(session_id)
        .into_iter()
        .chain(state.session_last_activity(session_id))
        .max()
        .unwrap_or_else(Instant::now);
    if let Some(queue) = command_queue {
        last_activity = last_activity.max(queue.last_changed_at());
    }
    Instant::now().duration_since(last_activity) >= idle_timeout
}

async fn wait_for_idle_activity(
    deadline: Option<Instant>,
    app_activity: &mut tokio::sync::watch::Receiver<u64>,
    sdk_activity: &mut tokio::sync::watch::Receiver<u64>,
    queue_changes: Vec<tokio::sync::watch::Receiver<u64>>,
) {
    let has_queue_changes = !queue_changes.is_empty();
    let queue_change = wait_for_any_queue_change(queue_changes);
    tokio::pin!(queue_change);

    match deadline {
        Some(deadline) => {
            tokio::select! {
                _ = tokio::time::sleep_until(deadline.into()) => {}
                _ = app_activity.changed() => {}
                _ = sdk_activity.changed() => {}
                _ = &mut queue_change, if has_queue_changes => {}
            }
        }
        None => {
            tokio::select! {
                _ = app_activity.changed() => {}
                _ = sdk_activity.changed() => {}
                _ = &mut queue_change, if has_queue_changes => {}
            }
        }
    }
}

async fn wait_for_any_queue_change(receivers: Vec<tokio::sync::watch::Receiver<u64>>) {
    let changes = FuturesUnordered::new();
    for mut receiver in receivers {
        changes.push(async move {
            let _ = receiver.changed().await;
        });
    }
    let mut changes = changes;
    let _ = changes.next().await;
}
