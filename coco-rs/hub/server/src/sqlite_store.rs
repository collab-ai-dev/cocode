use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use coco_hub_protocol::AnnounceFrame;
use coco_hub_protocol::BatchFrame;
use coco_hub_protocol::EventEnvelope;
use coco_hub_protocol::EventPayload;
use coco_types::SessionId;
use coco_types::TurnId;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;
use tokio::task;

use crate::store::AgentEdge;
use crate::store::EventQuery;
use crate::store::EventRow;
use crate::store::EventStore;
use crate::store::EventStoreError;
use crate::store::GoneReason;
use crate::store::HealthSnapshot;
use crate::store::IngestStats;
use crate::store::InstanceRow;
use crate::store::ListInstancesParams;
use crate::store::ListSessionsParams;
use crate::store::Page;
use crate::store::RetentionPolicy;
use crate::store::SearchHit;
use crate::store::SearchQuery;
use crate::store::SessionRow;
use crate::store::SweepStats;
use crate::store::UpsertInstanceOutcome;
use crate::store::event_matches_filter;
use crate::store::lane;
use crate::store::msg_type;

const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;

#[derive(Clone)]
pub struct SqliteEventStore {
    conn: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl SqliteEventStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, EventStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        initialize_connection(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: path.to_path_buf(),
        })
    }

    pub fn open_in_memory() -> Result<Self, EventStoreError> {
        let conn = Connection::open_in_memory()?;
        initialize_connection(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path: PathBuf::from(":memory:"),
        })
    }

    fn with_conn<T>(
        &self,
        op: impl FnOnce(&Connection) -> Result<T, EventStoreError> + Send + 'static,
    ) -> impl std::future::Future<Output = Result<T, EventStoreError>>
    where
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        async move {
            task::spawn_blocking(move || {
                let guard = conn
                    .lock()
                    .map_err(|_| EventStoreError::InvalidQuery("sqlite mutex poisoned".into()))?;
                op(&guard)
            })
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
        }
    }

    fn with_conn_mut<T>(
        &self,
        op: impl FnOnce(&mut Connection) -> Result<T, EventStoreError> + Send + 'static,
    ) -> impl std::future::Future<Output = Result<T, EventStoreError>>
    where
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        async move {
            task::spawn_blocking(move || {
                let mut guard = conn
                    .lock()
                    .map_err(|_| EventStoreError::InvalidQuery("sqlite mutex poisoned".into()))?;
                op(&mut guard)
            })
            .await
            .map_err(|err| EventStoreError::TaskJoin(err.to_string()))?
        }
    }
}

#[async_trait]
impl EventStore for SqliteEventStore {
    fn mode(&self) -> &'static str {
        "sqlite"
    }

    fn source_label(&self) -> String {
        format!("SQLite event store at {}", self.path.display())
    }

    async fn upsert_instance(
        &self,
        announce: &AnnounceFrame,
    ) -> Result<UpsertInstanceOutcome, EventStoreError> {
        let announce = announce.clone();
        self.with_conn_mut(move |conn| upsert_instance_sync(conn, &announce))
            .await
    }

    async fn mark_instance_gone(
        &self,
        instance_id: &str,
        reason: GoneReason,
    ) -> Result<(), EventStoreError> {
        let instance_id = instance_id.to_string();
        self.with_conn_mut(move |conn| mark_instance_gone_sync(conn, &instance_id, reason))
            .await
    }

    async fn ingest_batch(
        &self,
        instance_id: &str,
        batch: BatchFrame,
    ) -> Result<IngestStats, EventStoreError> {
        let instance_id = instance_id.to_string();
        self.with_conn_mut(move |conn| ingest_batch_sync(conn, &instance_id, batch))
            .await
    }

    async fn list_instances(
        &self,
        params: ListInstancesParams,
    ) -> Result<Page<InstanceRow>, EventStoreError> {
        self.with_conn(move |conn| list_instances_sync(conn, params))
            .await
    }

    async fn get_instance(
        &self,
        instance_id: &str,
    ) -> Result<Option<InstanceRow>, EventStoreError> {
        let instance_id = instance_id.to_string();
        self.with_conn(move |conn| get_instance_sync(conn, &instance_id))
            .await
    }

    async fn list_sessions(
        &self,
        instance_id: &str,
        params: ListSessionsParams,
    ) -> Result<Page<SessionRow>, EventStoreError> {
        let instance_id = instance_id.to_string();
        self.with_conn(move |conn| list_sessions_sync(conn, &instance_id, params))
            .await
    }

    async fn get_session(
        &self,
        instance_id: &str,
        session_id: &str,
    ) -> Result<Option<SessionRow>, EventStoreError> {
        let instance_id = instance_id.to_string();
        let session_id = session_id.to_string();
        self.with_conn(move |conn| get_session_sync(conn, &instance_id, &session_id))
            .await
    }

    async fn list_events(&self, query: EventQuery) -> Result<Page<EventRow>, EventStoreError> {
        self.with_conn(move |conn| list_events_sync(conn, query))
            .await
    }

    async fn get_event(
        &self,
        instance_id: &str,
        session_id: &str,
        session_seq: i64,
    ) -> Result<Option<EventRow>, EventStoreError> {
        let instance_id = instance_id.to_string();
        let session_id = session_id.to_string();
        self.with_conn(move |conn| get_event_sync(conn, &instance_id, &session_id, session_seq))
            .await
    }

    async fn search(&self, query: SearchQuery) -> Result<Page<SearchHit>, EventStoreError> {
        self.with_conn(move |conn| search_sync(conn, query)).await
    }

    async fn list_agent_edges(
        &self,
        _instance_id: &str,
        _session_id: &str,
    ) -> Result<Vec<AgentEdge>, EventStoreError> {
        Ok(Vec::new())
    }

    async fn run_retention_sweep(
        &self,
        policy: &RetentionPolicy,
    ) -> Result<SweepStats, EventStoreError> {
        let policy = policy.clone();
        self.with_conn_mut(move |conn| run_retention_sweep_sync(conn, &policy))
            .await
    }

    async fn health(&self) -> Result<HealthSnapshot, EventStoreError> {
        Ok(HealthSnapshot {
            ok: true,
            mode: self.mode(),
            read_only: false,
            ingest_supported: true,
            live_supported: false,
        })
    }
}

fn initialize_connection(conn: &Connection) -> Result<(), EventStoreError> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS instances (
             instance_id TEXT PRIMARY KEY,
             row_json TEXT NOT NULL,
             first_seen_at INTEGER NOT NULL,
             last_seen_at INTEGER NOT NULL,
             status TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_instances_last_seen
             ON instances(last_seen_at DESC);
         CREATE TABLE IF NOT EXISTS sessions (
             instance_id TEXT NOT NULL,
             session_id TEXT NOT NULL,
             row_json TEXT NOT NULL,
             started_at INTEGER NOT NULL,
             last_seq INTEGER NOT NULL,
             last_event_ts INTEGER NOT NULL,
             PRIMARY KEY (instance_id, session_id)
         );
         CREATE INDEX IF NOT EXISTS idx_sessions_last_event
             ON sessions(instance_id, last_event_ts DESC);
         CREATE TABLE IF NOT EXISTS events (
             instance_id TEXT NOT NULL,
             session_id TEXT NOT NULL,
             session_seq INTEGER NOT NULL,
             ts INTEGER NOT NULL,
             received_at INTEGER NOT NULL,
             kind TEXT NOT NULL,
             inner_kind TEXT,
             tool_name TEXT,
             agent_id TEXT,
             is_error INTEGER,
             msg_type TEXT NOT NULL,
             row_json TEXT NOT NULL,
             PRIMARY KEY (instance_id, session_id, session_seq)
         );
         CREATE INDEX IF NOT EXISTS idx_events_session_ts
             ON events(instance_id, session_id, ts);
         CREATE INDEX IF NOT EXISTS idx_events_kind ON events(kind);
         CREATE INDEX IF NOT EXISTS idx_events_tool_name
             ON events(tool_name) WHERE tool_name IS NOT NULL;
         CREATE INDEX IF NOT EXISTS idx_events_errors
             ON events(instance_id, session_id, session_seq) WHERE is_error = 1;
         CREATE INDEX IF NOT EXISTS idx_events_agent
             ON events(instance_id, session_id, agent_id) WHERE agent_id IS NOT NULL;",
    )?;
    Ok(())
}

fn upsert_instance_sync(
    conn: &mut Connection,
    announce: &AnnounceFrame,
) -> Result<UpsertInstanceOutcome, EventStoreError> {
    let instance_id = announce.instance_id.to_string();
    let started_at = announce.started_at.timestamp_millis();
    let now = Utc::now().timestamp_millis();
    let previous_last_seen_at = conn
        .query_row(
            "SELECT last_seen_at FROM instances WHERE instance_id = ?1",
            [&instance_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let first_seen_at = previous_last_seen_at
        .and_then(|_| {
            conn.query_row(
                "SELECT first_seen_at FROM instances WHERE instance_id = ?1",
                [&instance_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .transpose()
        })
        .transpose()?
        .unwrap_or(now);
    let row = InstanceRow {
        instance_id: instance_id.clone(),
        host: announce.host.clone(),
        cwd: announce.cwd.clone(),
        pid: Some(announce.pid),
        started_at,
        version: Some(announce.version.clone()),
        kind: announce.instance_kind.clone(),
        entrypoint: announce.entrypoint.clone(),
        name: announce.name.clone(),
        first_seen_at,
        last_seen_at: now,
        status: "online".to_string(),
        session_count: announce.live_sessions.len(),
        source_kind: "sqlite_ingest".to_string(),
        synthetic_identity: false,
    };
    let row_json = serde_json::to_string(&row)?;
    conn.execute(
        "INSERT INTO instances (instance_id, row_json, first_seen_at, last_seen_at, status)
         VALUES (?1, ?2, ?3, ?4, 'online')
         ON CONFLICT(instance_id) DO UPDATE SET
             row_json = excluded.row_json,
             last_seen_at = excluded.last_seen_at,
             status = 'online'",
        params![instance_id, row_json, first_seen_at, now],
    )?;

    for session_id in &announce.live_sessions {
        ensure_session(
            conn,
            row.instance_id.as_str(),
            session_id,
            started_at,
            started_at,
            Some(announce.cwd.clone()),
            "announce",
        )?;
    }

    Ok(UpsertInstanceOutcome {
        first_seen: previous_last_seen_at.is_none(),
        previous_last_seen_at,
    })
}

fn mark_instance_gone_sync(
    conn: &mut Connection,
    instance_id: &str,
    reason: GoneReason,
) -> Result<(), EventStoreError> {
    let Some(mut row) = get_instance_sync(conn, instance_id)? else {
        return Ok(());
    };
    row.status = match reason {
        GoneReason::GracefulClose => "closed",
        GoneReason::Reset => "reset",
        GoneReason::Timeout => "timeout",
    }
    .to_string();
    row.last_seen_at = Utc::now().timestamp_millis();
    let row_json = serde_json::to_string(&row)?;
    conn.execute(
        "UPDATE instances SET row_json = ?1, last_seen_at = ?2, status = ?3
         WHERE instance_id = ?4",
        params![row_json, row.last_seen_at, row.status, instance_id],
    )?;
    Ok(())
}

fn ingest_batch_sync(
    conn: &mut Connection,
    instance_id: &str,
    batch: BatchFrame,
) -> Result<IngestStats, EventStoreError> {
    let tx = conn.transaction()?;
    let mut stats = IngestStats {
        accepted: 0,
        duplicates: 0,
        parse_failures: 0,
    };
    let received_at = Utc::now().timestamp_millis();
    for event in batch.events {
        if event.instance_id.to_string() != instance_id {
            stats.parse_failures += 1;
            continue;
        }
        let row = event_row_from_envelope(event, received_at)?;
        ensure_session(
            &tx,
            &row.instance_id,
            &row.session_id,
            row.ts,
            row.ts,
            session_cwd_from_event(&row),
            "event",
        )?;
        let row_json = serde_json::to_string(&row)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO events (
                instance_id, session_id, session_seq, ts, received_at, kind,
                inner_kind, tool_name, agent_id, is_error, msg_type, row_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                row.instance_id,
                row.session_id.as_str(),
                row.session_seq,
                row.ts,
                row.received_at,
                row.kind,
                row.inner_kind,
                row.tool_name,
                row.agent_id,
                row.is_error.map(i64::from),
                row.msg_type,
                row_json,
            ],
        )?;
        if inserted == 0 {
            stats.duplicates += 1;
            continue;
        }
        stats.accepted += 1;
        update_session_rollup(&tx, &row)?;
        update_instance_last_seen(&tx, instance_id, received_at)?;
    }
    tx.commit()?;
    Ok(stats)
}

fn list_instances_sync(
    conn: &Connection,
    params: ListInstancesParams,
) -> Result<Page<InstanceRow>, EventStoreError> {
    let mut rows = load_all_instances(conn)?;
    for row in &mut rows {
        row.session_count = session_count(conn, &row.instance_id)?;
    }
    rows.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
    Ok(paginate_offset(
        rows,
        limit_or_default(params.limit),
        params.cursor.as_deref(),
    ))
}

fn get_instance_sync(
    conn: &Connection,
    instance_id: &str,
) -> Result<Option<InstanceRow>, EventStoreError> {
    let row = conn
        .query_row(
            "SELECT row_json FROM instances WHERE instance_id = ?1",
            [instance_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    row.map(|json| serde_json::from_str::<InstanceRow>(&json).map_err(EventStoreError::from))
        .transpose()
        .map(|row| {
            row.map(|mut row| {
                row.session_count = session_count(conn, instance_id).unwrap_or(row.session_count);
                row
            })
        })
}

fn list_sessions_sync(
    conn: &Connection,
    instance_id: &str,
    params: ListSessionsParams,
) -> Result<Page<SessionRow>, EventStoreError> {
    let mut stmt = conn.prepare(
        "SELECT row_json FROM sessions
         WHERE instance_id = ?1
         ORDER BY last_event_ts DESC, session_id ASC",
    )?;
    let mut rows = Vec::new();
    let mut query = stmt.query([instance_id])?;
    while let Some(row) = query.next()? {
        rows.push(serde_json::from_str::<SessionRow>(
            &row.get::<_, String>(0)?,
        )?);
    }
    Ok(paginate_offset(
        rows,
        limit_or_default(params.limit),
        params.cursor.as_deref(),
    ))
}

fn get_session_sync(
    conn: &Connection,
    instance_id: &str,
    session_id: &str,
) -> Result<Option<SessionRow>, EventStoreError> {
    let row = conn
        .query_row(
            "SELECT row_json FROM sessions WHERE instance_id = ?1 AND session_id = ?2",
            params![instance_id, session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    row.map(|json| serde_json::from_str::<SessionRow>(&json).map_err(EventStoreError::from))
        .transpose()
}

fn list_events_sync(
    conn: &Connection,
    query: EventQuery,
) -> Result<Page<EventRow>, EventStoreError> {
    let mut rows = load_events(conn, &query.instance_id, query.session_id.as_ref())?;
    rows.retain(|row| event_matches_filter(row, &query.filter));
    rows.sort_by(|a, b| {
        a.session_seq
            .cmp(&b.session_seq)
            .then_with(|| a.ts.cmp(&b.ts))
    });
    Ok(paginate_offset(
        rows,
        limit_or_default(Some(query.limit)),
        query.before.as_deref(),
    ))
}

fn get_event_sync(
    conn: &Connection,
    instance_id: &str,
    session_id: &str,
    session_seq: i64,
) -> Result<Option<EventRow>, EventStoreError> {
    let row = conn
        .query_row(
            "SELECT row_json FROM events
             WHERE instance_id = ?1 AND session_id = ?2 AND session_seq = ?3",
            params![instance_id, session_id, session_seq],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    row.map(|json| serde_json::from_str::<EventRow>(&json).map_err(EventStoreError::from))
        .transpose()
}

fn search_sync(conn: &Connection, query: SearchQuery) -> Result<Page<SearchHit>, EventStoreError> {
    if query.q.as_deref().is_some_and(|q| !q.is_empty()) {
        return Err(EventStoreError::FreeTextNotSupported);
    }
    let from_ms = crate::local_store::parse_optional_rfc3339(query.from.as_deref())?;
    let to_ms = crate::local_store::parse_optional_rfc3339(query.to.as_deref())?;
    let filter = query.filter(from_ms, to_ms);
    let instance_ids = match query.instance {
        Some(instance) => vec![instance],
        None => load_all_instances(conn)?
            .into_iter()
            .map(|row| row.instance_id)
            .collect(),
    };
    let session_id = query
        .session
        .map(SessionId::try_new)
        .transpose()
        .map_err(|err| EventStoreError::InvalidQuery(format!("invalid session id: {err}")))?;
    let mut rows = Vec::new();
    for instance_id in instance_ids {
        rows.extend(load_events(conn, &instance_id, session_id.as_ref())?);
    }
    rows.retain(|row| event_matches_filter(row, &filter));
    rows.sort_by(|a, b| {
        b.ts.cmp(&a.ts)
            .then_with(|| b.session_seq.cmp(&a.session_seq))
    });
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

fn run_retention_sweep_sync(
    conn: &mut Connection,
    policy: &RetentionPolicy,
) -> Result<SweepStats, EventStoreError> {
    let before_bytes = database_bytes(conn)?;
    let cutoff_ms = Utc::now()
        .timestamp_millis()
        .saturating_sub(policy.retention_days.saturating_mul(86_400_000));
    let mut stats = SweepStats {
        deleted_events: 0,
        deleted_sessions: 0,
        freed_bytes: 0,
    };

    let tx = conn.transaction()?;
    let deleted_by_age = tx.execute(
        "DELETE FROM events WHERE received_at <= ?1",
        params![cutoff_ms],
    )?;
    stats.deleted_events += deleted_by_age;
    let deleted_empty = prune_empty_sessions(&tx)?;
    stats.deleted_sessions += deleted_empty;
    refresh_all_session_rollups(&tx)?;
    tx.commit()?;

    if policy.retention_max_bytes >= 0 {
        while database_bytes(conn)? > policy.retention_max_bytes {
            let Some((instance_id, session_id)) = oldest_session_key(conn)? else {
                break;
            };
            let deleted_events = delete_session_events(conn, &instance_id, &session_id)?;
            let deleted_sessions = prune_empty_sessions(conn)?;
            stats.deleted_events += deleted_events;
            stats.deleted_sessions += deleted_sessions;
            vacuum(conn)?;
            if deleted_events == 0 && deleted_sessions == 0 {
                break;
            }
        }
    }

    refresh_all_session_rollups(conn)?;
    let after_bytes = database_bytes(conn)?;
    stats.freed_bytes = before_bytes.saturating_sub(after_bytes);
    Ok(stats)
}

fn prune_empty_sessions(conn: &Connection) -> Result<usize, EventStoreError> {
    let deleted = conn.execute(
        "DELETE FROM sessions
         WHERE NOT EXISTS (
             SELECT 1 FROM events
             WHERE events.instance_id = sessions.instance_id
               AND events.session_id = sessions.session_id
         )",
        [],
    )?;
    Ok(deleted)
}

fn refresh_all_session_rollups(conn: &Connection) -> Result<(), EventStoreError> {
    let mut stmt = conn.prepare("SELECT instance_id, session_id, row_json FROM sessions")?;
    let mut rows = Vec::new();
    let mut query = stmt.query([])?;
    while let Some(row) = query.next()? {
        rows.push((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ));
    }
    drop(query);
    drop(stmt);

    for (instance_id, session_id, row_json) in rows {
        let Some((message_count, last_seq, last_event_ts, started_at)) =
            session_event_rollup(conn, &instance_id, &session_id)?
        else {
            continue;
        };
        let mut row = serde_json::from_str::<SessionRow>(&row_json)?;
        row.message_count = message_count;
        row.last_seq = last_seq;
        row.last_event_ts = last_event_ts;
        row.started_at = started_at;
        let row_json = serde_json::to_string(&row)?;
        conn.execute(
            "UPDATE sessions SET row_json = ?1, last_seq = ?2, last_event_ts = ?3
             WHERE instance_id = ?4 AND session_id = ?5",
            params![row_json, last_seq, last_event_ts, instance_id, session_id],
        )?;
    }
    Ok(())
}

fn session_event_rollup(
    conn: &Connection,
    instance_id: &str,
    session_id: &str,
) -> Result<Option<(i32, i64, i64, i64)>, EventStoreError> {
    conn.query_row(
        "SELECT COUNT(*), MAX(session_seq), MAX(ts), MIN(ts)
         FROM events
         WHERE instance_id = ?1 AND session_id = ?2",
        params![instance_id, session_id],
        |row| {
            let count = row.get::<_, i64>(0)?;
            if count == 0 {
                return Ok(None);
            }
            let message_count = i32::try_from(count).unwrap_or(i32::MAX);
            Ok(Some((
                message_count,
                row.get::<_, Option<i64>>(1)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(2)?.unwrap_or_default(),
                row.get::<_, Option<i64>>(3)?.unwrap_or_default(),
            )))
        },
    )
    .map_err(EventStoreError::from)
}

fn oldest_session_key(conn: &Connection) -> Result<Option<(String, String)>, EventStoreError> {
    conn.query_row(
        "SELECT instance_id, session_id FROM sessions
         ORDER BY last_event_ts ASC, session_id ASC
         LIMIT 1",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
    )
    .optional()
    .map_err(EventStoreError::from)
}

fn delete_session_events(
    conn: &Connection,
    instance_id: &str,
    session_id: &str,
) -> Result<usize, EventStoreError> {
    let deleted = conn.execute(
        "DELETE FROM events WHERE instance_id = ?1 AND session_id = ?2",
        params![instance_id, session_id],
    )?;
    Ok(deleted)
}

fn database_bytes(conn: &Connection) -> Result<i64, EventStoreError> {
    let page_count = conn.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?;
    let page_size = conn.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?;
    Ok(page_count.saturating_mul(page_size))
}

fn vacuum(conn: &Connection) -> Result<(), EventStoreError> {
    conn.execute_batch("VACUUM")?;
    Ok(())
}

fn ensure_session(
    conn: &Connection,
    instance_id: &str,
    session_id: &SessionId,
    started_at: i64,
    last_event_ts: i64,
    cwd: Option<String>,
    discovered_via: &str,
) -> Result<(), EventStoreError> {
    if get_session_sync(conn, instance_id, session_id.as_str())?.is_some() {
        return Ok(());
    }
    let row = SessionRow {
        instance_id: instance_id.to_string(),
        session_id: session_id.clone(),
        started_at,
        ended_at: None,
        model: None,
        total_input_tokens: 0,
        total_output_tokens: 0,
        total_cost_usd: 0.0,
        last_seq: 0,
        last_event_ts,
        discovered_via: discovered_via.to_string(),
        title: None,
        first_prompt: String::new(),
        message_count: 0,
        cwd,
        file_size: 0,
    };
    let row_json = serde_json::to_string(&row)?;
    conn.execute(
        "INSERT OR IGNORE INTO sessions (
            instance_id, session_id, row_json, started_at, last_seq, last_event_ts
         ) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
        params![
            instance_id,
            session_id.as_str(),
            row_json,
            started_at,
            last_event_ts
        ],
    )?;
    Ok(())
}

fn update_session_rollup(conn: &Connection, event: &EventRow) -> Result<(), EventStoreError> {
    let Some(mut row) = get_session_sync(conn, &event.instance_id, event.session_id.as_str())?
    else {
        return Ok(());
    };
    row.last_seq = row.last_seq.max(event.session_seq);
    row.last_event_ts = row.last_event_ts.max(event.ts);
    row.message_count = row.message_count.saturating_add(1);
    if row.started_at <= 0 {
        row.started_at = event.ts;
    }
    if let Some(model) = protocol_param_string(event, "model") {
        row.model = Some(model);
    }
    if let Some(cwd) = protocol_param_string(event, "cwd") {
        row.cwd = Some(cwd);
    }
    if let Some(method) = event.inner_kind.as_deref()
        && matches!(method, "session/ended" | "session/archived")
    {
        row.ended_at = Some(event.ts);
    }
    let row_json = serde_json::to_string(&row)?;
    conn.execute(
        "UPDATE sessions SET row_json = ?1, last_seq = ?2, last_event_ts = ?3
         WHERE instance_id = ?4 AND session_id = ?5",
        params![
            row_json,
            row.last_seq,
            row.last_event_ts,
            event.instance_id,
            event.session_id.as_str()
        ],
    )?;
    Ok(())
}

fn update_instance_last_seen(
    conn: &Connection,
    instance_id: &str,
    last_seen_at: i64,
) -> Result<(), EventStoreError> {
    let Some(mut row) = get_instance_sync(conn, instance_id)? else {
        return Ok(());
    };
    row.last_seen_at = last_seen_at;
    let row_json = serde_json::to_string(&row)?;
    conn.execute(
        "UPDATE instances SET row_json = ?1, last_seen_at = ?2 WHERE instance_id = ?3",
        params![row_json, last_seen_at, instance_id],
    )?;
    Ok(())
}

fn event_row_from_envelope(
    event: EventEnvelope,
    received_at: i64,
) -> Result<EventRow, EventStoreError> {
    let payload = serde_json::to_value(&event.payload)?;
    let analysis = analyze_payload(&event.payload);
    let payload_size = serde_json::to_vec(&payload)?.len();
    let ts = event.ts.timestamp_millis();
    Ok(EventRow {
        instance_id: event.instance_id.to_string(),
        session_id: event.session_id,
        event_id: event.session_seq.to_string(),
        session_seq: event.session_seq,
        line_index: event.session_seq,
        block_index: None,
        ts,
        ts_display: ts_display(ts),
        received_at,
        schema_version: event.schema_version,
        kind: analysis.kind,
        turn_id: analysis.turn_id,
        agent_id: event.agent_id.map(coco_types::AgentId::into_inner),
        item_id: analysis.item_id,
        tool_name: analysis.tool_name,
        call_id: analysis.call_id,
        is_error: analysis.is_error,
        inner_kind: analysis.inner_kind,
        payload,
        block_payload: None,
        payload_size,
        parse_status: "ok".to_string(),
        preview: analysis.preview,
        display_text: analysis.display_text,
        display_mode: "json".to_string(),
        display_language: "json".to_string(),
        role: analysis.role,
        msg_type: analysis.msg_type,
        lane: analysis.lane,
        lane_class: analysis.lane_class,
        action: analysis.action,
        file_refs: analysis.file_refs,
        searchable: analysis.searchable,
        default_open: analysis.default_open,
    })
}

#[derive(Default)]
struct PayloadAnalysis {
    kind: String,
    inner_kind: Option<String>,
    turn_id: Option<TurnId>,
    item_id: Option<String>,
    tool_name: Option<String>,
    call_id: Option<String>,
    is_error: Option<bool>,
    preview: Option<String>,
    display_text: Option<String>,
    role: String,
    msg_type: String,
    lane: String,
    lane_class: String,
    action: String,
    file_refs: Vec<String>,
    searchable: String,
    default_open: bool,
}

fn analyze_payload(payload: &EventPayload) -> PayloadAnalysis {
    match payload {
        EventPayload::Protocol { value } => analyze_protocol_payload(value),
        EventPayload::ToolUseQueued { value } => analyze_tool_payload("tool_use_queued", value),
        EventPayload::ToolUseStarted { value } => analyze_tool_payload("tool_use_started", value),
        EventPayload::ToolUseCompleted { value } => {
            analyze_tool_result_payload("tool_use_completed", value)
        }
        EventPayload::McpToolCallBegin { value } => {
            analyze_tool_payload("mcp_tool_call_begin", value)
        }
        EventPayload::McpToolCallEnd { value } => {
            analyze_tool_result_payload("mcp_tool_call_end", value)
        }
        EventPayload::TextBlockCompleted { value } => {
            analyze_text_payload("text_block_completed", value)
        }
        EventPayload::ThinkingBlockCompleted { value } => {
            analyze_text_payload("thinking_block_completed", value)
        }
        EventPayload::EventsDropped { count, reason, .. } => PayloadAnalysis {
            kind: "events_dropped".to_string(),
            inner_kind: Some(reason.clone()),
            preview: Some(format!("dropped {count} events")),
            role: "system".to_string(),
            msg_type: "events_dropped".to_string(),
            lane: lane::EVENT.to_string(),
            lane_class: "lane--event".to_string(),
            action: "Events dropped".to_string(),
            searchable: "drop marker".to_string(),
            ..PayloadAnalysis::default()
        },
        EventPayload::Unknown { value } => analyze_unknown_payload(value),
    }
}

fn analyze_protocol_payload(value: &serde_json::Value) -> PayloadAnalysis {
    let method = value
        .get("method")
        .or_else(|| value.get("type"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let params = value.get("params").unwrap_or(value);
    let role = params
        .get("role")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("system");
    PayloadAnalysis {
        kind: "protocol".to_string(),
        inner_kind: method.clone(),
        turn_id: id_string(params, &["turn_id", "turnId"]).map(TurnId::new),
        item_id: id_string(params, &["item_id", "itemId"]),
        tool_name: id_string(params, &["tool_name", "toolName", "name"]),
        call_id: id_string(params, &["call_id", "callId", "tool_use_id", "toolUseId"]),
        is_error: params
            .get("is_error")
            .or_else(|| params.get("isError"))
            .and_then(serde_json::Value::as_bool),
        preview: preview_from_value(params),
        display_text: preview_from_value(params),
        role: role.to_string(),
        msg_type: method
            .clone()
            .unwrap_or_else(|| msg_type::METADATA.to_string()),
        lane: lane::METADATA.to_string(),
        lane_class: "lane--metadata".to_string(),
        action: method.unwrap_or_else(|| "Protocol event".to_string()),
        file_refs: Vec::new(),
        searchable: "protocol method, preview".to_string(),
        default_open: false,
    }
}

fn analyze_tool_payload(kind: &str, value: &serde_json::Value) -> PayloadAnalysis {
    let tool_name = id_string(value, &["tool_name", "toolName", "name"]);
    PayloadAnalysis {
        kind: kind.to_string(),
        inner_kind: tool_name.clone(),
        turn_id: id_string(value, &["turn_id", "turnId"]).map(TurnId::new),
        item_id: id_string(value, &["item_id", "itemId"]),
        tool_name: tool_name.clone(),
        call_id: id_string(value, &["call_id", "callId", "id"]),
        preview: tool_name.as_ref().map(|tool| format!("tool_use: {tool}")),
        display_text: preview_from_value(value),
        role: "assistant".to_string(),
        msg_type: msg_type::TOOL_USE.to_string(),
        lane: lane::TOOL.to_string(),
        lane_class: "lane--tool".to_string(),
        action: tool_name
            .as_ref()
            .map(|tool| format!("Tool request: {tool}"))
            .unwrap_or_else(|| "Tool request".to_string()),
        file_refs: Vec::new(),
        searchable: "tool name, input preview".to_string(),
        default_open: true,
        ..PayloadAnalysis::default()
    }
}

fn analyze_tool_result_payload(kind: &str, value: &serde_json::Value) -> PayloadAnalysis {
    let tool_name = id_string(value, &["tool_name", "toolName", "name"]);
    PayloadAnalysis {
        kind: kind.to_string(),
        inner_kind: tool_name.clone(),
        turn_id: id_string(value, &["turn_id", "turnId"]).map(TurnId::new),
        item_id: id_string(value, &["item_id", "itemId"]),
        tool_name,
        call_id: id_string(value, &["call_id", "callId", "id"]),
        is_error: value
            .get("is_error")
            .or_else(|| value.get("isError"))
            .and_then(serde_json::Value::as_bool),
        preview: preview_from_value(value),
        display_text: preview_from_value(value),
        role: "tool".to_string(),
        msg_type: msg_type::TOOL_RESULT.to_string(),
        lane: lane::TOOL_RESULT.to_string(),
        lane_class: "lane--tool-result".to_string(),
        action: "Tool result".to_string(),
        file_refs: Vec::new(),
        searchable: "tool result preview".to_string(),
        default_open: true,
    }
}

fn analyze_text_payload(kind: &str, value: &serde_json::Value) -> PayloadAnalysis {
    let text = id_string(value, &["text", "full_text", "fullText", "thinking"]);
    let is_thinking = kind == "thinking_block_completed";
    PayloadAnalysis {
        kind: kind.to_string(),
        inner_kind: Some(if is_thinking { "thinking" } else { "text" }.to_string()),
        turn_id: id_string(value, &["turn_id", "turnId"]).map(TurnId::new),
        item_id: id_string(value, &["item_id", "itemId"]),
        preview: text.as_deref().map(truncate_preview),
        display_text: text.as_deref().map(truncate_preview),
        role: "assistant".to_string(),
        msg_type: if is_thinking {
            msg_type::REASONING.to_string()
        } else {
            "assistant".to_string()
        },
        lane: if is_thinking {
            lane::REASONING.to_string()
        } else {
            lane::MESSAGE.to_string()
        },
        lane_class: if is_thinking {
            "lane--reasoning".to_string()
        } else {
            "lane--message".to_string()
        },
        action: if is_thinking {
            "Reasoning block".to_string()
        } else {
            "Assistant message".to_string()
        },
        file_refs: Vec::new(),
        searchable: "text preview".to_string(),
        default_open: false,
        ..PayloadAnalysis::default()
    }
}

fn analyze_unknown_payload(value: &serde_json::Value) -> PayloadAnalysis {
    PayloadAnalysis {
        kind: "unknown".to_string(),
        preview: preview_from_value(value),
        display_text: preview_from_value(value),
        role: "system".to_string(),
        msg_type: "unknown".to_string(),
        lane: lane::EVENT.to_string(),
        lane_class: "lane--event".to_string(),
        action: "Unknown event".to_string(),
        searchable: "unknown event preview".to_string(),
        ..PayloadAnalysis::default()
    }
}

fn load_all_instances(conn: &Connection) -> Result<Vec<InstanceRow>, EventStoreError> {
    let mut stmt = conn.prepare("SELECT row_json FROM instances ORDER BY last_seen_at DESC")?;
    let mut rows = Vec::new();
    let mut query = stmt.query([])?;
    while let Some(row) = query.next()? {
        rows.push(serde_json::from_str::<InstanceRow>(
            &row.get::<_, String>(0)?,
        )?);
    }
    Ok(rows)
}

fn load_events(
    conn: &Connection,
    instance_id: &str,
    session_id: Option<&SessionId>,
) -> Result<Vec<EventRow>, EventStoreError> {
    let mut rows = Vec::new();
    match session_id {
        Some(session_id) => {
            let mut stmt = conn.prepare(
                "SELECT row_json FROM events
                 WHERE instance_id = ?1 AND session_id = ?2
                 ORDER BY session_seq ASC",
            )?;
            let mut query = stmt.query(params![instance_id, session_id.as_str()])?;
            while let Some(row) = query.next()? {
                rows.push(serde_json::from_str::<EventRow>(&row.get::<_, String>(0)?)?);
            }
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT row_json FROM events
                 WHERE instance_id = ?1
                 ORDER BY session_id ASC, session_seq ASC",
            )?;
            let mut query = stmt.query([instance_id])?;
            while let Some(row) = query.next()? {
                rows.push(serde_json::from_str::<EventRow>(&row.get::<_, String>(0)?)?);
            }
        }
    }
    Ok(rows)
}

fn session_count(conn: &Connection, instance_id: &str) -> Result<usize, EventStoreError> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM sessions WHERE instance_id = ?1",
        [instance_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as usize)
}

fn protocol_param_string(event: &EventRow, key: &str) -> Option<String> {
    let EventRow {
        payload: serde_json::Value::Object(payload),
        ..
    } = event
    else {
        return None;
    };
    let value = payload
        .get("value")
        .and_then(|value| value.get("params"))
        .and_then(|params| params.get(key));
    value
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn session_cwd_from_event(event: &EventRow) -> Option<String> {
    protocol_param_string(event, "cwd")
}

fn id_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(|value| match value {
            serde_json::Value::String(value) => Some(value.clone()),
            serde_json::Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
}

fn preview_from_value(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value
        .get("text")
        .or_else(|| value.get("content"))
        .or_else(|| value.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        return Some(truncate_preview(text));
    }
    serde_json::to_string(value)
        .ok()
        .map(|value| truncate_preview(&value))
}

fn truncate_preview(value: &str) -> String {
    let trimmed = value.trim();
    let mut out = trimmed.chars().take(200).collect::<String>();
    if out.len() < trimmed.len() {
        out.push_str("...");
    }
    out
}

fn ts_display(ts: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(ts)
        .map(|dt| dt.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default()
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

#[cfg(test)]
#[path = "sqlite_store.test.rs"]
mod tests;
