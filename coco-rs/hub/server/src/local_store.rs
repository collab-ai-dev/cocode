use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use coco_hub_protocol::SCHEMA_VERSION_V1;
use coco_session::DiskCatalog;
use coco_session::Entry;
use coco_session::MetadataEntry;
use coco_session::SessionCatalog;
use coco_session::TranscriptEntry;
use coco_session::TranscriptMetadata;
use coco_types::ToolName;
use tokio::task;

use crate::display::DisplaySource;
use crate::store::AgentEdge;
use crate::store::EventFilter;
use crate::store::EventQuery;
use crate::store::EventRow;
use crate::store::EventStore;
use crate::store::EventStoreError;
use crate::store::HealthSnapshot;
use crate::store::InstanceRow;
use crate::store::ListInstancesParams;
use crate::store::ListSessionsParams;
use crate::store::Page;
use crate::store::SearchHit;
use crate::store::SearchQuery;
use crate::store::SessionRow;
use crate::store::event_kind;
use crate::store::lane;
use crate::store::msg_type;

const MAX_EVENT_PREVIEW_CHARS: usize = 200;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
const MULTI_EDIT_TOOL: &str = "MultiEdit";

#[derive(Clone)]
pub struct LocalSessionJsonStore {
    memory_base: PathBuf,
    catalog: Arc<dyn SessionCatalog>,
}

impl LocalSessionJsonStore {
    pub fn new(memory_base: PathBuf) -> Self {
        let catalog = Arc::new(DiskCatalog::new(memory_base.clone()));
        Self::with_catalog(memory_base, catalog)
    }

    pub fn with_catalog(memory_base: PathBuf, catalog: Arc<dyn SessionCatalog>) -> Self {
        Self {
            memory_base,
            catalog,
        }
    }

    fn list_instances_from_jsonl(
        &self,
        params: ListInstancesParams,
    ) -> Result<Page<InstanceRow>, EventStoreError> {
        let rows = self.all_instances_from_jsonl()?;
        Ok(paginate_offset(
            rows,
            limit_or_default(params.limit),
            params.cursor.as_deref(),
        ))
    }

    fn all_instances_from_jsonl(&self) -> Result<Vec<InstanceRow>, EventStoreError> {
        let mut groups = std::collections::BTreeMap::<String, Vec<TranscriptMetadata>>::new();
        for meta in self.catalog.list_all()? {
            let instance_id = self.instance_id_for_meta(&meta)?;
            groups.entry(instance_id).or_default().push(meta);
        }

        let mut rows = Vec::new();
        for (instance_id, metas) in groups {
            if let Some(row) = self.instance_for_metadata_group(instance_id, metas)? {
                rows.push(row);
            }
        }
        rows.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
        Ok(rows)
    }

    fn get_instance_from_jsonl(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRow>, EventStoreError> {
        if !is_safe_path_segment(instance_id) {
            return Ok(None);
        }
        Ok(self
            .all_instances_from_jsonl()?
            .into_iter()
            .find(|row| row.instance_id == instance_id))
    }

    fn list_sessions_from_jsonl(
        &self,
        instance_id: &str,
        params: ListSessionsParams,
    ) -> Result<Page<SessionRow>, EventStoreError> {
        let rows = self.all_sessions_from_jsonl(instance_id)?;
        Ok(paginate_offset(
            rows,
            limit_or_default(params.limit),
            params.cursor.as_deref(),
        ))
    }

    fn all_sessions_from_jsonl(
        &self,
        instance_id: &str,
    ) -> Result<Vec<SessionRow>, EventStoreError> {
        if !is_safe_path_segment(instance_id) {
            return Ok(Vec::new());
        }
        let mut rows = Vec::new();
        for meta in self.catalog.list_all()? {
            if self.instance_id_for_meta(&meta)? != instance_id {
                continue;
            }
            rows.push(self.session_row_from_meta(instance_id, &meta)?);
        }
        rows.sort_by(|a, b| b.last_event_ts.cmp(&a.last_event_ts));
        Ok(rows)
    }

    fn get_session_from_jsonl(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, EventStoreError> {
        let Some(meta) = self.session_meta_for_instance(instance_id, session_id)? else {
            return Ok(None);
        };
        Ok(Some(self.session_row_from_meta(instance_id, &meta)?))
    }

    fn list_events_from_jsonl(&self, query: EventQuery) -> Result<Page<EventRow>, EventStoreError> {
        let Some(session_id) = query.session_id.as_deref() else {
            return Ok(Page::new(Vec::new()));
        };
        let Some(meta) = self.session_meta_for_instance(&query.instance_id, session_id)? else {
            return Ok(Page::new(Vec::new()));
        };
        let mut rows = self.event_rows_from_meta(&query.instance_id, &meta)?;
        rows.retain(|row| event_matches_filter(row, &query.filter));
        Ok(paginate_offset(
            rows,
            limit_or_default(Some(query.limit)),
            query.before.as_deref(),
        ))
    }

    fn search_jsonl(&self, query: SearchQuery) -> Result<Page<SearchHit>, EventStoreError> {
        if query.q.as_deref().is_some_and(|q| !q.is_empty()) {
            return Err(EventStoreError::FreeTextNotSupported);
        }
        let from_ms = parse_optional_rfc3339(query.from.as_deref())?;
        let to_ms = parse_optional_rfc3339(query.to.as_deref())?;
        let filter = query.filter(from_ms, to_ms);

        let mut rows = Vec::new();
        let instances = match query.instance.as_deref() {
            Some(instance) => self
                .get_instance_from_jsonl(instance)?
                .map(|row| vec![row])
                .unwrap_or_default(),
            None => self.all_instances_from_jsonl()?,
        };

        for instance in instances {
            let sessions = match query.session.as_deref() {
                Some(session_id) => self
                    .get_session_from_jsonl(&instance.instance_id, session_id)?
                    .map(|row| vec![row])
                    .unwrap_or_default(),
                None => self.all_sessions_from_jsonl(&instance.instance_id)?,
            };
            for session in sessions {
                let Some(meta) =
                    self.session_meta_for_instance(&instance.instance_id, &session.session_id)?
                else {
                    continue;
                };
                let mut session_rows = self.event_rows_from_meta(&instance.instance_id, &meta)?;
                session_rows.retain(|row| event_matches_filter(row, &filter));
                rows.append(&mut session_rows);
            }
        }

        rows.sort_by(|a, b| b.ts.cmp(&a.ts).then_with(|| b.seq.cmp(&a.seq)));
        let hits = rows
            .into_iter()
            .map(|event| SearchHit { event })
            .collect::<Vec<_>>();
        Ok(paginate_offset(
            hits,
            limit_or_default(query.limit),
            query.cursor.as_deref(),
        ))
    }

    fn instance_for_metadata_group(
        &self,
        instance_id: String,
        metas: Vec<TranscriptMetadata>,
    ) -> Result<Option<InstanceRow>, EventStoreError> {
        let sessions = metas
            .iter()
            .map(|meta| self.session_row_from_meta(&instance_id, meta))
            .collect::<Result<Vec<_>, _>>()?;
        if sessions.is_empty() {
            return Ok(None);
        }

        let cwd = sessions
            .iter()
            .find_map(|session| session.cwd.clone())
            .unwrap_or_else(|| instance_id.clone());
        let started_at = sessions
            .iter()
            .map(|session| session.started_at)
            .min()
            .unwrap_or(0);
        let last_seen_at = sessions
            .iter()
            .map(|session| session.last_event_ts)
            .max()
            .unwrap_or(started_at);
        let name = self
            .project_dir(&instance_id)?
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| instance_id.clone());

        Ok(Some(InstanceRow {
            instance_id,
            host: "local".to_string(),
            cwd,
            pid: None,
            started_at,
            version: None,
            kind: "local_transcripts".to_string(),
            entrypoint: None,
            name: Some(name),
            first_seen_at: started_at,
            last_seen_at,
            status: "offline".to_string(),
            session_count: sessions.len(),
            source_kind: "local_session_jsonl".to_string(),
            synthetic_identity: true,
        }))
    }

    fn session_meta_for_instance(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Option<TranscriptMetadata>, EventStoreError> {
        if !is_safe_path_segment(instance_id) || !is_safe_path_segment(session_id) {
            return Ok(None);
        }
        Ok(self.catalog.list_all()?.into_iter().find(|meta| {
            meta.session_id == session_id
                && self
                    .instance_id_for_meta(meta)
                    .is_ok_and(|derived| derived == instance_id)
        }))
    }

    fn session_row_from_meta(
        &self,
        instance_id: &str,
        meta: &TranscriptMetadata,
    ) -> Result<SessionRow, EventStoreError> {
        let entries = self.load_entries(meta)?;
        let stats = transcript_stats_from_entries(&entries);
        Ok(session_row_from_meta(
            instance_id,
            meta,
            stats,
            meta.file_size,
        ))
    }

    fn event_rows_from_meta(
        &self,
        instance_id: &str,
        meta: &TranscriptMetadata,
    ) -> Result<Vec<EventRow>, EventStoreError> {
        let entries = self.load_entries(meta)?;
        Ok(event_rows_from_entries(
            instance_id,
            &meta.session_id,
            &entries,
        ))
    }

    fn load_entries(&self, meta: &TranscriptMetadata) -> Result<Vec<Entry>, EventStoreError> {
        let cwd = meta.cwd.as_deref().unwrap_or_default();
        let store = self.catalog.store_for(Path::new(cwd));
        let mut entries = store.load_entries(&meta.session_id)?;
        // Subagent messages now live in per-agent files (`<sid>/subagents/…`),
        // not interleaved in the main transcript. Append them so the projection
        // still surfaces subagent activity (attributed via each entry's
        // `agent_id`) and folds subagent tokens back into session stats.
        // Appending keeps a continuous `line_index`, so event seqs stay
        // collision-free.
        entries.extend(store.load_agent_transcript_entries(&meta.session_id)?);
        Ok(entries)
    }

    fn instance_id_for_meta(&self, meta: &TranscriptMetadata) -> Result<String, EventStoreError> {
        if let Some(cwd) = meta.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
            return Ok(coco_paths::ProjectSlug::for_path(Path::new(cwd))
                .as_str()
                .to_string());
        }
        if let Some(resolved) = self.catalog.resolve(&meta.session_id, None)?
            && let Some(project_dir) = resolved.transcript_path.parent()
            && let Some(name) = project_dir.file_name().and_then(|name| name.to_str())
        {
            return Ok(name.to_string());
        }
        Ok("unknown".to_string())
    }

    fn project_dir(&self, instance_id: &str) -> Result<Option<PathBuf>, EventStoreError> {
        if !is_safe_path_segment(instance_id) {
            return Ok(None);
        }
        let path = self.memory_base.join("projects").join(instance_id);
        if path.is_dir() {
            Ok(Some(path))
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl EventStore for LocalSessionJsonStore {
    fn mode(&self) -> &'static str {
        "local_session_jsonl"
    }

    fn source_label(&self) -> String {
        self.memory_base.display().to_string()
    }

    async fn list_instances(
        &self,
        params: ListInstancesParams,
    ) -> Result<Page<InstanceRow>, EventStoreError> {
        let this = self.clone();
        task::spawn_blocking(move || this.list_instances_from_jsonl(params))
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
    }

    async fn get_instance(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRow>, EventStoreError> {
        let this = self.clone();
        let instance_id = instance_id.to_string();
        task::spawn_blocking(move || this.get_instance_from_jsonl(&instance_id))
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
    }

    async fn list_sessions(
        &self,
        instance_id: &str,
        params: ListSessionsParams,
    ) -> Result<Page<SessionRow>, EventStoreError> {
        let this = self.clone();
        let instance_id = instance_id.to_string();
        task::spawn_blocking(move || this.list_sessions_from_jsonl(&instance_id, params))
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
    }

    async fn get_session(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, EventStoreError> {
        let this = self.clone();
        let instance_id = instance_id.to_string();
        let session_id = session_id.to_string();
        task::spawn_blocking(move || this.get_session_from_jsonl(&instance_id, &session_id))
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
    }

    async fn list_events(&self, query: EventQuery) -> Result<Page<EventRow>, EventStoreError> {
        let this = self.clone();
        task::spawn_blocking(move || this.list_events_from_jsonl(query))
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
    }

    async fn get_event(
        &self,
        instance_id: &str,
        session_id: &str,
        seq: i64,
    ) -> Result<Option<EventRow>, EventStoreError> {
        let mut before = None;
        loop {
            let page = self
                .list_events(EventQuery {
                    instance_id: instance_id.to_string(),
                    session_id: Some(session_id.to_string()),
                    before,
                    limit: MAX_LIMIT,
                    filter: EventFilter::default(),
                })
                .await?;
            if let Some(event) = page.items.into_iter().find(|event| event.seq == seq) {
                return Ok(Some(event));
            }
            let Some(next_cursor) = page.next_cursor else {
                return Ok(None);
            };
            before = Some(next_cursor);
        }
    }

    async fn search(&self, query: SearchQuery) -> Result<Page<SearchHit>, EventStoreError> {
        let this = self.clone();
        task::spawn_blocking(move || this.search_jsonl(query))
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
    }

    async fn list_agent_edges(
        &self,
        _instance_id: &str,
        _session_id: &str,
    ) -> Result<Vec<AgentEdge>, EventStoreError> {
        Ok(Vec::new())
    }

    async fn health(&self) -> Result<HealthSnapshot, EventStoreError> {
        Ok(HealthSnapshot {
            ok: true,
            mode: self.mode(),
            read_only: true,
            ingest_supported: false,
            live_supported: false,
        })
    }
}

fn session_row_from_meta(
    instance_id: &str,
    meta: &TranscriptMetadata,
    stats: TranscriptStats,
    file_size: u64,
) -> SessionRow {
    SessionRow {
        instance_id: instance_id.to_string(),
        session_id: meta.session_id.clone(),
        started_at: parse_timestamp_ms(&meta.created_at),
        ended_at: None,
        model: stats.model,
        total_input_tokens: stats.total_input_tokens,
        total_output_tokens: stats.total_output_tokens,
        total_cost_usd: stats.total_cost_usd,
        last_seq: stats.last_seq,
        last_event_ts: parse_timestamp_ms(&meta.modified_at),
        discovered_via: "local_transcript".to_string(),
        title: meta.custom_title.clone(),
        first_prompt: meta.first_prompt.clone(),
        message_count: meta.message_count,
        cwd: meta.cwd.clone(),
        file_size,
    }
}

fn event_rows_from_entries(
    instance_id: &str,
    session_id: &str,
    entries: &[Entry],
) -> Vec<EventRow> {
    let mut rows = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let payload = serde_json::to_value(entry).unwrap_or(serde_json::Value::Null);
        let payload_size = serde_json::to_string(&payload)
            .map(|line| line.len())
            .unwrap_or(0);
        rows.extend(event_rows_from_value(
            instance_id,
            session_id,
            index as i64,
            payload,
            payload_size,
        ));
    }
    rows
}

#[cfg(test)]
fn event_rows_from_line(
    instance_id: &str,
    session_id: &str,
    line_index: i64,
    line: &str,
) -> Vec<EventRow> {
    let payload = serde_json::from_str::<serde_json::Value>(line)
        .unwrap_or_else(|_| serde_json::Value::String(line.to_string()));
    event_rows_from_value(instance_id, session_id, line_index, payload, line.len())
}

fn event_rows_from_value(
    instance_id: &str,
    session_id: &str,
    line_index: i64,
    payload: serde_json::Value,
    payload_size: usize,
) -> Vec<EventRow> {
    let transcript = serde_json::from_value::<TranscriptEntry>(payload.clone()).ok();
    let metadata = serde_json::from_value::<MetadataEntry>(payload.clone()).ok();
    let redacted_payload = redact_value(payload.clone());

    let kind = if transcript.is_some() {
        event_kind::TRANSCRIPT
    } else if metadata.is_some() {
        event_kind::METADATA
    } else {
        event_kind::UNKNOWN
    };
    let inner_kind = payload
        .get("type")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let ts_display = transcript
        .as_ref()
        .map(|entry| entry.timestamp.clone())
        .filter(|ts| !ts.is_empty())
        .unwrap_or_default();
    let ts = parse_timestamp_ms(&ts_display);
    let agent_id = transcript
        .as_ref()
        .filter(|entry| entry.is_sidechain)
        .and_then(|entry| entry.agent_id.clone());
    let role = message_value(&payload)
        .and_then(|message| message.get("role"))
        .and_then(serde_json::Value::as_str)
        .or(inner_kind.as_deref())
        .unwrap_or("event")
        .to_string();

    let Some(blocks) = content_blocks(&payload) else {
        let analysis = analyze_row_without_block(kind, &role, &redacted_payload);
        return vec![event_row_from_parts(EventRowParts {
            instance_id,
            session_id,
            line_index,
            block_index: None,
            ts,
            ts_display,
            kind,
            inner_kind,
            agent_id,
            payload: redacted_payload,
            block_payload: None,
            payload_size,
            analysis,
        })];
    };

    let mut rows = Vec::new();
    for (block_index, block) in blocks.iter().enumerate() {
        let redacted_block = redact_value(block.clone());
        let analysis = analyze_content_block(&role, &redacted_block);
        rows.push(event_row_from_parts(EventRowParts {
            instance_id,
            session_id,
            line_index,
            block_index: Some(block_index as i64),
            ts,
            ts_display: ts_display.clone(),
            kind,
            inner_kind: inner_kind.clone(),
            agent_id: agent_id.clone(),
            payload: redacted_payload.clone(),
            block_payload: Some(redacted_block),
            payload_size,
            analysis,
        }));
    }
    if rows.is_empty() {
        let analysis = analyze_row_without_block(kind, &role, &redacted_payload);
        rows.push(event_row_from_parts(EventRowParts {
            instance_id,
            session_id,
            line_index,
            block_index: None,
            ts,
            ts_display,
            kind,
            inner_kind,
            agent_id,
            payload: redacted_payload,
            block_payload: None,
            payload_size,
            analysis,
        }));
    }
    rows
}

struct EventRowParts<'a> {
    instance_id: &'a str,
    session_id: &'a str,
    line_index: i64,
    block_index: Option<i64>,
    ts: i64,
    ts_display: String,
    kind: &'static str,
    inner_kind: Option<String>,
    agent_id: Option<String>,
    payload: serde_json::Value,
    block_payload: Option<serde_json::Value>,
    payload_size: usize,
    analysis: EventAnalysis,
}

fn event_row_from_parts(parts: EventRowParts<'_>) -> EventRow {
    let seq = event_seq(parts.line_index, parts.block_index);
    let display = DisplaySource::from_block(
        &parts.analysis.msg_type,
        parts.analysis.tool_name.as_deref(),
        parts.block_payload.as_ref(),
        parts.analysis.preview.as_deref(),
    );
    let display_mode = display.mode_name().to_string();
    let display_language = display.language;
    let display_text = (!display.text.is_empty()).then_some(display.text);
    EventRow {
        instance_id: parts.instance_id.to_string(),
        session_id: parts.session_id.to_string(),
        event_id: event_id(parts.line_index, parts.block_index),
        seq,
        line_index: parts.line_index,
        block_index: parts.block_index,
        ts: parts.ts,
        ts_display: parts.ts_display,
        received_at: parts.ts,
        schema_version: SCHEMA_VERSION_V1,
        kind: parts.kind.to_string(),
        turn_id: None,
        agent_id: parts.agent_id,
        item_id: None,
        tool_name: parts.analysis.tool_name,
        call_id: parts.analysis.call_id,
        is_error: parts.analysis.is_error,
        inner_kind: parts.inner_kind,
        payload: parts.payload,
        block_payload: parts.block_payload,
        payload_size: parts.payload_size,
        parse_status: if parts.kind == event_kind::UNKNOWN {
            "unknown_kind"
        } else {
            "ok"
        }
        .to_string(),
        preview: parts.analysis.preview,
        display_text,
        display_mode,
        display_language,
        role: parts.analysis.role,
        msg_type: parts.analysis.msg_type,
        lane: parts.analysis.lane,
        lane_class: parts.analysis.lane_class,
        action: parts.analysis.action,
        file_refs: parts.analysis.file_refs,
        searchable: parts.analysis.searchable,
        default_open: parts.analysis.default_open,
    }
}

fn message_value(payload: &serde_json::Value) -> Option<&serde_json::Value> {
    let message = payload.get("message")?;
    message.get("message").or(Some(message))
}

fn content_blocks(payload: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    message_value(payload)?
        .get("content")
        .and_then(serde_json::Value::as_array)
}

#[derive(Debug, Default)]
struct TranscriptStats {
    total_input_tokens: i64,
    total_output_tokens: i64,
    total_cost_usd: f64,
    model: Option<String>,
    last_seq: i64,
}

fn transcript_stats_from_entries(entries: &[Entry]) -> TranscriptStats {
    let mut stats = TranscriptStats::default();
    for (index, entry) in entries.iter().enumerate() {
        stats.last_seq = event_seq(index as i64, None);
        let Entry::Transcript(entry) = entry else {
            continue;
        };
        if stats.model.is_none() {
            stats.model.clone_from(&entry.model);
        }
        if let Some(usage) = &entry.usage {
            stats.total_input_tokens += usage.input_tokens;
            stats.total_output_tokens += usage.output_tokens;
        }
        stats.total_cost_usd += entry.cost_usd.unwrap_or(0.0);
    }
    stats
}

#[derive(Debug, Default)]
struct EventAnalysis {
    role: String,
    msg_type: String,
    lane: String,
    lane_class: String,
    action: String,
    tool_name: Option<String>,
    call_id: Option<String>,
    is_error: Option<bool>,
    preview: Option<String>,
    file_refs: Vec<String>,
    searchable: String,
    default_open: bool,
}

fn analyze_row_without_block(
    kind: &'static str,
    role: &str,
    payload: &serde_json::Value,
) -> EventAnalysis {
    let (lane, action) = if kind == event_kind::METADATA {
        (lane::METADATA, "Session metadata")
    } else {
        (lane_for_role(role), role_action(role))
    };
    EventAnalysis {
        role: if kind == event_kind::METADATA {
            "system".to_string()
        } else {
            role.to_string()
        },
        msg_type: if kind == event_kind::METADATA {
            msg_type::METADATA.to_string()
        } else {
            role.to_string()
        },
        lane: lane.to_string(),
        lane_class: lane_class_for(lane).to_string(),
        action: action.to_string(),
        preview: preview_for_value(payload),
        searchable: "role, preview, timestamp".to_string(),
        ..EventAnalysis::default()
    }
}

fn analyze_content_block(role: &str, block: &serde_json::Value) -> EventAnalysis {
    match block.get("type").and_then(serde_json::Value::as_str) {
        Some("tool_use" | "tool-call") => {
            let tool = block
                .get("name")
                .or_else(|| block.get("toolName"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool");
            let input = block.get("input").unwrap_or(&serde_json::Value::Null);
            let lane = lane_for_tool(tool);
            EventAnalysis {
                role: role.to_string(),
                msg_type: msg_type::TOOL_USE.to_string(),
                lane: lane.to_string(),
                lane_class: lane_class_for(lane).to_string(),
                action: format!("Tool request: {tool}"),
                tool_name: Some(tool.to_string()),
                call_id: block
                    .get("id")
                    .or_else(|| block.get("toolCallId"))
                    .and_then(as_string_value),
                preview: Some(format!("tool_use: {tool}")),
                file_refs: file_refs_from_input(tool, input),
                searchable: searchable_for_lane(lane).to_string(),
                default_open: should_open_tool(tool),
                ..EventAnalysis::default()
            }
        }
        Some("tool_result" | "tool-result") => {
            let call_id = block
                .get("tool_use_id")
                .or_else(|| block.get("toolUseId"))
                .or_else(|| block.get("toolCallId"))
                .and_then(as_string_value);
            let tool_name = block
                .get("tool_name")
                .or_else(|| block.get("toolName"))
                .and_then(as_string_value);
            EventAnalysis {
                role: role.to_string(),
                msg_type: msg_type::TOOL_RESULT.to_string(),
                lane: lane::TOOL_RESULT.to_string(),
                lane_class: lane_class_for(lane::TOOL_RESULT).to_string(),
                action: "Tool result".to_string(),
                tool_name,
                call_id: call_id.clone(),
                is_error: block
                    .get("is_error")
                    .or_else(|| block.get("isError"))
                    .and_then(serde_json::Value::as_bool),
                preview: block
                    .get("content")
                    .and_then(preview_for_content)
                    .or_else(|| block.get("output").and_then(preview_for_tool_output))
                    .or_else(|| call_id.as_ref().map(|id| format!("tool_result: {id}"))),
                searchable: "call id, error flag, result preview".to_string(),
                ..EventAnalysis::default()
            }
        }
        Some("thinking") | Some("reasoning") => EventAnalysis {
            role: role.to_string(),
            msg_type: msg_type::REASONING.to_string(),
            lane: lane::REASONING.to_string(),
            lane_class: lane_class_for(lane::REASONING).to_string(),
            action: "Reasoning block".to_string(),
            preview: block
                .get("thinking")
                .or_else(|| block.get("text"))
                .and_then(serde_json::Value::as_str)
                .map(truncate_preview),
            searchable: "reasoning preview".to_string(),
            ..EventAnalysis::default()
        },
        Some("text") => EventAnalysis {
            role: role.to_string(),
            msg_type: role.to_string(),
            lane: lane_for_role(role).to_string(),
            lane_class: lane_class_for(lane_for_role(role)).to_string(),
            action: role_action(role).to_string(),
            preview: block
                .get("text")
                .and_then(serde_json::Value::as_str)
                .map(truncate_preview),
            searchable: "message preview, role, timestamp".to_string(),
            ..EventAnalysis::default()
        },
        _ => EventAnalysis {
            role: role.to_string(),
            msg_type: role.to_string(),
            lane: lane_for_role(role).to_string(),
            lane_class: lane_class_for(lane_for_role(role)).to_string(),
            action: role_action(role).to_string(),
            searchable: "role, preview, timestamp".to_string(),
            ..EventAnalysis::default()
        },
    }
}

fn preview_for_value(value: &serde_json::Value) -> Option<String> {
    // Wire fields are snake_case (coco_session::MetadataEntry write
    // path). The CustomTitle / LastPrompt metadata variants serialize
    // their payload fields as `custom_title` / `last_prompt`; the
    // store reads the raw JSON here so we have to match those names
    // literally rather than going through the typed enum.
    value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(preview_for_content)
        .or_else(|| value.get("custom_title").and_then(as_string_value))
        .or_else(|| value.get("last_prompt").and_then(as_string_value))
        .map(|text| truncate_preview(&text))
}

fn preview_for_content(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(truncate_preview(text));
    }
    let blocks = value.as_array()?;
    for block in blocks {
        if block.get("type").and_then(|value| value.as_str()) == Some("text")
            && let Some(text) = block.get("text").and_then(|value| value.as_str())
        {
            return Some(truncate_preview(text));
        }
    }
    None
}

fn preview_for_tool_output(value: &serde_json::Value) -> Option<String> {
    value
        .get("value")
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.as_str())
        .map(truncate_preview)
}

fn is_safe_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && !segment.contains('/')
        && !segment.contains('\\')
        && segment != "."
        && segment != ".."
}

pub(crate) fn parse_optional_rfc3339(value: Option<&str>) -> Result<Option<i64>, EventStoreError> {
    value
        .filter(|value| !value.is_empty())
        .map(parse_rfc3339_ms)
        .transpose()
}

fn parse_timestamp_ms(value: &str) -> i64 {
    value
        .parse::<i64>()
        .unwrap_or_else(|_| parse_rfc3339_ms(value).unwrap_or(0))
}

fn parse_rfc3339_ms(value: &str) -> Result<i64, EventStoreError> {
    DateTime::parse_from_rfc3339(value)
        .map(|ts| ts.with_timezone(&Utc).timestamp_millis())
        .map_err(|err| {
            EventStoreError::InvalidQuery(format!("invalid RFC3339 timestamp {value}: {err}"))
        })
}

fn paginate_offset<T>(items: Vec<T>, limit: usize, cursor: Option<&str>) -> Page<T> {
    let total = items.len();
    let offset = decode_offset_cursor(cursor).min(total);
    let mut page_items = items
        .into_iter()
        .skip(offset)
        .take(limit.saturating_add(1))
        .collect::<Vec<_>>();
    let next_cursor = if page_items.len() > limit {
        page_items.truncate(limit);
        Some(format!("offset:{}", offset + limit))
    } else {
        None
    };
    Page {
        items: page_items,
        next_cursor,
        estimated_total: Some(total as i64),
    }
}

fn decode_offset_cursor(cursor: Option<&str>) -> usize {
    cursor
        .filter(|value| !value.is_empty())
        .and_then(|value| value.strip_prefix("offset:").unwrap_or(value).parse().ok())
        .unwrap_or(0)
}

fn limit_or_default(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn event_id(line_index: i64, block_index: Option<i64>) -> String {
    match block_index {
        Some(block_index) => format!("{line_index}:{block_index}"),
        None => line_index.to_string(),
    }
}

fn event_seq(line_index: i64, block_index: Option<i64>) -> i64 {
    let line = line_index.max(0);
    if line > i64::MAX >> 32 {
        return i64::MAX;
    }
    let block = block_index.unwrap_or(0).clamp(0, i64::from(u32::MAX));
    (line << 32) | block
}

fn redact_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(redact_text(&text)),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(redact_value).collect())
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .map(|(key, value)| (key, redact_value(value)))
                .collect(),
        ),
        other => other,
    }
}

fn redact_text(text: &str) -> String {
    coco_secret_redact::redact_secrets(text).into_owned()
}

fn truncate_preview(text: &str) -> String {
    let flat = text.replace('\n', " ");
    let trimmed = flat.trim();
    let mut end = trimmed.len();
    for (count, (index, _)) in trimmed.char_indices().enumerate() {
        if count == MAX_EVENT_PREVIEW_CHARS {
            end = index;
            break;
        }
    }
    if end < trimmed.len() {
        format!("{}...", &trimmed[..end])
    } else {
        trimmed.to_string()
    }
}

fn as_string_value(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn event_matches_filter(event: &EventRow, filter: &EventFilter) -> bool {
    filter
        .kind
        .as_deref()
        .filter(|value| !value.is_empty())
        .is_none_or(|kind| event.kind == kind)
        && filter
            .inner_kind
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|inner_kind| event.inner_kind.as_deref() == Some(inner_kind))
        && filter
            .tool
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|tool| event.tool_name.as_deref() == Some(tool))
        && filter
            .error
            .is_none_or(|error| event.is_error == Some(error))
        && filter
            .agent
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|agent| event.agent_id.as_deref() == Some(agent))
        && filter
            .msg_type
            .as_deref()
            .filter(|value| !value.is_empty())
            .is_none_or(|msg_type| {
                event.msg_type == msg_type || event.lane == msg_type || event.role == msg_type
            })
        && filter.from_ms.is_none_or(|from_ms| event.ts >= from_ms)
        && filter.to_ms.is_none_or(|to_ms| event.ts <= to_ms)
}

fn role_action(role: &str) -> &'static str {
    match role {
        "user" => "User intent",
        "assistant" => "Assistant message",
        "system" => "System context",
        _ => "Transcript event",
    }
}

fn lane_for_role(role: &str) -> &'static str {
    match role {
        "user" => lane::INTENT,
        "assistant" => lane::MESSAGE,
        "system" => lane::METADATA,
        _ => lane::EVENT,
    }
}

fn lane_for_tool(tool: &str) -> &'static str {
    match tool {
        name if name == ToolName::Read.as_str() || matches!(name, "NotebookRead" | "LS") => {
            lane::READ
        }
        name if name == ToolName::Glob.as_str()
            || name == ToolName::Grep.as_str()
            || name == ToolName::WebSearch.as_str()
            || name == ToolName::WebFetch.as_str() =>
        {
            lane::SEARCH
        }
        name if name == ToolName::Edit.as_str()
            || name == MULTI_EDIT_TOOL
            || name == ToolName::Write.as_str()
            || name == ToolName::NotebookEdit.as_str() =>
        {
            lane::WRITE
        }
        name if name == ToolName::Bash.as_str() || name == ToolName::PowerShell.as_str() => {
            lane::SHELL
        }
        name if name == ToolName::Agent.as_str() || name == "Task" => lane::SUBAGENT,
        _ => lane::TOOL,
    }
}

fn lane_class_for(lane: &str) -> &'static str {
    match lane {
        lane::INTENT => "lane--intent",
        lane::MESSAGE => "lane--message",
        lane::REASONING => "lane--reasoning",
        lane::READ => "lane--read",
        lane::SEARCH => "lane--search",
        lane::WRITE => "lane--write",
        lane::SHELL => "lane--shell",
        lane::SUBAGENT => "lane--subagent",
        lane::TOOL_RESULT => "lane--result",
        lane::METADATA => "lane--metadata",
        _ => "lane--event",
    }
}

fn searchable_for_lane(lane: &str) -> &'static str {
    match lane {
        lane::READ => "file path, tool name, call id",
        lane::SEARCH => "pattern/query, path scope, tool name",
        lane::WRITE => "target file, tool input, call id",
        lane::SHELL => "command text, call id",
        lane::SUBAGENT => "agent prompt, agent type, call id",
        _ => "tool name, call id, preview",
    }
}

fn should_open_tool(tool: &str) -> bool {
    tool == ToolName::Edit.as_str()
        || tool == MULTI_EDIT_TOOL
        || tool == ToolName::Write.as_str()
        || tool == ToolName::Bash.as_str()
        || tool == ToolName::PowerShell.as_str()
}

fn file_refs_from_input(tool: &str, input: &serde_json::Value) -> Vec<String> {
    let mut refs = Vec::new();
    for key in [
        "file_path",
        "path",
        "notebook_path",
        "pattern",
        "query",
        "command",
        "description",
    ] {
        if let Some(value) = input.get(key).and_then(serde_json::Value::as_str)
            && !value.is_empty()
        {
            refs.push(format!("{key}: {}", truncate_with_limit(value, 120)));
        }
    }
    if refs.is_empty()
        && (tool == ToolName::Edit.as_str()
            || tool == MULTI_EDIT_TOOL
            || tool == ToolName::Write.as_str())
    {
        refs.push("write target not present in parsed input".to_string());
    }
    refs
}

fn truncate_with_limit(text: &str, max_chars: usize) -> String {
    let flat = text.replace('\n', " ");
    let trimmed = flat.trim();
    let mut end = trimmed.len();
    for (count, (index, _)) in trimmed.char_indices().enumerate() {
        if count == max_chars {
            end = index;
            break;
        }
    }
    if end < trimmed.len() {
        format!("{}...", &trimmed[..end])
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
#[path = "local_store.test.rs"]
mod tests;
