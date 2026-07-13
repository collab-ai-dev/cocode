//! Construction helpers for replacement session runtimes.
//!
//! Surfaces decide when to switch and how to present the change. This module
//! owns the runtime-side work that must stay consistent across surfaces:
//! building the replacement runtime, binding shared integrations, and seeding
//! session-owned state.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use coco_app_runtime::ProcessRuntime;
use coco_messages::Message;
use coco_types::CoreEvent;
use coco_types::SessionId;
use tokio::sync::mpsc;

use crate::session_runtime::ClearReplacementSnapshot;
use crate::session_runtime::SessionHandle;
use crate::session_runtime::SessionRuntimeFactory;

pub async fn build_resume_replacement_runtime(
    runtime_factory: SessionRuntimeFactory,
    session_id: SessionId,
    prior_messages: Vec<Message>,
    process_runtime: Arc<ProcessRuntime>,
    cwd: PathBuf,
    event_sink: Option<mpsc::Sender<CoreEvent>>,
) -> Result<SessionHandle> {
    let session = runtime_factory
        .build_with_session_id_and_cwd(session_id.clone(), cwd.clone())
        .await?;
    if let Some(event_sink) = event_sink.as_ref() {
        session
            .install_side_query_event_tx(event_sink.clone())
            .await;
    }
    crate::runtime_resume::hydrate_runtime_for_resume(&session, &session_id, &prior_messages).await;
    crate::session_bootstrap::install_session_integrations(
        session.clone(),
        &cwd,
        process_runtime,
        crate::session_bootstrap::SessionIntegrationOptions {
            event_sink,
            ..Default::default()
        },
    )
    .await?;

    session
        .fire_session_start_hooks(coco_hooks::orchestration::SessionStartSource::Resume)
        .await;
    Ok(session)
}

pub async fn build_clear_replacement_runtime(
    runtime_factory: SessionRuntimeFactory,
    session_id: SessionId,
    snapshot: ClearReplacementSnapshot,
    process_runtime: Arc<ProcessRuntime>,
    cwd: PathBuf,
    event_sink: Option<mpsc::Sender<CoreEvent>>,
) -> Result<SessionHandle> {
    let session = runtime_factory
        .build_with_session_id_and_cwd(session_id, cwd.clone())
        .await?;
    if let Some(event_sink) = event_sink.as_ref() {
        session
            .install_side_query_event_tx(event_sink.clone())
            .await;
    }
    session.apply_clear_replacement_snapshot(snapshot).await;

    crate::session_bootstrap::install_session_integrations(
        session.clone(),
        &cwd,
        process_runtime,
        crate::session_bootstrap::SessionIntegrationOptions {
            event_sink,
            ..Default::default()
        },
    )
    .await?;

    Ok(session)
}
