use std::sync::Arc;

use coco_app_server::AppServer;
use coco_app_server::JsonRpcDispatchError;
use coco_types::ClientRequest;
use coco_types::SessionId;

use super::app_server_bridge::LocalAppSessionHandle;
use super::handlers::SdkServerState;

#[derive(Debug, Clone)]
pub(super) enum LocalSessionDataRequest {
    List,
    Read(coco_types::SessionReadParams),
    TurnsList(coco_types::SessionTurnsListParams),
}

impl LocalSessionDataRequest {
    pub(super) fn from_client_request(request: &ClientRequest) -> Option<Self> {
        match request {
            ClientRequest::SessionList => Some(Self::List),
            ClientRequest::SessionRead(params) => Some(Self::Read(params.clone())),
            ClientRequest::SessionTurnsList(params) => Some(Self::TurnsList(params.clone())),
            _ => None,
        }
    }
}

pub(super) struct LocalSessionDataView {
    pub(super) app_server: Arc<AppServer<LocalAppSessionHandle>>,
    pub(super) state: Arc<SdkServerState>,
}

impl LocalSessionDataView {
    pub(super) async fn handle(
        &self,
        request: &LocalSessionDataRequest,
    ) -> Result<serde_json::Value, JsonRpcDispatchError> {
        match request {
            LocalSessionDataRequest::List => {
                let listed = self.list_persisted_sessions().await?;
                self.merge_session_list(listed).await
            }
            LocalSessionDataRequest::Read(params) => {
                match self.read_persisted_session(params).await {
                    Ok(result) => encode_local_session_data_result("session/read", result),
                    Err(error) if error.code == coco_types::error_codes::INVALID_REQUEST => {
                        match self.read_live_session(params).await? {
                            Some(result) => Ok(result),
                            None => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
            LocalSessionDataRequest::TurnsList(params) => {
                match self.list_persisted_session_turns(params).await {
                    Ok(result) => encode_local_session_data_result("session/turns/list", result),
                    Err(error) if error.code == coco_types::error_codes::INVALID_REQUEST => {
                        match self.list_live_session_turns(params).await? {
                            Some(result) => Ok(result),
                            None => Err(error),
                        }
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }

    async fn merge_session_list(
        &self,
        mut listed: coco_types::SessionListResult,
    ) -> Result<serde_json::Value, JsonRpcDispatchError> {
        let mut known: std::collections::HashSet<SessionId> = listed
            .sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect();
        for live in self.app_server.list_live_sessions() {
            if known.contains(&live.session_id) {
                continue;
            }
            if let Some(summary) = self.live_session_summary(&live.session_id).await {
                known.insert(summary.session_id.clone());
                listed.sessions.push(summary);
            }
        }
        encode_local_session_data_result("session/list", listed)
    }

    async fn list_persisted_sessions(
        &self,
    ) -> Result<coco_types::SessionListResult, JsonRpcDispatchError> {
        let manager = match self.state.session_manager_snapshot().await {
            Some(manager) => manager,
            None => return Ok(coco_types::SessionListResult::default()),
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
            Ok(Err(message)) => Err(internal_local_session_data_error(message)),
            Err(join_err) => Err(internal_local_session_data_error(format!(
                "session/list task panicked: {join_err}"
            ))),
        }
    }

    async fn read_persisted_session(
        &self,
        params: &coco_types::SessionReadParams,
    ) -> Result<coco_types::SessionReadResult, JsonRpcDispatchError> {
        let cursor = parse_local_session_data_cursor("session/read", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/read", params.limit)?;
        let manager = self.session_manager_or_invalid().await?;
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
                session: session_record_to_summary(&session).map_err(|error| {
                    format!("session/read returned invalid session id: {error}")
                })?,
                messages,
                next_cursor: page.next_cursor(),
                has_more: page.has_more,
            })
        })
        .await;
        match read_result {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(message)) => Err(invalid_local_session_data_error(message)),
            Err(join_err) => Err(internal_local_session_data_error(format!(
                "session/read task panicked: {join_err}"
            ))),
        }
    }

    async fn list_persisted_session_turns(
        &self,
        params: &coco_types::SessionTurnsListParams,
    ) -> Result<coco_types::SessionTurnsListResult, JsonRpcDispatchError> {
        let cursor =
            parse_local_session_data_cursor("session/turns/list", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/turns/list", params.limit)?;
        let manager = self.session_manager_or_invalid().await?;
        let session_id = params.session_id.as_str().to_string();
        let list_result = tokio::task::spawn_blocking(move || {
            let session = manager
                .load(&session_id)
                .map_err(|error| format!("session/turns/list: {error}"))?;
            let store = manager.store_for(&session.working_dir);
            let transcript_messages = store
                .load_transcript_messages(&session_id)
                .map_err(|error| format!("session/turns/list: {error}"))?;
            let turns =
                coco_app_server::derive_session_turn_summaries(transcript_messages.iter().map(
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
            Ok(Err(message)) => Err(invalid_local_session_data_error(message)),
            Err(join_err) => Err(internal_local_session_data_error(format!(
                "session/turns/list task panicked: {join_err}"
            ))),
        }
    }

    async fn session_manager_or_invalid(
        &self,
    ) -> Result<Arc<coco_session::SessionManager>, JsonRpcDispatchError> {
        self.state.session_manager_snapshot().await.ok_or_else(|| {
            invalid_local_session_data_error("session persistence is not enabled on this server")
        })
    }

    async fn read_live_session(
        &self,
        params: &coco_types::SessionReadParams,
    ) -> Result<Option<serde_json::Value>, JsonRpcDispatchError> {
        if self.app_server.registry().get(&params.session_id).is_none() {
            return Ok(None);
        }
        let Some((summary, history)) = self
            .live_session_summary_and_history(&params.session_id)
            .await
        else {
            return Ok(None);
        };
        let cursor = parse_local_session_data_cursor("session/read", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/read", params.limit)?;
        let page = coco_app_server::session_data_page(history.len(), cursor, limit);
        let messages = history[page.start..page.end]
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/read encode failed: {error}"),
                data: None,
            })?;
        let result = coco_types::SessionReadResult {
            session: summary,
            messages,
            next_cursor: page.next_cursor(),
            has_more: page.has_more,
        };
        serde_json::to_value(result)
            .map(Some)
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/read encode failed: {error}"),
                data: None,
            })
    }

    async fn list_live_session_turns(
        &self,
        params: &coco_types::SessionTurnsListParams,
    ) -> Result<Option<serde_json::Value>, JsonRpcDispatchError> {
        if self.app_server.registry().get(&params.session_id).is_none() {
            return Ok(None);
        }
        let Some((summary, history)) = self
            .live_session_summary_and_history(&params.session_id)
            .await
        else {
            return Ok(None);
        };
        let cursor =
            parse_local_session_data_cursor("session/turns/list", params.cursor.as_deref())?;
        let limit = parse_local_session_data_limit("session/turns/list", params.limit)?;
        let turns = coco_app_server::derive_session_turn_summaries(history.iter().map(|message| {
            coco_app_server::TranscriptTurnEntry {
                is_user: matches!(message.as_ref(), coco_messages::Message::User(_)),
                timestamp: None,
            }
        }));
        let (turns, next_cursor, has_more) =
            coco_app_server::page_session_items(&turns, cursor, limit);
        let result = coco_types::SessionTurnsListResult {
            session: summary,
            turns,
            next_cursor,
            has_more,
        };
        serde_json::to_value(result)
            .map(Some)
            .map_err(|error| JsonRpcDispatchError {
                code: coco_types::error_codes::INTERNAL_ERROR,
                message: format!("local AppServer session/turns/list encode failed: {error}"),
                data: None,
            })
    }

    async fn live_session_summary(
        &self,
        session_id: &SessionId,
    ) -> Option<coco_types::SdkSessionSummary> {
        self.live_session_summary_and_history(session_id)
            .await
            .map(|(summary, _)| summary)
    }

    async fn live_session_summary_and_history(
        &self,
        session_id: &SessionId,
    ) -> Option<(
        coco_types::SdkSessionSummary,
        Vec<std::sync::Arc<coco_messages::Message>>,
    )> {
        if let Some(handle) = self.app_server.registry().get(session_id)
            && let Some(result) = handle.live_summary_and_history().await
        {
            return Some(result);
        }

        live_sdk_session_summary_and_history(&self.state, session_id).await
    }
}

fn encode_local_session_data_result(
    operation: &str,
    result: impl serde::Serialize,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    serde_json::to_value(result).map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("local AppServer {operation} encode failed: {error}"),
        data: None,
    })
}

fn invalid_local_session_data_error(message: impl Into<String>) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: message.into(),
        data: None,
    }
}

fn internal_local_session_data_error(message: impl Into<String>) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: message.into(),
        data: None,
    }
}

fn session_record_to_summary(
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

fn parse_local_session_data_cursor(
    operation: &str,
    raw: Option<&str>,
) -> Result<usize, JsonRpcDispatchError> {
    coco_app_server::parse_session_data_cursor(operation, raw)
        .map_err(local_session_data_projection_error)
}

fn parse_local_session_data_limit(
    operation: &str,
    limit: Option<i32>,
) -> Result<Option<usize>, JsonRpcDispatchError> {
    coco_app_server::parse_session_data_limit(operation, limit)
        .map_err(local_session_data_projection_error)
}

fn local_session_data_projection_error(
    error: coco_app_server::SessionDataProjectionError,
) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: error.message(),
        data: None,
    }
}
