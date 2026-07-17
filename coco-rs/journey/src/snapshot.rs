//! Snapshot assembly — turn the on-disk skill + memory state (and, once wired,
//! the append-only journal) into a [`JourneySnapshot`] the TUI can render.
//!
//! Infallible: any missing directory / corrupt file means that source simply
//! contributes nothing (plus `tracing`) — never an error. Sync, blocking I/O;
//! async callers wrap in `spawn_blocking`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use coco_skills::SkillDefinition;
use coco_skills::SkillSource;
use coco_skills::agent_scope::{IncludeDisabled, scan_agent_skills};
use coco_skills::telemetry::SkillTelemetryStats;
use coco_types::JourneyEvent;
use coco_types::JourneyNodeId;
use coco_types::JourneyRecord;

/// Relaxed memory-scan cap for the journey view — the recall path's 200 cap is
/// far too small for a full timeline. Overflow past this is traced.
const MEMORY_SCAN_CAP: usize = 2000;

/// Per-node journal history cap (newest first); keeps the wire payload bounded.
const HISTORY_CAP: usize = 20;

/// One learning-timeline node (a skill or a memory topic file).
#[derive(Debug, Clone)]
pub struct JourneyNode {
    /// Skill display name / memory frontmatter name (falls back to filename).
    pub title: String,
    /// Detail-panel text; the presentation layer truncates.
    pub description: String,
    /// When it entered the timeline: earliest journal event > provenance
    /// `created_at` > mtime.
    pub first_seen_ms: i64,
    /// Latest activity: `max(latest journal event, telemetry last_*_at, mtime)`.
    pub last_activity_ms: i64,
    /// Kind + kind-specific data. Illegal states unrepresentable: telemetry
    /// exists exactly for skills, lifecycle exactly for agent skills.
    pub body: JourneyNodeBody,
    /// This node's journal history (detail panel), newest first, capped.
    pub history: Vec<JourneyRecord>,
}

/// Kind + kind-specific payload of a [`JourneyNode`].
#[derive(Debug, Clone)]
pub enum JourneyNodeBody {
    AgentSkill {
        /// Canonical `SKILL.md` path.
        path: PathBuf,
        lifecycle: AgentSkillLifecycle,
        /// Reused verbatim from `coco_skills` — no mirror type.
        telemetry: SkillTelemetryStats,
    },
    UserSkill {
        path: PathBuf,
        telemetry: SkillTelemetryStats,
    },
    Memory {
        /// memdir-relative filename.
        filename: String,
    },
}

/// Lifecycle of an agent-created skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSkillLifecycle {
    /// Quarantined (not yet promoted). Carries its own progress toward the
    /// curator's invocation gate so no consumer has to re-derive the policy
    /// — a `Learning` node without a threshold is unrepresentable, which is
    /// what stops a surface from quietly inventing its own.
    Learning {
        /// Invocations so far (successes + failures) — what the gate counts.
        invocations: i64,
        /// `SkillLearnConfig::promote_min_invocations` as resolved by the host.
        required: i64,
    },
    /// Promoted to model-invocability.
    Learned,
    /// `disabled: true`.
    Retired,
}

impl JourneyNode {
    /// Derived addressing for mutations (single construction site keeps id and
    /// body coherent).
    pub fn id(&self) -> JourneyNodeId {
        match &self.body {
            JourneyNodeBody::AgentSkill { path, .. } | JourneyNodeBody::UserSkill { path, .. } => {
                JourneyNodeId::Skill { path: path.clone() }
            }
            JourneyNodeBody::Memory { filename } => JourneyNodeId::Memory {
                filename: filename.clone(),
            },
        }
    }
}

/// Aggregate counts + the busiest calendar day.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JourneyStats {
    pub learning: i32,
    pub learned: i32,
    pub retired: i32,
    pub user_skills: i32,
    pub memories: i32,
    /// Busiest calendar day (label, node count) — computed at day granularity,
    /// independent of the display bucketing.
    pub busiest_day: Option<(String, i32)>,
}

/// The assembled learning timeline.
#[derive(Debug, Clone)]
pub struct JourneySnapshot {
    /// Ascending `last_activity_ms`.
    pub nodes: Vec<JourneyNode>,
    pub stats: JourneyStats,
}

/// Where `build_journey` reads from.
#[derive(Debug, Clone)]
pub struct JourneyPaths {
    /// `<config_home>` — skills/.agent, telemetry, skill journal.
    pub config_home: PathBuf,
    /// Current project memdir; `None` (no git project) => empty memory side.
    pub memdir: Option<PathBuf>,
}

/// Assembly entry point. See module docs for the infallibility contract.
///
/// `user_skills` is the manager's full skill list (`SkillManager::all()`); the
/// assembler selects the non-bundled, non-agent, actually-used subset. Agent
/// skills come from the location-keyed scan instead, so they are excluded here
/// to avoid double-counting.
pub fn build_journey(
    paths: &JourneyPaths,
    user_skills: &[Arc<SkillDefinition>],
    promote_min_invocations: i64,
) -> JourneySnapshot {
    let telemetry = coco_skills::telemetry::load_all(&paths.config_home);
    let skill_events = read_skill_events(&paths.config_home);
    let memory_events = paths
        .memdir
        .as_deref()
        .map(read_memory_events)
        .unwrap_or_default();
    let mut nodes: Vec<JourneyNode> = Vec::new();

    // 1. Agent skills (including retired) via the shared scan.
    for scan in scan_agent_skills(&paths.config_home, IncludeDisabled::Yes) {
        let stats = telemetry.get(&scan.skill.name).cloned().unwrap_or_default();
        let lifecycle = if scan.disabled {
            AgentSkillLifecycle::Retired
        } else if scan.promoted {
            AgentSkillLifecycle::Learned
        } else {
            AgentSkillLifecycle::Learning {
                // The curator gates on `total_invocations()`, so progress is
                // successes + failures — a success-only count under-reports it.
                invocations: stats.total_invocations(),
                required: promote_min_invocations,
            }
        };
        let mtime = coco_memory::scan::file_mtime_ms(&scan.skill_md).unwrap_or(0);
        let created = scan
            .skill
            .provenance
            .created_at
            .map(|dt| dt.timestamp_millis())
            .filter(|&c| c > 0);
        let (j_first, j_last, history) = journal_derived(skill_events.get(&scan.skill.name));
        let telemetry_activity = mtime
            .max(stats.last_used_at_ms)
            .max(stats.last_patched_at_ms);
        nodes.push(JourneyNode {
            title: scan.skill.user_facing_name().to_string(),
            description: scan.skill.description.clone(),
            first_seen_ms: j_first.or(created).unwrap_or(mtime),
            last_activity_ms: telemetry_activity.max(j_last.unwrap_or(i64::MIN)),
            body: JourneyNodeBody::AgentSkill {
                path: scan.skill_md,
                lifecycle,
                telemetry: stats,
            },
            history,
        });
    }

    // 2. User/project skills: non-bundled, non-agent, actually used.
    for skill in user_skills {
        if matches!(skill.source, SkillSource::Bundled) || skill.is_agent_created() {
            continue;
        }
        let stats = telemetry.get(&skill.name).cloned().unwrap_or_default();
        if stats.total_invocations() <= 0 {
            continue;
        }
        let Some(path) = skill_source_path(&skill.source) else {
            continue;
        };
        let mtime = coco_memory::scan::file_mtime_ms(&path).unwrap_or(0);
        let (j_first, j_last, history) = journal_derived(skill_events.get(&skill.name));
        let telemetry_activity = mtime
            .max(stats.last_used_at_ms)
            .max(stats.last_patched_at_ms);
        nodes.push(JourneyNode {
            title: skill.user_facing_name().to_string(),
            description: skill.description.clone(),
            first_seen_ms: j_first.unwrap_or(mtime),
            last_activity_ms: telemetry_activity.max(j_last.unwrap_or(i64::MIN)),
            body: JourneyNodeBody::UserSkill {
                path,
                telemetry: stats,
            },
            history,
        });
    }

    // 3. Memories: one node per topic file.
    if let Some(memdir) = &paths.memdir {
        let mems = coco_memory::scan::scan_memory_files(memdir, MEMORY_SCAN_CAP);
        if mems.len() >= MEMORY_SCAN_CAP {
            tracing::warn!(
                target: "coco_journey::snapshot",
                cap = MEMORY_SCAN_CAP,
                "memory scan hit cap; timeline may omit files"
            );
        }
        for mem in mems {
            let (title, description) = mem
                .frontmatter
                .as_ref()
                .map(|fm| (fm.name.clone(), fm.description.clone()))
                .unwrap_or_default();
            let title = if title.is_empty() {
                mem.filename.clone()
            } else {
                title
            };
            let (j_first, j_last, history) = journal_derived(memory_events.get(&mem.filename));
            nodes.push(JourneyNode {
                title,
                description,
                first_seen_ms: j_first.unwrap_or(mem.mtime_ms),
                last_activity_ms: mem.mtime_ms.max(j_last.unwrap_or(i64::MIN)),
                body: JourneyNodeBody::Memory {
                    filename: mem.filename,
                },
                history,
            });
        }
    }

    nodes.sort_by_key(|n| n.last_activity_ms);

    let stats = compute_stats(&nodes);
    JourneySnapshot { nodes, stats }
}

/// Read the skill journal into a name-keyed event index.
fn read_skill_events(config_home: &Path) -> HashMap<String, Vec<JourneyRecord>> {
    let path = coco_skills::agent_scope::agent_journal_path(config_home);
    let records: Vec<JourneyRecord> = coco_maintenance::journal::read_jsonl(&path);
    let mut map: HashMap<String, Vec<JourneyRecord>> = HashMap::new();
    for record in records {
        if let Some(name) = record.event.skill_name() {
            map.entry(name.to_string()).or_default().push(record);
        }
    }
    map
}

/// Read the memory journal into a filename-keyed event index. `MemoryWritten`
/// fans out over its file list; `MemoryConsolidated` is an aggregate fact with
/// no per-file key, so it is disk-authoritatively dropped (no matching node).
fn read_memory_events(memdir: &Path) -> HashMap<String, Vec<JourneyRecord>> {
    let path = coco_memory::path::memory_journal_path(memdir);
    let records: Vec<JourneyRecord> = coco_maintenance::journal::read_jsonl(&path);
    let mut map: HashMap<String, Vec<JourneyRecord>> = HashMap::new();
    for record in records {
        match &record.event {
            JourneyEvent::MemoryWritten { files } => {
                for file in files {
                    map.entry(file.clone()).or_default().push(record.clone());
                }
            }
            JourneyEvent::MemoryDeleted { file } => {
                map.entry(file.clone()).or_default().push(record.clone());
            }
            _ => {}
        }
    }
    map
}

/// Derive `(first_seen, last_activity, capped-newest-first history)` from a
/// node's matching journal events. All `None`/empty when there are none.
fn journal_derived(
    events: Option<&Vec<JourneyRecord>>,
) -> (Option<i64>, Option<i64>, Vec<JourneyRecord>) {
    let Some(events) = events.filter(|e| !e.is_empty()) else {
        return (None, None, Vec::new());
    };
    let first = events.iter().map(|r| r.at_ms).min();
    let last = events.iter().map(|r| r.at_ms).max();
    let mut history = events.clone();
    history.sort_by(|a, b| b.at_ms.cmp(&a.at_ms));
    history.truncate(HISTORY_CAP);
    (first, last, history)
}

/// Extract the on-disk path a skill source points at (its `SKILL.md` / dir).
/// Bundled / plugin / MCP skills have no local file — they never reach here.
fn skill_source_path(source: &SkillSource) -> Option<PathBuf> {
    match source {
        SkillSource::User { path }
        | SkillSource::Project { path }
        | SkillSource::Managed { path } => Some(path.clone()),
        SkillSource::Bundled | SkillSource::Plugin { .. } | SkillSource::Mcp { .. } => None,
    }
}

fn compute_stats(nodes: &[JourneyNode]) -> JourneyStats {
    let mut stats = JourneyStats::default();
    for node in nodes {
        match &node.body {
            JourneyNodeBody::AgentSkill { lifecycle, .. } => match lifecycle {
                AgentSkillLifecycle::Learning { .. } => stats.learning += 1,
                AgentSkillLifecycle::Learned => stats.learned += 1,
                AgentSkillLifecycle::Retired => stats.retired += 1,
            },
            JourneyNodeBody::UserSkill { .. } => stats.user_skills += 1,
            JourneyNodeBody::Memory { .. } => stats.memories += 1,
        }
    }
    stats.busiest_day = crate::timeline::busiest_day(nodes);
    stats
}

#[cfg(test)]
#[path = "snapshot.test.rs"]
mod tests;
