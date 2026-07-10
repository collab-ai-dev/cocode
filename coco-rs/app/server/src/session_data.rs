use futures::future::BoxFuture;

use crate::AppServer;
use crate::JsonRpcDispatchError;
use coco_types::ClientRequest;
use coco_types::SdkSessionTurnSummary;
use coco_types::SessionId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSessionDataError {
    pub code: i32,
    pub message: String,
}

impl AppSessionDataError {
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: coco_types::error_codes::INVALID_REQUEST,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: coco_types::error_codes::INTERNAL_ERROR,
            message: message.into(),
        }
    }

    pub fn into_dispatch_error(self) -> JsonRpcDispatchError {
        JsonRpcDispatchError {
            code: self.code,
            message: self.message,
            data: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AppSessionDataRequest {
    List,
    Read(coco_types::SessionReadParams),
    TurnsList(coco_types::SessionTurnsListParams),
}

impl AppSessionDataRequest {
    pub fn from_client_request(request: &ClientRequest) -> Option<Self> {
        match request {
            ClientRequest::SessionList => Some(Self::List),
            ClientRequest::SessionRead(params) => Some(Self::Read(params.clone())),
            ClientRequest::SessionTurnsList(params) => Some(Self::TurnsList(params.clone())),
            _ => None,
        }
    }
}

pub struct LiveSessionDataMessage {
    pub value: serde_json::Value,
    pub is_user: bool,
    pub timestamp: Option<String>,
}

pub struct LiveSessionDataSnapshot {
    pub summary: coco_types::SdkSessionSummary,
    pub messages: Vec<LiveSessionDataMessage>,
}

pub trait AppSessionDataHandle {
    fn session_data_snapshot(
        &self,
    ) -> BoxFuture<'_, Result<Option<LiveSessionDataSnapshot>, AppSessionDataError>>;
}

pub trait AppSessionDataSource {
    fn list_persisted_sessions(
        &self,
    ) -> BoxFuture<'_, Result<coco_types::SessionListResult, AppSessionDataError>>;

    fn read_persisted_session(
        &self,
        params: coco_types::SessionReadParams,
    ) -> BoxFuture<'_, Result<coco_types::SessionReadResult, AppSessionDataError>>;

    fn list_persisted_session_turns(
        &self,
        params: coco_types::SessionTurnsListParams,
    ) -> BoxFuture<'_, Result<coco_types::SessionTurnsListResult, AppSessionDataError>>;

    fn live_session_fallback(
        &self,
        _session_id: SessionId,
    ) -> BoxFuture<'_, Result<Option<LiveSessionDataSnapshot>, AppSessionDataError>> {
        Box::pin(async { Ok(None) })
    }
}

impl<H> AppServer<H>
where
    H: AppSessionDataHandle + Clone,
{
    pub async fn handle_session_data_request<S>(
        &self,
        request: &AppSessionDataRequest,
        source: &S,
    ) -> Result<serde_json::Value, JsonRpcDispatchError>
    where
        S: AppSessionDataSource + ?Sized,
    {
        match request {
            AppSessionDataRequest::List => {
                let listed = source
                    .list_persisted_sessions()
                    .await
                    .map_err(AppSessionDataError::into_dispatch_error)?;
                self.merge_session_list(listed, source).await
            }
            AppSessionDataRequest::Read(params) => {
                match source.read_persisted_session(params.clone()).await {
                    Ok(result) => encode_session_data_result("session/read", result),
                    Err(error) if error.code == coco_types::error_codes::INVALID_REQUEST => {
                        match self.read_live_session(params, source).await? {
                            Some(result) => Ok(result),
                            None => Err(error.into_dispatch_error()),
                        }
                    }
                    Err(error) => Err(error.into_dispatch_error()),
                }
            }
            AppSessionDataRequest::TurnsList(params) => {
                match source.list_persisted_session_turns(params.clone()).await {
                    Ok(result) => encode_session_data_result("session/turns/list", result),
                    Err(error) if error.code == coco_types::error_codes::INVALID_REQUEST => {
                        match self.list_live_session_turns(params, source).await? {
                            Some(result) => Ok(result),
                            None => Err(error.into_dispatch_error()),
                        }
                    }
                    Err(error) => Err(error.into_dispatch_error()),
                }
            }
        }
    }

    async fn merge_session_list<S>(
        &self,
        mut listed: coco_types::SessionListResult,
        source: &S,
    ) -> Result<serde_json::Value, JsonRpcDispatchError>
    where
        S: AppSessionDataSource + ?Sized,
    {
        let mut known: std::collections::HashSet<SessionId> = listed
            .sessions
            .iter()
            .map(|session| session.session_id.clone())
            .collect();
        for live in self.list_live_sessions() {
            if known.contains(&live.session_id) {
                continue;
            }
            if let Some(snapshot) = self.live_session_data(&live.session_id, source).await? {
                known.insert(snapshot.summary.session_id.clone());
                listed.sessions.push(snapshot.summary);
            }
        }
        encode_session_data_result("session/list", listed)
    }

    async fn read_live_session<S>(
        &self,
        params: &coco_types::SessionReadParams,
        source: &S,
    ) -> Result<Option<serde_json::Value>, JsonRpcDispatchError>
    where
        S: AppSessionDataSource + ?Sized,
    {
        if self.registry().get(&params.session_id).is_none() {
            return Ok(None);
        }
        let Some(snapshot) = self.live_session_data(&params.session_id, source).await? else {
            return Ok(None);
        };
        let cursor = parse_session_data_cursor("session/read", params.cursor.as_deref())
            .map_err(session_data_projection_dispatch_error)?;
        let limit = parse_session_data_limit("session/read", params.limit)
            .map_err(session_data_projection_dispatch_error)?;
        let page = session_data_page(snapshot.messages.len(), cursor, limit);
        let messages = snapshot.messages[page.start..page.end]
            .iter()
            .map(|message| message.value.clone())
            .collect::<Vec<_>>();
        let result = coco_types::SessionReadResult {
            session: snapshot.summary,
            messages,
            next_cursor: page.next_cursor(),
            has_more: page.has_more,
        };
        encode_session_data_result("session/read", result).map(Some)
    }

    async fn list_live_session_turns<S>(
        &self,
        params: &coco_types::SessionTurnsListParams,
        source: &S,
    ) -> Result<Option<serde_json::Value>, JsonRpcDispatchError>
    where
        S: AppSessionDataSource + ?Sized,
    {
        if self.registry().get(&params.session_id).is_none() {
            return Ok(None);
        }
        let Some(snapshot) = self.live_session_data(&params.session_id, source).await? else {
            return Ok(None);
        };
        let cursor = parse_session_data_cursor("session/turns/list", params.cursor.as_deref())
            .map_err(session_data_projection_dispatch_error)?;
        let limit = parse_session_data_limit("session/turns/list", params.limit)
            .map_err(session_data_projection_dispatch_error)?;
        let turns = derive_session_turn_summaries(snapshot.messages.iter().map(|message| {
            TranscriptTurnEntry {
                is_user: message.is_user,
                timestamp: message.timestamp.as_deref(),
            }
        }));
        let (turns, next_cursor, has_more) = page_session_items(&turns, cursor, limit);
        let result = coco_types::SessionTurnsListResult {
            session: snapshot.summary,
            turns,
            next_cursor,
            has_more,
        };
        encode_session_data_result("session/turns/list", result).map(Some)
    }

    async fn live_session_data<S>(
        &self,
        session_id: &SessionId,
        source: &S,
    ) -> Result<Option<LiveSessionDataSnapshot>, JsonRpcDispatchError>
    where
        S: AppSessionDataSource + ?Sized,
    {
        if let Some(handle) = self.registry().get(session_id)
            && let Some(snapshot) = handle
                .session_data_snapshot()
                .await
                .map_err(AppSessionDataError::into_dispatch_error)?
        {
            return Ok(Some(snapshot));
        }

        source
            .live_session_fallback(session_id.clone())
            .await
            .map_err(AppSessionDataError::into_dispatch_error)
    }
}

fn encode_session_data_result(
    operation: &str,
    result: impl serde::Serialize,
) -> Result<serde_json::Value, JsonRpcDispatchError> {
    serde_json::to_value(result).map_err(|error| JsonRpcDispatchError {
        code: coco_types::error_codes::INTERNAL_ERROR,
        message: format!("AppServer {operation} encode failed: {error}"),
        data: None,
    })
}

fn session_data_projection_dispatch_error(
    error: SessionDataProjectionError,
) -> JsonRpcDispatchError {
    JsonRpcDispatchError {
        code: coco_types::error_codes::INVALID_REQUEST,
        message: error.message(),
        data: None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionDataProjectionError {
    InvalidCursor { operation: String, raw: String },
    InvalidLimit { operation: String, limit: i32 },
}

impl SessionDataProjectionError {
    pub fn message(&self) -> String {
        match self {
            Self::InvalidCursor { operation, raw } => {
                format!("{operation}: invalid cursor {raw:?}")
            }
            Self::InvalidLimit { operation, limit } => {
                format!("{operation}: invalid limit {limit}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionPage {
    pub start: usize,
    pub end: usize,
    pub has_more: bool,
}

impl SessionPage {
    pub fn next_cursor(self) -> Option<String> {
        self.has_more.then(|| self.end.to_string())
    }
}

pub fn parse_session_data_cursor(
    operation: &str,
    raw: Option<&str>,
) -> Result<usize, SessionDataProjectionError> {
    match raw {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| SessionDataProjectionError::InvalidCursor {
                operation: operation.to_string(),
                raw: raw.to_string(),
            }),
        None => Ok(0),
    }
}

pub fn parse_session_data_limit(
    operation: &str,
    limit: Option<i32>,
) -> Result<Option<usize>, SessionDataProjectionError> {
    match limit {
        Some(limit) if limit < 0 => Err(SessionDataProjectionError::InvalidLimit {
            operation: operation.to_string(),
            limit,
        }),
        Some(limit) => Ok(Some(limit as usize)),
        None => Ok(None),
    }
}

pub fn session_data_page(total: usize, cursor: usize, limit: Option<usize>) -> SessionPage {
    let start = cursor.min(total);
    let end = match limit {
        Some(limit) => start.saturating_add(limit).min(total),
        None => total,
    };
    SessionPage {
        start,
        end,
        has_more: end < total,
    }
}

pub fn page_session_items<T: Clone>(
    items: &[T],
    cursor: usize,
    limit: Option<usize>,
) -> (Vec<T>, Option<String>, bool) {
    let page = session_data_page(items.len(), cursor, limit);
    (
        items[page.start..page.end].to_vec(),
        page.next_cursor(),
        page.has_more,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptTurnEntry<'a> {
    pub is_user: bool,
    pub timestamp: Option<&'a str>,
}

pub fn derive_session_turn_summaries<'a>(
    entries: impl IntoIterator<Item = TranscriptTurnEntry<'a>>,
) -> Vec<SdkSessionTurnSummary> {
    let mut spans = Vec::new();
    let mut current: Option<TurnSpanBuilder> = None;
    for (message_index, entry) in entries.into_iter().enumerate() {
        if entry.is_user || current.is_none() {
            if let Some(span) = current.take() {
                spans.push(span.finish(spans.len()));
            }
            current = Some(TurnSpanBuilder::new(message_index, entry.timestamp));
        } else if let Some(span) = current.as_mut() {
            span.message_count += 1;
            span.ended_at = entry.timestamp.and_then(non_empty_timestamp);
        }
    }
    if let Some(span) = current {
        spans.push(span.finish(spans.len()));
    }
    spans
}

#[derive(Debug, Clone)]
struct TurnSpanBuilder {
    start_message_index: usize,
    message_count: i32,
    started_at: Option<String>,
    ended_at: Option<String>,
}

impl TurnSpanBuilder {
    fn new(start_message_index: usize, timestamp: Option<&str>) -> Self {
        let timestamp = timestamp.and_then(non_empty_timestamp);
        Self {
            start_message_index,
            message_count: 1,
            started_at: timestamp.clone(),
            ended_at: timestamp,
        }
    }

    fn finish(self, index: usize) -> SdkSessionTurnSummary {
        SdkSessionTurnSummary {
            index: index as i32,
            start_cursor: self.start_message_index.to_string(),
            message_count: self.message_count,
            started_at: self.started_at,
            ended_at: self.ended_at,
        }
    }
}

fn non_empty_timestamp(timestamp: &str) -> Option<String> {
    (!timestamp.is_empty()).then(|| timestamp.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_items_with_numeric_cursor_and_limit() {
        let page = session_data_page(5, 2, Some(2));
        assert_eq!(page.start, 2);
        assert_eq!(page.end, 4);
        assert_eq!(page.next_cursor().as_deref(), Some("4"));
        assert!(page.has_more);

        let (items, next_cursor, has_more) = page_session_items(&[1, 2, 3, 4, 5], 4, Some(4));
        assert_eq!(items, vec![5]);
        assert_eq!(next_cursor, None);
        assert!(!has_more);
    }

    #[test]
    fn rejects_invalid_cursor_and_limit() {
        let cursor =
            parse_session_data_cursor("session/read", Some("bad")).expect_err("invalid cursor");
        assert_eq!(cursor.message(), "session/read: invalid cursor \"bad\"");

        let limit = parse_session_data_limit("session/read", Some(-1)).expect_err("invalid limit");
        assert_eq!(limit.message(), "session/read: invalid limit -1");
    }

    #[test]
    fn derives_turn_spans_from_user_boundaries() {
        let turns = derive_session_turn_summaries([
            TranscriptTurnEntry {
                is_user: true,
                timestamp: Some("2026-01-01T00:00:00Z"),
            },
            TranscriptTurnEntry {
                is_user: false,
                timestamp: Some("2026-01-01T00:00:01Z"),
            },
            TranscriptTurnEntry {
                is_user: true,
                timestamp: Some("2026-01-01T00:00:02Z"),
            },
        ]);

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].index, 0);
        assert_eq!(turns[0].start_cursor, "0");
        assert_eq!(turns[0].message_count, 2);
        assert_eq!(turns[0].ended_at.as_deref(), Some("2026-01-01T00:00:01Z"));
        assert_eq!(turns[1].index, 1);
        assert_eq!(turns[1].start_cursor, "2");
        assert_eq!(turns[1].message_count, 1);
    }
}
