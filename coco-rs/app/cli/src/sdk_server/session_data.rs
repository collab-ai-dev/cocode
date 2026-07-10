use std::sync::Arc;

use futures::future::BoxFuture;

use coco_app_server::AppServer;
use coco_app_server::AppSessionDataError;
use coco_app_server::AppSessionDataHandle;
use coco_app_server::AppSessionDataRequest;
use coco_app_server::AppSessionDataSource;
use coco_app_server::JsonRpcDispatchError;
use coco_app_server::LiveSessionDataMessage;
use coco_app_server::LiveSessionDataSnapshot;
use coco_types::SessionId;

use super::app_server_bridge::LocalAppSessionHandle;
use super::handlers::SdkServerState;

pub(crate) type PersistedSessionDataError = AppSessionDataError;

pub(super) type LocalSessionDataRequest = AppSessionDataRequest;

pub(super) struct LocalSessionDataView {
    pub(super) app_server: Arc<AppServer<LocalAppSessionHandle>>,
    pub(super) state: Arc<SdkServerState>,
}

impl LocalSessionDataView {
    pub(super) async fn handle(
        &self,
        request: &LocalSessionDataRequest,
    ) -> Result<serde_json::Value, JsonRpcDispatchError> {
        self.app_server
            .handle_session_data_request(request, self)
            .await
    }
}

impl AppSessionDataSource for LocalSessionDataView {
    fn list_persisted_sessions(
        &self,
    ) -> BoxFuture<'_, Result<coco_types::SessionListResult, AppSessionDataError>> {
        Box::pin(async move {
            persisted_session_list(self.state.session_manager_snapshot().await).await
        })
    }

    fn read_persisted_session(
        &self,
        params: coco_types::SessionReadParams,
    ) -> BoxFuture<'_, Result<coco_types::SessionReadResult, AppSessionDataError>> {
        Box::pin(async move {
            persisted_session_read(self.state.session_manager_snapshot().await, &params).await
        })
    }

    fn list_persisted_session_turns(
        &self,
        params: coco_types::SessionTurnsListParams,
    ) -> BoxFuture<'_, Result<coco_types::SessionTurnsListResult, AppSessionDataError>> {
        Box::pin(async move {
            persisted_session_turns_list(self.state.session_manager_snapshot().await, &params).await
        })
    }

    fn live_session_fallback(
        &self,
        session_id: SessionId,
    ) -> BoxFuture<'_, Result<Option<LiveSessionDataSnapshot>, AppSessionDataError>> {
        Box::pin(async move {
            live_sdk_session_summary_and_history(&self.state, &session_id)
                .await
                .map(live_session_data_snapshot)
                .transpose()
        })
    }
}

impl AppSessionDataHandle for LocalAppSessionHandle {
    fn session_data_snapshot(
        &self,
    ) -> BoxFuture<'_, Result<Option<LiveSessionDataSnapshot>, AppSessionDataError>> {
        Box::pin(async move {
            self.live_summary_and_history()
                .await
                .map(live_session_data_snapshot)
                .transpose()
        })
    }
}

fn live_session_data_snapshot(
    (summary, history): (
        coco_types::SdkSessionSummary,
        Vec<std::sync::Arc<coco_messages::Message>>,
    ),
) -> Result<LiveSessionDataSnapshot, AppSessionDataError> {
    let messages = history
        .iter()
        .map(|message| {
            let value = serde_json::to_value(message).map_err(|error| {
                AppSessionDataError::internal(format!(
                    "local AppServer session/read encode failed: {error}"
                ))
            })?;
            Ok(LiveSessionDataMessage {
                value,
                is_user: matches!(message.as_ref(), coco_messages::Message::User(_)),
                timestamp: None,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LiveSessionDataSnapshot { summary, messages })
}

pub(crate) fn session_record_to_summary(
    session: &coco_session::Session,
) -> Result<coco_types::SdkSessionSummary, String> {
    Ok(coco_types::SdkSessionSummary {
        session_id: SessionId::try_new(session.id.clone()).map_err(|error| error.to_string())?,
        model: session.model.clone(),
        cwd: session.working_dir.to_string_lossy().into_owned(),
        created_at: session.created_at.clone(),
        updated_at: session.updated_at.clone(),
        title: session.title.clone(),
        message_count: session.message_count,
        total_tokens: session.total_tokens,
    })
}

pub(crate) async fn persisted_session_list(
    manager: Option<Arc<coco_session::SessionManager>>,
) -> Result<coco_types::SessionListResult, PersistedSessionDataError> {
    let Some(manager) = manager else {
        return Ok(coco_types::SessionListResult::default());
    };
    let list_result = tokio::task::spawn_blocking(move || {
        let sessions = manager
            .list()
            .map_err(|error| format!("session/list failed: {error}"))?;
        sessions
            .iter()
            .map(session_record_to_summary)
            .collect::<Result<Vec<_>, _>>()
            .map(|sessions| coco_types::SessionListResult { sessions })
            .map_err(|error| format!("session/list returned invalid session id: {error}"))
    })
    .await;
    match list_result {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(message)) => Err(internal_persisted_session_data_error(message)),
        Err(join_err) => Err(internal_persisted_session_data_error(format!(
            "session/list task panicked: {join_err}"
        ))),
    }
}

pub(crate) async fn persisted_session_read(
    manager: Option<Arc<coco_session::SessionManager>>,
    params: &coco_types::SessionReadParams,
) -> Result<coco_types::SessionReadResult, PersistedSessionDataError> {
    let cursor = parse_persisted_session_data_cursor("session/read", params.cursor.as_deref())?;
    let limit = parse_persisted_session_data_limit("session/read", params.limit)?;
    let manager = session_manager_or_invalid(manager)?;
    let session_id = params.session_id.as_str().to_string();
    let read_result = tokio::task::spawn_blocking(move || {
        let session = manager
            .load(&session_id)
            .map_err(|error| format!("session/read: {error}"))?;
        let store = manager.store_for(&session.working_dir);
        let transcript_messages = store
            .load_transcript_messages(&session_id)
            .map_err(|error| format!("session/read: {error}"))?;
        let page = coco_app_server::session_data_page(transcript_messages.len(), cursor, limit);
        let messages = transcript_messages[page.start..page.end]
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("session/read: {error}"))?;
        Ok::<_, String>(coco_types::SessionReadResult {
            session: session_record_to_summary(&session)
                .map_err(|error| format!("session/read returned invalid session id: {error}"))?,
            messages,
            next_cursor: page.next_cursor(),
            has_more: page.has_more,
        })
    })
    .await;
    match read_result {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(message)) => Err(invalid_persisted_session_data_error(message)),
        Err(join_err) => Err(internal_persisted_session_data_error(format!(
            "session/read task panicked: {join_err}"
        ))),
    }
}

pub(crate) async fn persisted_session_turns_list(
    manager: Option<Arc<coco_session::SessionManager>>,
    params: &coco_types::SessionTurnsListParams,
) -> Result<coco_types::SessionTurnsListResult, PersistedSessionDataError> {
    let cursor =
        parse_persisted_session_data_cursor("session/turns/list", params.cursor.as_deref())?;
    let limit = parse_persisted_session_data_limit("session/turns/list", params.limit)?;
    let manager = session_manager_or_invalid(manager)?;
    let session_id = params.session_id.as_str().to_string();
    let list_result = tokio::task::spawn_blocking(move || {
        let session = manager
            .load(&session_id)
            .map_err(|error| format!("session/turns/list: {error}"))?;
        let store = manager.store_for(&session.working_dir);
        let transcript_messages = store
            .load_transcript_messages(&session_id)
            .map_err(|error| format!("session/turns/list: {error}"))?;
        let turns = coco_app_server::derive_session_turn_summaries(transcript_messages.iter().map(
            |entry| coco_app_server::TranscriptTurnEntry {
                is_user: entry.entry_type == "user",
                timestamp: Some(entry.timestamp.as_str()),
            },
        ));
        let (turns, next_cursor, has_more) =
            coco_app_server::page_session_items(&turns, cursor, limit);
        Ok::<_, String>(coco_types::SessionTurnsListResult {
            session: session_record_to_summary(&session).map_err(|error| {
                format!("session/turns/list returned invalid session id: {error}")
            })?,
            turns,
            next_cursor,
            has_more,
        })
    })
    .await;
    match list_result {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(message)) => Err(invalid_persisted_session_data_error(message)),
        Err(join_err) => Err(internal_persisted_session_data_error(format!(
            "session/turns/list task panicked: {join_err}"
        ))),
    }
}

pub(super) async fn live_sdk_session_summary_and_history(
    state: &Arc<SdkServerState>,
    session_id: &SessionId,
) -> Option<(
    coco_types::SdkSessionSummary,
    Vec<std::sync::Arc<coco_messages::Message>>,
)> {
    let metadata = state.session_metadata_snapshot(session_id)?;
    let handoff = state.session_handoff_snapshot(session_id)?;
    let history = handoff.history.lock().await.clone();
    let accounting = state.session_accounting_snapshot(session_id);
    let timestamp = chrono::Utc::now().to_rfc3339();
    Some((
        coco_types::SdkSessionSummary {
            session_id: session_id.clone(),
            model: metadata.model,
            cwd: metadata.cwd,
            created_at: timestamp.clone(),
            updated_at: Some(timestamp),
            title: None,
            message_count: history.len() as i32,
            total_tokens: accounting.stats.usage.input_tokens.total
                + accounting.stats.usage.output_tokens.total,
        },
        history,
    ))
}

fn session_manager_or_invalid(
    manager: Option<Arc<coco_session::SessionManager>>,
) -> Result<Arc<coco_session::SessionManager>, PersistedSessionDataError> {
    manager.ok_or_else(|| {
        invalid_persisted_session_data_error("session persistence is not enabled on this server")
    })
}

fn parse_persisted_session_data_cursor(
    operation: &str,
    raw: Option<&str>,
) -> Result<usize, PersistedSessionDataError> {
    coco_app_server::parse_session_data_cursor(operation, raw)
        .map_err(persisted_session_data_projection_error)
}

fn parse_persisted_session_data_limit(
    operation: &str,
    limit: Option<i32>,
) -> Result<Option<usize>, PersistedSessionDataError> {
    coco_app_server::parse_session_data_limit(operation, limit)
        .map_err(persisted_session_data_projection_error)
}

fn persisted_session_data_projection_error(
    error: coco_app_server::SessionDataProjectionError,
) -> PersistedSessionDataError {
    AppSessionDataError::invalid_request(error.message())
}

fn invalid_persisted_session_data_error(message: impl Into<String>) -> PersistedSessionDataError {
    AppSessionDataError::invalid_request(message)
}

fn internal_persisted_session_data_error(message: impl Into<String>) -> PersistedSessionDataError {
    AppSessionDataError::internal(message)
}
