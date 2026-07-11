use std::collections::BTreeMap;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::Mutex;

use askama::Template;
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::sse::Event as SseEvent;
use axum::response::sse::KeepAlive;
use axum::response::sse::Sse;
use axum::routing::get;
use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use coco_hub_protocol::AnnounceAckFrame;
use coco_hub_protocol::BatchAckFrame;
use coco_hub_protocol::ErrorFrame;
use coco_hub_protocol::HubFrame;
use coco_hub_protocol::SCHEMA_VERSION_V2;
use coco_hub_protocol::SUBPROTOCOL_V2;
use coco_types::SessionId;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::broadcast;

use crate::local_store::parse_optional_rfc3339;
use crate::store::EventFilter;
use crate::store::EventQuery;
use crate::store::EventRow;
use crate::store::EventStore;
use crate::store::EventStoreError;
use crate::store::HealthSnapshot;
use crate::store::InstanceRow;
use crate::store::ListInstancesParams;
use crate::store::ListSessionsParams;
use crate::store::SearchQuery;
use crate::store::SessionRow;
use crate::store::msg_type;

#[derive(Clone)]
pub struct AppState {
    store: Arc<dyn EventStore>,
    web_static_dir: Arc<std::path::PathBuf>,
    live_topics: Arc<Mutex<HashMap<LiveTopicKey, broadcast::Sender<EventRow>>>>,
}

impl AppState {
    pub fn new(store: impl EventStore + 'static) -> Self {
        Self {
            store: Arc::new(store),
            web_static_dir: Arc::new(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/static"),
            ),
            live_topics: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn subscribe_session(
        &self,
        instance_id: String,
        session_id: SessionId,
    ) -> broadcast::Receiver<EventRow> {
        self.live_topic(instance_id, session_id).subscribe()
    }

    fn publish_event(&self, event: EventRow) {
        let sender = self.live_topic(event.instance_id.clone(), event.session_id.clone());
        let _ = sender.send(event);
    }

    fn live_topic(
        &self,
        instance_id: String,
        session_id: SessionId,
    ) -> broadcast::Sender<EventRow> {
        let mut topics = self
            .live_topics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        topics
            .entry((instance_id, session_id))
            .or_insert_with(|| broadcast::channel(1024).0)
            .clone()
    }
}

type LiveTopicKey = (String, SessionId);

fn live_supported_for(health: &HealthSnapshot) -> bool {
    health.ingest_supported
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index_page))
        .route("/i", get(instances_page))
        .route("/i/{instance_id}", get(instance_page))
        .route(
            "/i/{instance_id}/s/{session_id}",
            get(session_timeline_page),
        )
        .route("/healthz", get(healthz))
        .route("/static/{file}", get(static_asset))
        .route("/p/events", get(events_partial))
        .route("/sse/session/{instance_id}/{session_id}", get(sse_session))
        .route("/v1/connect", get(connect_ws))
        .route("/v1/protocol", get(protocol))
        .route("/v1/instances", get(list_instances))
        .route("/v1/instances/{instance_id}", get(get_instance))
        .route("/v1/instances/{instance_id}/sessions", get(list_sessions))
        .route(
            "/v1/instances/{instance_id}/sessions/{session_id}/events",
            get(list_events),
        )
        .route("/v1/search", get(search))
        .with_state(state)
}

async fn healthz(State(state): State<AppState>) -> Result<Json<HealthSnapshot>, ApiError> {
    let mut health = state.store.health().await?;
    health.live_supported = live_supported_for(&health);
    Ok(Json(health))
}

async fn protocol(State(state): State<AppState>) -> Result<Json<ProtocolResponse>, ApiError> {
    let health = state.store.health().await?;
    Ok(Json(ProtocolResponse {
        mode: state.store.mode(),
        supported_subprotocols: vec![SUBPROTOCOL_V2],
        schema_version: SCHEMA_VERSION_V2,
        read_only: health.read_only,
        ingest_supported: health.ingest_supported,
        live_supported: live_supported_for(&health),
    }))
}

async fn connect_ws(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    let requested_v2 = ws
        .requested_protocols()
        .any(|protocol| protocol.as_bytes() == SUBPROTOCOL_V2.as_bytes());
    if !requested_v2 {
        return (
            StatusCode::BAD_REQUEST,
            format!("missing websocket subprotocol {SUBPROTOCOL_V2}"),
        )
            .into_response();
    }
    ws.protocols([SUBPROTOCOL_V2])
        .max_message_size(10 * 1024 * 1024)
        .on_upgrade(move |socket| async move {
            run_ws_connection(socket, state).await;
        })
}

async fn run_ws_connection(mut socket: WebSocket, state: AppState) {
    let mut announced_instance = None;
    while let Some(message) = socket.recv().await {
        let response = match message {
            Ok(Message::Text(text)) => match serde_json::from_str::<HubFrame>(&text) {
                Ok(frame) => handle_hub_frame(&state, &mut announced_instance, frame).await,
                Err(err) => HubFrame::Error(ErrorFrame {
                    code: "invalid_json".to_string(),
                    detail: err.to_string(),
                }),
            },
            Ok(Message::Binary(_)) => HubFrame::Error(ErrorFrame {
                code: "unsupported_frame".to_string(),
                detail: "binary hub frames are not supported".to_string(),
            }),
            Ok(Message::Close(_)) => return,
            Ok(Message::Ping(_) | Message::Pong(_)) => continue,
            Err(err) => {
                tracing::debug!(error = %err, "event hub websocket receive failed");
                return;
            }
        };
        let Ok(text) = serde_json::to_string(&response) else {
            tracing::error!("failed to serialize event hub websocket response");
            return;
        };
        if socket.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }
}

async fn handle_hub_frame(
    state: &AppState,
    announced_instance: &mut Option<String>,
    frame: HubFrame,
) -> HubFrame {
    match frame {
        HubFrame::Announce(announce) => {
            let instance_id = announce.instance_id.to_string();
            let live_sessions = announce.live_sessions.clone();
            match state.store.upsert_instance(&announce).await {
                Ok(outcome) => {
                    *announced_instance = Some(instance_id.clone());
                    let mut resume_from = HashMap::new();
                    for session_id in live_sessions {
                        let cursor = match state
                            .store
                            .get_session(&instance_id, session_id.as_str())
                            .await
                        {
                            Ok(Some(session)) => session.last_seq,
                            Ok(None) => 0,
                            Err(err) => {
                                return store_error(err);
                            }
                        };
                        resume_from.insert(session_id, cursor);
                    }
                    HubFrame::AnnounceAck(AnnounceAckFrame {
                        first_seen: outcome.first_seen,
                        hub_version: env!("CARGO_PKG_VERSION").to_string(),
                        resume_from,
                    })
                }
                Err(err) => store_error(err),
            }
        }
        HubFrame::Batch(batch) => {
            let Some(instance_id) = announced_instance.as_deref() else {
                return HubFrame::Error(ErrorFrame {
                    code: "announce_required".to_string(),
                    detail: "send announce before batch".to_string(),
                });
            };
            let mut up_to_seq = HashMap::<SessionId, i64>::new();
            let mut publish_keys = Vec::new();
            for event in &batch.events {
                up_to_seq
                    .entry(event.session_id.clone())
                    .and_modify(|seq| *seq = (*seq).max(event.session_seq))
                    .or_insert(event.session_seq);
                match state
                    .store
                    .get_event(instance_id, event.session_id.as_str(), event.session_seq)
                    .await
                {
                    Ok(None) => publish_keys.push((event.session_id.clone(), event.session_seq)),
                    Ok(Some(_)) => {}
                    Err(err) => return store_error(err),
                }
            }
            match state.store.ingest_batch(instance_id, batch).await {
                Ok(stats) => {
                    for (session_id, session_seq) in publish_keys {
                        match state
                            .store
                            .get_event(instance_id, session_id.as_str(), session_seq)
                            .await
                        {
                            Ok(Some(event)) => state.publish_event(event),
                            Ok(None) => {}
                            Err(err) => return store_error(err),
                        }
                    }
                    // The cursor advances past every seq in the batch (including
                    // rejected regressions — retrying them can't help), but the
                    // rejected map lets the connector observe the loss.
                    HubFrame::BatchAck(BatchAckFrame {
                        up_to_seq,
                        rejected: stats.rejected_by_session,
                    })
                }
                Err(err) => store_error(err),
            }
        }
        HubFrame::AnnounceAck(_) | HubFrame::BatchAck(_) | HubFrame::Error(_) => {
            HubFrame::Error(ErrorFrame {
                code: "unexpected_frame".to_string(),
                detail: "hub received a server-to-client frame".to_string(),
            })
        }
    }
}

fn store_error(err: EventStoreError) -> HubFrame {
    HubFrame::Error(ErrorFrame {
        code: "store_error".to_string(),
        detail: err.to_string(),
    })
}

async fn sse_session(
    State(state): State<AppState>,
    Path((instance_id, session_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let session_id = parse_session_id(session_id)?;
    let mut receiver = state.subscribe_session(instance_id, session_id);
    let stream = async_stream::stream! {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let session_seq = event.session_seq;
                    let html = EventListTemplate {
                        events: vec![EventView::from(event)],
                    }
                    .render()
                    .unwrap_or_default();
                    yield Ok::<_, Infallible>(
                        SseEvent::default()
                            .event("event")
                            .id(session_seq.to_string())
                            .data(html),
                    );
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

async fn static_asset(
    State(state): State<AppState>,
    Path(file): Path<String>,
) -> Result<Response, ApiError> {
    if !is_safe_asset_name(&file) {
        return Err(ApiError::not_found("asset not found"));
    }
    let path = state.web_static_dir.join(&file);
    let body = tokio::fs::read(path).await?;
    let content_type = match file.rsplit_once('.').map(|(_, ext)| ext) {
        Some("css") => "text/css",
        Some("js") => "application/javascript",
        _ => "application/octet-stream",
    };
    let response = ([(axum::http::header::CONTENT_TYPE, content_type)], body).into_response();
    Ok(response)
}

async fn list_instances(
    State(state): State<AppState>,
    Query(query): Query<PageParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let page = state.store.list_instances(query.into_instances()).await?;
    Ok(Json(serde_json::json!(page)))
}

async fn get_instance(
    State(state): State<AppState>,
    Path(instance_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let Some(row) = state.store.get_instance(&instance_id).await? else {
        return Err(ApiError::not_found("instance not found"));
    };
    Ok(Json(serde_json::json!(row)))
}

async fn list_sessions(
    State(state): State<AppState>,
    Path(instance_id): Path<String>,
    Query(query): Query<PageParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let page = state
        .store
        .list_sessions(&instance_id, query.into_sessions())
        .await?;
    Ok(Json(serde_json::json!(page)))
}

async fn list_events(
    State(state): State<AppState>,
    Path((instance_id, session_id)): Path<(String, String)>,
    Query(query): Query<EventParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let page = state
        .store
        .list_events(query.into_event_query(instance_id, Some(session_id))?)
        .await?;
    Ok(Json(serde_json::json!(page)))
}

async fn search(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let page = state.store.search(query).await?;
    Ok(Json(serde_json::json!(page)))
}

async fn events_partial(
    State(state): State<AppState>,
    Query(query): Query<PartialEventsParams>,
) -> Result<Html<String>, ApiError> {
    let page = state.store.list_events(query.into_event_query()?).await?;
    let events = page.items.into_iter().map(EventView::from).collect();
    render(EventListTemplate { events })
}

async fn index_page(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    let page = state
        .store
        .list_instances(ListInstancesParams::default())
        .await?;
    let total_sessions = page.items.iter().map(|row| row.session_count).sum();
    render(IndexTemplate {
        title: "Local Event Hub",
        page_kicker: "Session JSONL flight recorder",
        subtitle: "Read-only analysis over local transcripts",
        source: state.store.source_label(),
        total_sessions,
        instances: page.items,
    })
}

async fn instances_page(State(state): State<AppState>) -> Result<Html<String>, ApiError> {
    index_page(State(state)).await
}

async fn instance_page(
    State(state): State<AppState>,
    Path(instance_id): Path<String>,
) -> Result<Html<String>, ApiError> {
    let Some(instance) = state.store.get_instance(&instance_id).await? else {
        return Err(ApiError::not_found("instance not found"));
    };
    let sessions = state
        .store
        .list_sessions(&instance_id, ListSessionsParams::default())
        .await?;
    let total_messages = sessions.items.iter().map(|row| row.message_count).sum();
    let total_tokens = sessions
        .items
        .iter()
        .map(|row| row.total_input_tokens + row.total_output_tokens)
        .sum();
    render(InstanceTemplate {
        title: "Project Sessions",
        instance,
        total_messages,
        total_tokens,
        sessions: sessions.items.into_iter().map(SessionView::from).collect(),
    })
}

async fn session_timeline_page(
    State(state): State<AppState>,
    Path((instance_id, session_id)): Path<(String, String)>,
    Query(query): Query<EventParams>,
) -> Result<Html<String>, ApiError> {
    let Some(session) = state.store.get_session(&instance_id, &session_id).await? else {
        return Err(ApiError::not_found("session not found"));
    };
    let filters = query.to_filter_state();
    let events = state
        .store
        .list_events(query.into_event_query(instance_id.clone(), Some(session_id.clone()))?)
        .await?;
    let session_events = load_session_events(&state, &instance_id, &session_id).await?;
    let tokens = session.total_input_tokens + session.total_output_tokens;
    let time_range = TimeRangeView::from_events(&session_events);
    let session_event_views: Vec<EventView> =
        session_events.into_iter().map(EventView::from).collect();
    let all_event_views: Vec<EventView> = events.items.into_iter().map(EventView::from).collect();
    let audit = AuditSummary::from_events(&all_event_views);
    let file_impacts = ImpactView::top_files(&all_event_views);
    let tool_impacts = ImpactView::top_tools(&all_event_views);
    let tool_options = ImpactView::tool_options(&session_event_views);
    let event_count = all_event_views.len();
    render(SessionTemplate {
        title: "Session Timeline",
        instance_id,
        session,
        event_count,
        tokens,
        audit,
        file_impacts,
        tool_impacts,
        tool_options,
        time_range,
        filters,
        events: all_event_views,
    })
}

async fn load_session_events(
    state: &AppState,
    instance_id: &str,
    session_id: &str,
) -> Result<Vec<EventRow>, ApiError> {
    let mut before = None;
    let mut rows = Vec::new();
    loop {
        let page = state
            .store
            .list_events(EventQuery {
                instance_id: instance_id.to_string(),
                session_id: Some(parse_session_id(session_id.to_string())?),
                before,
                limit: 500,
                filter: EventFilter::default(),
            })
            .await?;
        rows.extend(page.items);
        let Some(next_cursor) = page.next_cursor else {
            return Ok(rows);
        };
        before = Some(next_cursor);
    }
}

fn render(template: impl Template) -> Result<Html<String>, ApiError> {
    Ok(Html(template.render()?))
}

#[derive(Template)]
#[template(path = "index.html")]
struct IndexTemplate {
    title: &'static str,
    page_kicker: &'static str,
    subtitle: &'static str,
    source: String,
    total_sessions: usize,
    instances: Vec<InstanceRow>,
}

#[derive(Template)]
#[template(path = "instance.html")]
struct InstanceTemplate {
    title: &'static str,
    instance: InstanceRow,
    total_messages: i32,
    total_tokens: i64,
    sessions: Vec<SessionView>,
}

#[derive(Template)]
#[template(path = "session.html")]
struct SessionTemplate {
    title: &'static str,
    instance_id: String,
    session: SessionRow,
    event_count: usize,
    tokens: i64,
    audit: AuditSummary,
    file_impacts: Vec<ImpactView>,
    tool_impacts: Vec<ImpactView>,
    tool_options: Vec<ImpactView>,
    time_range: TimeRangeView,
    filters: EventFilterState,
    events: Vec<EventView>,
}

#[derive(Template)]
#[template(path = "partials/events.html")]
struct EventListTemplate {
    events: Vec<EventView>,
}

struct SessionView {
    instance_id: String,
    session_id: SessionId,
    title: String,
    message_count: i32,
    model: String,
    last_event_ts: i64,
    total_tokens: i64,
}

impl From<SessionRow> for SessionView {
    fn from(session: SessionRow) -> Self {
        let title = session
            .title
            .filter(|title| !title.is_empty())
            .unwrap_or(session.first_prompt);
        Self {
            instance_id: session.instance_id,
            session_id: session.session_id,
            title,
            message_count: session.message_count,
            model: session.model.unwrap_or_default(),
            last_event_ts: session.last_event_ts,
            total_tokens: session.total_input_tokens + session.total_output_tokens,
        }
    }
}

struct EventView {
    session_seq: i64,
    title: String,
    ts: String,
    msg_type: String,
    preview: String,
    display_text: String,
    display_mode: String,
    display_language: String,
    search_text: String,
    json: String,
    kind_class: String,
    tool_name: String,
    call_id: String,
    role: String,
    action: String,
    lane: String,
    files: Vec<String>,
    searchable: String,
    default_open: bool,
}

impl From<EventRow> for EventView {
    fn from(event: EventRow) -> Self {
        let title = match event.inner_kind {
            Some(inner) => format!("{} / {inner}", event.kind),
            None => event.kind,
        };
        let json = serde_json::to_string_pretty(&event.payload).unwrap_or_default();
        let preview = event.preview.unwrap_or_default();
        let display_text = event.display_text.unwrap_or_default();
        let search_text = [
            event.msg_type.as_str(),
            event.lane.as_str(),
            event.tool_name.as_deref().unwrap_or_default(),
            preview.as_str(),
            display_text.as_str(),
        ]
        .join(" ");
        Self {
            session_seq: event.session_seq,
            title,
            ts: event.ts_display,
            msg_type: event.msg_type,
            preview,
            display_text,
            display_mode: event.display_mode,
            display_language: event.display_language,
            search_text,
            json,
            kind_class: event.lane_class,
            tool_name: event.tool_name.unwrap_or_default(),
            call_id: event.call_id.unwrap_or_default(),
            role: event.role,
            action: event.action,
            lane: event.lane,
            files: event.file_refs,
            searchable: event.searchable,
            default_open: event.default_open,
        }
    }
}

struct ImpactView {
    label: String,
    count: usize,
}

impl ImpactView {
    fn top_files(events: &[EventView]) -> Vec<Self> {
        let mut counts = BTreeMap::new();
        for file in events.iter().flat_map(|event| event.files.iter()) {
            *counts.entry(file.clone()).or_insert(0) += 1;
        }
        Self::top_counts(counts, 8)
    }

    fn top_tools(events: &[EventView]) -> Vec<Self> {
        let mut counts = BTreeMap::new();
        for tool_name in events
            .iter()
            .filter(|event| event.msg_type == msg_type::TOOL_USE)
            .map(|event| event.tool_name.as_str())
            .filter(|tool_name| !tool_name.is_empty())
        {
            *counts.entry(tool_name.to_owned()).or_insert(0) += 1;
        }
        Self::top_counts(counts, 8)
    }

    fn tool_options(events: &[EventView]) -> Vec<Self> {
        let mut counts = BTreeMap::new();
        for tool_name in events
            .iter()
            .map(|event| event.tool_name.as_str())
            .filter(|tool_name| !tool_name.is_empty())
        {
            *counts.entry(tool_name.to_owned()).or_insert(0) += 1;
        }
        Self::top_counts(counts, usize::MAX)
    }

    fn top_counts(counts: BTreeMap<String, usize>, limit: usize) -> Vec<Self> {
        let mut rows: Vec<_> = counts
            .into_iter()
            .map(|(label, count)| Self { label, count })
            .collect();
        rows.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.label.cmp(&right.label))
        });
        rows.truncate(limit);
        rows
    }
}

struct TimeRangeView {
    has_range: bool,
    start_iso: String,
    end_iso: String,
    start_label: String,
    end_label: String,
}

impl TimeRangeView {
    fn from_events(events: &[EventRow]) -> Self {
        let min_ts = events.iter().filter_map(valid_ts).min();
        let max_ts = events.iter().filter_map(valid_ts).max();
        match (min_ts, max_ts) {
            (Some(start), Some(end)) => Self {
                has_range: true,
                start_iso: format_rfc3339_seconds(start),
                end_iso: format_rfc3339_seconds(end),
                start_label: format_minute_label(start),
                end_label: format_minute_label(end),
            },
            _ => Self {
                has_range: false,
                start_iso: String::new(),
                end_iso: String::new(),
                start_label: String::new(),
                end_label: String::new(),
            },
        }
    }
}

fn valid_ts(event: &EventRow) -> Option<i64> {
    (event.ts > 0).then_some(event.ts)
}

fn format_rfc3339_seconds(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ts)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default()
}

fn format_minute_label(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ts)
        .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_default()
}

#[derive(Debug)]
struct AuditSummary {
    user_turns: usize,
    assistant_messages: usize,
    reasoning_blocks: usize,
    tool_requests: usize,
    tool_results: usize,
    reads: usize,
    searches: usize,
    writes: usize,
    shell: usize,
    attention_rows: usize,
    metadata: usize,
}

impl AuditSummary {
    fn from_events(events: &[EventView]) -> Self {
        let writes = events.iter().filter(|event| event.lane == "write").count();
        let shell = events.iter().filter(|event| event.lane == "shell").count();
        Self {
            user_turns: events.iter().filter(|event| event.role == "user").count(),
            assistant_messages: events
                .iter()
                .filter(|event| event.role == "assistant" && event.lane == "message")
                .count(),
            reasoning_blocks: events
                .iter()
                .filter(|event| event.lane == "reasoning")
                .count(),
            tool_requests: events
                .iter()
                .filter(|event| event.msg_type == "tool_use")
                .count(),
            tool_results: events
                .iter()
                .filter(|event| event.lane == "tool-result" || event.msg_type == "tool_result")
                .count(),
            reads: events.iter().filter(|event| event.lane == "read").count(),
            searches: events.iter().filter(|event| event.lane == "search").count(),
            writes,
            shell,
            attention_rows: writes + shell,
            metadata: events
                .iter()
                .filter(|event| event.lane == "metadata")
                .count(),
        }
    }
}

#[derive(Debug, Default)]
struct EventFilterState {
    kind: String,
    msg_type: String,
    tool: String,
    time_from: String,
    time_to: String,
    limit: usize,
}

#[derive(Debug, Deserialize, Clone)]
struct PageParams {
    limit: Option<usize>,
    cursor: Option<String>,
}

impl PageParams {
    fn into_instances(self) -> ListInstancesParams {
        ListInstancesParams {
            limit: self.limit,
            cursor: self.cursor,
        }
    }

    fn into_sessions(self) -> ListSessionsParams {
        ListSessionsParams {
            limit: self.limit,
            cursor: self.cursor,
        }
    }
}

fn parse_session_id(session_id: String) -> Result<SessionId, ApiError> {
    SessionId::try_new(session_id)
        .map_err(|err| ApiError::bad_request(format!("invalid session id: {err}")))
}

fn parse_optional_session_id(session_id: Option<String>) -> Result<Option<SessionId>, ApiError> {
    session_id.map(parse_session_id).transpose()
}

#[derive(Debug, Deserialize, Clone)]
struct EventParams {
    kind: Option<String>,
    msg_type: Option<String>,
    tool: Option<String>,
    time_from: Option<String>,
    time_to: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
    before: Option<String>,
}

impl EventParams {
    fn to_filter_state(&self) -> EventFilterState {
        EventFilterState {
            kind: self.kind.clone().unwrap_or_default(),
            msg_type: self.msg_type.clone().unwrap_or_default(),
            tool: self.tool.clone().unwrap_or_default(),
            time_from: self.time_from.clone().unwrap_or_default(),
            time_to: self.time_to.clone().unwrap_or_default(),
            limit: self.limit.unwrap_or(100).clamp(1, 500),
        }
    }

    fn into_event_query(
        self,
        instance_id: String,
        session_id: Option<String>,
    ) -> Result<EventQuery, ApiError> {
        Ok(EventQuery {
            instance_id,
            session_id: parse_optional_session_id(session_id)?,
            before: self.cursor.or(self.before),
            limit: self.limit.unwrap_or(100).clamp(1, 500),
            filter: EventFilter {
                kind: self.kind,
                msg_type: self.msg_type,
                tool: self.tool,
                from_ms: parse_optional_rfc3339(self.time_from.as_deref())?,
                to_ms: parse_optional_rfc3339(self.time_to.as_deref())?,
                ..EventFilter::default()
            },
        })
    }
}

#[derive(Debug, Deserialize)]
struct PartialEventsParams {
    instance: String,
    session: String,
    kind: Option<String>,
    msg_type: Option<String>,
    tool: Option<String>,
    time_from: Option<String>,
    time_to: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
    before: Option<String>,
}

impl PartialEventsParams {
    fn into_event_query(self) -> Result<EventQuery, ApiError> {
        Ok(EventQuery {
            instance_id: self.instance,
            session_id: Some(parse_session_id(self.session)?),
            before: self.cursor.or(self.before),
            limit: self.limit.unwrap_or(100).clamp(1, 500),
            filter: EventFilter {
                kind: self.kind,
                msg_type: self.msg_type,
                tool: self.tool,
                from_ms: parse_optional_rfc3339(self.time_from.as_deref())?,
                to_ms: parse_optional_rfc3339(self.time_to.as_deref())?,
                ..EventFilter::default()
            },
        })
    }
}

#[derive(Debug, Serialize)]
struct ProtocolResponse {
    mode: &'static str,
    supported_subprotocols: Vec<&'static str>,
    schema_version: u32,
    read_only: bool,
    ingest_supported: bool,
    live_supported: bool,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }
}

impl From<EventStoreError> for ApiError {
    fn from(err: EventStoreError) -> Self {
        let status = match err {
            EventStoreError::FreeTextNotSupported | EventStoreError::InvalidQuery(_) => {
                StatusCode::BAD_REQUEST
            }
            EventStoreError::NotFound(_) => StatusCode::NOT_FOUND,
            EventStoreError::NotSupported(_) => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self {
            status,
            message: err.to_string(),
        }
    }
}

impl From<askama::Error> for ApiError {
    fn from(err: askama::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: err.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = Json(serde_json::json!({ "error": self.message }));
        (self.status, body).into_response()
    }
}

fn is_safe_asset_name(file: &str) -> bool {
    !file.is_empty() && !file.contains('/') && !file.contains('\\') && file != "." && file != ".."
}

#[cfg(test)]
#[path = "routes.test.rs"]
mod tests;
