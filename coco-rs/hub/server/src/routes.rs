use std::collections::BTreeMap;
use std::sync::Arc;

use askama::Template;
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use coco_hub_protocol::SCHEMA_VERSION_V1;
use coco_hub_protocol::SUBPROTOCOL_V1;
use serde::Deserialize;
use serde::Serialize;

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
}

impl AppState {
    pub fn new(store: impl EventStore + 'static) -> Self {
        Self {
            store: Arc::new(store),
            web_static_dir: Arc::new(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("web/static"),
            ),
        }
    }
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
    Ok(Json(state.store.health().await?))
}

async fn protocol(State(state): State<AppState>) -> Json<ProtocolResponse> {
    Json(ProtocolResponse {
        mode: state.store.mode(),
        supported_subprotocols: vec![SUBPROTOCOL_V1],
        schema_version: SCHEMA_VERSION_V1,
        read_only: true,
        ingest_supported: false,
        live_supported: false,
    })
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
                session_id: Some(session_id.to_string()),
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
    session_id: String,
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
    seq: i64,
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
            seq: event.seq,
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
            session_id,
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
            session_id: Some(self.session),
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
