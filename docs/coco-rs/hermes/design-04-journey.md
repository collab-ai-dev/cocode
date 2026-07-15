# Design #4: Journey Learning Timeline + Journal Event Source — Design & Execution Plan

> [← Design #1: Skill Learning Loop](design-01-skill-learning-loop.md) · [Index](README.md)

> **One-liner:** give the already-shipped skill-learn loop (the implementation of designs #1/#3)
> the observability surface it is missing — a `/journey` timeline view that lays out
> *learned skills + memories* over time (the coco version of Hermes journey), backed by an
> **append-only journal (JSONL event source)** that fixes the hard gaps in coco's current
> timeline substrate. A second workstream delivers six concrete skill-learn optimizations
> (including a user-initiated `/learn`).
>
> **No backward-compatibility constraints.** All new types, files, and signatures are designed
> in their final shape. Existing signatures are changed in place where the design calls for it
> (no shims, no deprecation paths, no serde aliases for legacy files).

Status: **reviewed** · Baseline: workspace HEAD as of 2026-07-16 · All `file:line` references
verified against the current tree by an adversarial audit (28/30 confirmed; 2 corrected —
see §11 review log). The body below is the **post-review** design: every accepted finding is
already integrated.

---

## 1. Goals & Non-Goals

### Goals

1. **`/journey` TUI view** — an alt-screen overlay: the top half is a horizontal timeline bar
   chart bucketed by date (skill segments `━` and memory segments `◆`, recency-driven ink
   gradient); the bottom half is a navigable list (`j`/`k` select, `Enter` detail, `e` edit,
   `d` retire/restore/delete) with quarantine/promoted/retired badges and telemetry summaries.
2. **Journal event source** — four trusted write sites (`stamp`, `curator`, memory `extract`,
   memory `dream` — all **host-side post-fork code**, never inside a fork) append to an
   append-only JSONL journal, closing the missing time dimension (when a retire/promote
   happened, when a memory was written, dream rewrites no longer erase history). **Same trust
   model as `stamp`: only trusted Rust code writes it; LLM forks can never reach it** — both
   journal files sit outside the fork write-fence roots, verified against the actual fence
   predicates (§4 Component 1).
3. **Delete/edit operations** — skill = `disabled` frontmatter flip (reversible, no confirm
   needed); memory = hard delete behind a default-No confirm + `MEMORY.md` index-line
   cleanup. Mutations use the permissions-editor round-trip model: `UserCommand` → CLI applies
   to disk → rebuilt payload re-sent → overlay refreshes in place.
4. **skill-learn optimizations** (§7) — notices dual channel, user-initiated `/learn`,
   description budget, review signal + cursor + failure backoff, `SkillLearnConfig`,
   `/skills` quarantine section.

### Non-Goals

- No Hermes-style desktop GPU constellation / graph edges (coco has no `related_skills`
  frontmatter field; open question).
- No web/REST surface (no web dashboard exists; the Event Hub can consume the journal later).
- No headless `coco journey` subcommand in v1 (TUI overlay only; the snapshot/bucketing layer
  is pure, so the later cost is low).
- No timestamp fields added to memory frontmatter (prompt-enforced invariants are unreliable
  and dream consolidation destroys them; the journal is the correct fix).
- No new `Feature` variant (journey is a passive view; the journal write sites already live
  behind `Feature::SkillLearning` / `Feature::AutoMemory`).

---

## 2. Background

### 2.1 Hermes reference (what we keep, what we fix)

Hermes journey (`hermes journey`, aliases `learning`/`memory-graph`) makes three decisions
worth keeping:

1. **Data is a scan, not a database.** `build_learning_graph()` re-scans SKILL.md files +
   a usage sidecar + memory files on every call and assembles `{nodes, edges, stats}`.
   The "learned" filter: `source != base && (created_by == agent || use_count > 0)`.
2. **One data assembler + one renderer shared by all surfaces**, mutations funneled through
   one mutations module.
3. **Timestamps prefer last activity** (usage `last_*_at`, falling back to file mtime);
   bucketing adapts granularity day→month→year; bar length ∝ node count; recency drives an
   ink gradient (old = dim, new = bright).

Hermes' weakness (which coco fixes): the timeline is *reconstructed* from mtimes and
aggregate counters — one consolidation rewrite and history is falsified. coco eliminates
this with the journal event source.

### 2.2 coco-rs current state — verified assets

| Capability | Seam | Reuse |
|---|---|---|
| Precise creation time for agent skills | `SkillProvenance { origin, created_by, created_at }` (`skills/src/lib.rs:322-334`); stamp force-writes it and UPDATE preserves the original (`skill-learn/src/stamp.rs:75-100`) | Preferred source for `first_seen_ms` |
| Quarantine/promotion badges | `SkillProvenanceBadge::{Learning, Learned}` + `provenance_badge()` (`skills/src/lib.rs:200-209`) | List-row badges as-is |
| Dual-timestamp telemetry | `SkillTelemetryStats { success_count, failure_count, patch_count, last_status, last_used_at_ms, last_patched_at_ms }` → `<config_home>/skill_telemetry.json` (`skills/src/telemetry.rs:34-48,74-76`) | Embedded verbatim in skill nodes (no mirror type) |
| Usage scoring | `SkillUsageStats` + `score_for` (`skills/src/usage.rs:70-76,186-193`) | **Not used** (autocomplete-only; the curator already deliberately decoupled from it) |
| Memory enumeration | `scan_memory_files` (`memory/src/scan.rs:35-134`) → `ScannedMemory { path, filename, mtime_ms, frontmatter }` | Memory node source (cap must be parameterized, see R2) |
| Frontmatter emit + atomic write | `coco_frontmatter::emit_frontmatter` (`utils/frontmatter/src/lib.rs:159`) + `coco_utils_common::write_atomic` (`utils/common/src/fs.rs:12`); the disable-write pattern already exists at `skill-learn/src/curator.rs:243`; `frontmatter_keys::DISABLED` owned by skills (`lib.rs:305-307`) | `set_skill_disabled` is a pure extraction — zero new deps |
| Fence-external trusted-file precedent | `.agent-promotions.json` (`skills/src/agent_scope.rs:52-54`, unreachable by forks) | Journal file placement uses the same trick |
| Shared maintenance primitives | `coco-maintenance` (lock + write_fence, `maintenance/src/lib.rs`; both memory and skill-learn already depend on it) | Journal append primitive lands here (needs a `serde` workspace dep — it has only `serde_json` today) |
| Overlay reference | `/permissions` PermissionsEditor: dedicated TUI context + intercept + mutation round-trip refresh (`app/tui/src/state/permissions_editor.rs:283`, `modal_pane/permissions_editor.rs`; CLI apply at `app/cli/src/tui/driver.rs:1319-1331`) | Journey overlay copies this shape wholesale |
| Styled render path | `picker_styled` → `modal_styled_surface` (`app/tui/src/surface_content/mod.rs:124`) → `render_styled_modal_box` (`surface/modal.rs:394`) | Colored timeline Lines go through this path |
| Colored glyph-grid precedent | `/context` view (`app/tui/src/presentation/context_view.rs:26-29`, span construction at :76/:145-146/:181) | Span-assembly technique for the bars |
| Render-time color computation | `coco_tui_ui::color::rgb()` (truecolor pass-through / Ansi256 auto-downsample, `tui-ui/src/color.rs:120,134`) | Recency gradient output |
| Width-safe truncation | `coco_tui_ui::truncate::truncate_to_width` (`tui-ui/src/truncate.rs:21`) | Mandatory for bar/title clipping |
| Runtime-behavior slash precedent | `/dream`: handler sentinel → `classify_sentinel_trigger` (`app/cli/src/tui/slash_execution.rs:560-567`) → CLI-side operation | `/learn` routing model (typed variant instead of sentinel text, see L2) |

### 2.3 coco-rs current state — verified gaps

| # | Gap | Evidence | Fix in this design |
|---|---|---|---|
| G1 | Human-authored skills have no creation time; memories have **no explicit timestamp at all** | memory frontmatter is only name/description/type (`memory/src/store/format.rs:27-35`); the only time is mtime (`scan.rs:115-120`) | Journal accrues events; bootstrap period falls back to mtime (accepted distortion) |
| G2 | dream/consolidation refreshes mtimes, renames, deletes → timeline falsified, filename refs break | dream prompt instructs merge/prune/delete (`memory/prompt/builders.rs:284,338-341`) | Journal is an append-only fact record, immune to rewrites |
| G3 | Retire/promote record no timestamps | `disabled: true` flip is time-less (`skill-learn/src/curator.rs:237-245`); promotions file is a bare name list | `SkillRetired`/`SkillPromoted` journal events |
| G4 | usage/telemetry are aggregate counters, not an event stream → per-day distribution cannot be reconstructed | `skill_usage.json`/`skill_telemetry.json` hold counts + `last_*_at` only | Journal accrues from day one; pre-journal history uses node-level timestamps (Hermes-style compromise) |
| G5 | `disabled` skills are filtered out of discovery → journey cannot see retired skills; `parse_skill_markdown` is **private** (`skills/src/lib.rs:1131`) so outside crates cannot re-parse either | `try_load_skill` skips disabled (`skills/src/lib.rs:1020-1022`) | New shared `agent_scope::scan_agent_skills(include_disabled)` API in `coco-skills`, consumed by both the curator and journey (single scan implementation) |
| G6 | No stable node identity | skill names collide across scopes (`lib.rs:412-447` first-wins); memory filenames get renamed by dream | `JourneyNodeId` typed addressing: canonical path / memdir-relative filename |
| G7 | No delete/edit APIs | Neither side has a typed delete/update; memory `store/` is a **pure data layer with no I/O** and MEMORY.md is read-only by documented invariant | New `coco_skills::set_skill_disabled` + `coco_memory::mutate::delete_entry` (new module, §4 Component 6) |
| G8 | Learned skills are invisible to the user (**liveness deadlock**) | No notice, no reminder; the only clue is the `(learning)` suffix in the autocomplete popup (`app/tui/src/autocomplete/slash.rs:190-212`) | §7 L1 (notices); journey itself is a discovery surface |

The liveness deadlock deserves emphasis because it motivates both workstreams: promotion
requires 5 successful user invocations, but nothing tells the user the skill exists →
telemetry never accrues → never promoted; never-invoked skills are also exempt from
inactivity retirement (`curator.rs:35-50`) → quarantined skills pile up forever.

---

## 3. Architecture Overview

New root-layer crate **`coco-journey`** (`journey/`) — the read-side assembler, mirroring the
responsibility boundary of Hermes `learning_graph`. Dependencies: `coco-skills` +
`coco-memory` + `coco-types` + utils. Root→root dependencies have precedent
(`skill-learn → skills`, `memory`/`skill-learn` → `maintenance`).

- **Consumed only by `app/cli`** — NOT by `commands`. The `/journey` handler stays thin
  (returns a payload-less `DialogSpec::Journey`); snapshot assembly happens at the CLI
  translation site. Rationale: `commands` does not depend on `coco-memory` today (the
  `/memory` handler walks files via `coco_context`), and a `coco-journey` dependency would
  drag `coco-memory`'s heavy transitive tree (`coco-tool-runtime`, `coco-shell`,
  `coco-sandbox`, `coco-apply-patch`, …) into every consumer of `commands`. Verified: no
  dependency cycle either way.
- **Journal write primitive** → `coco-maintenance` (policy-free; both writers already depend
  on it; gains a `serde` workspace dep for the `Serialize` bound).
- **Event schema** → `coco-types` (shared by skill-learn, memory, journey, app/tui — per the
  house 3+ rule).

```
                Write side (trusted host-side Rust, post-fork; forks cannot reach these files)
┌─────────────────────────────────────────────────────────────────────────────────────┐
│ skill-learn/stamp.rs ──── SkillLearned / SkillUpdated ──┐                            │
│ skill-learn/curator.rs ── SkillPromoted / SkillRetired ─┤→ coco_maintenance::       │
│ memory/service/extract.rs (paths_written) ─ MemoryWritten ─┤    journal::append_jsonl│
│ memory/service/dream.rs (paths_written) ─ MemoryConsolidated ┘  (O_APPEND, one line, │
│                                                                  best-effort)        │
└──────────────┬───────────────────────────────────────────────┬──────────────────────┘
               ▼                                               ▼
  <config_home>/skills/.agent-journal.jsonl   <config_home>/projects/<san>/memory-journal.jsonl
  (sibling of the fenced .agent/ root —        (sibling of memdir; the memory fences require
   same placement as the promotions file)       .md AND containment in memdir → unwritable
                                                by extract/dream forks, verified §4 C1)
               │                                               │
               └──────────────────┬────────────────────────────┘
                                  ▼  Read side
                     ┌────────────────────────────────┐
                     │ coco-journey (root, new crate) │
                     │  snapshot.rs: build_journey     │←── skills::agent_scope::scan_agent_skills
                     │   (disk scan + telemetry join   │←── SkillManager user/project skills
                     │    + journal merge)             │←── scan_memory_files(memdir)
                     │  timeline.rs: bucketize          │←── skill_telemetry.json
                     └──────────────┬─────────────────┘     (pure fns, day→month→year)
                                    ▼
  commands: /journey → CommandResult::OpenDialog(DialogSpec::Journey)   [payload-less, thin]
                                    ▼
  app/cli slash_execution: spawn_blocking(build_journey) → wire payload
        → CoreEvent::Tui(TuiOnlyEvent::OpenJourney { payload })
                                    ▼
  app/tui ── ModalState::Journey(JourneyState) ── presentation/journey.rs → Vec<Line>
        │  e / d actions (UserCommand is in-process — no serde, PathBuf fine)
        ▼
  UserCommand::ApplyJourneyAction ──► app/cli driver.rs (permission-apply precedent :1319-1331)
        applies set_skill_disabled / delete_entry / editor → journal event
        └─► spawn_blocking(build_journey) → re-send OpenJourney (in-place refresh)
```

**Trust boundary** (three layers, consistent with skill-learn): journal files live **outside
both write-fence roots** — verified against the actual predicates: the memory fences take
`allowed_write_roots = vec![memory_dir]` (`extract.rs:711-713`, `dream.rs:536-541`) and the
inner ring additionally requires a `.md` suffix (`memory/src/can_use_tool.rs:150,358-364`);
the skill fence is contained to `.agent/` with an extension whitelist that excludes `jsonl`
and denies dot-prefixed names (`fence.rs:52`). All write sites are in trusted post-spawn code
(stamp/curator/service finalize); the reader skips corrupt lines with tracing — one bad line
never poisons the view.

### 3.1 Decision log

| # | Decision | Rationale |
|---|---|---|
| D1 | Journal (append-only JSONL) is the authoritative event source; disk scan is the bootstrap fallback | Fixes G1–G4 at the root. Same trust model as `stamp.rs`: trusted Rust only, fence-external placement (promotions-file precedent), placement verified against both fence predicates |
| D2 | New root crate `coco-journey` for read-side assembly, consumed only by `app/cli` | TUI must not own domain logic (TEA seam); skill-learn deliberately avoids a `coco-memory` dependency; `commands` must not inherit `coco-memory`'s transitive tree. The assembler needs skills+memory; only `app/cli` already carries both |
| D3 | Event schema in `coco-types`; append primitive in `coco-maintenance` | Schema is consumed by 4 crates (house rule: 3+ → coco-types). The primitive is policy-free infra; the `T: Serialize` generic keeps maintenance free of coco-types |
| D4 | No new `Feature`; no new config section in v1 | Journey is a passive view (like `/context`); writers are already gated by `Feature::SkillLearning` / `Feature::AutoMemory`. Constants live in the owning crates; `journal_enabled` joins `SkillLearnConfig` when L5 lands |
| D5 | TUI copies the PermissionsEditor shape (dedicated TUI-local context, Global-only keybinding stack, intercept, round-trip mutations) | Richest existing overlay with list + confirm sub-mode + disk-mutation refresh. The generic Picker context eats letters into its filter — `j`/`k` would break (`modal_pane/mod.rs:49`). A Global-only `context_stack` arm avoids touching the user-rebindable `keybindings` crate enum entirely |
| D6 | Skill "delete" = `disabled` flip, **no confirm** (reversible from the same view); memory delete = hard delete behind default-No confirm | Skills: curator is write-only, files stay, one-line reversal — a confirm would punish a reversible action. Memories: irreversible (no recycle bin), so confirm; journal keeps a `MemoryDeleted` fact |
| D7 | Journey and the curator share one `agent_scope::scan_agent_skills` API instead of journey re-parsing | `parse_skill_markdown` is private (`lib.rs:1131`) and `SkillManager` filters disabled skills (G5); the scan logic belongs next to `agent_scope`, which owns the directory. One implementation, two consumers |
| D8 | `JourneyNodeId` = canonical SKILL.md path / memdir-relative filename; kind-specific data lives in a `JourneyNodeBody` enum payload | Typed addressing (G6). The body enum makes illegal states unrepresentable — no `status: Memory` next to `id: Skill`, no `Option<telemetry>` with a "Some iff skill" prose invariant |
| D9 | In-process vs wire types are distinct: `UserCommand`/`JourneyAction` carry `PathBuf` (in-process mpsc, no serde — `app/tui/src/command.rs:93` precedent); `TuiOnlyEvent` payloads use `String` paths, snake_case fields, `tag = "kind"` enums | Matches the verified dominant convention in `common/types/src/event.rs` (`MemoryDialogEntry.path: String` :2871, `PermissionsEditorPayload.cwd: String` :3308) |

---

## 4. Detailed Design

### Component 0 — Event schema (`coco-types`) + journal primitives (`coco-maintenance`)

**`common/types/src/journey.rs` (new):**

```rust
/// One fact on the learning timeline. Append-only; never rewritten.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JourneyRecord {
    /// Event time (epoch ms). Write sites use their own clock; forks are never trusted.
    pub at_ms: i64,
    /// Originating session (audit/backtrace); None when absent (e.g. curator ticks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(flatten)]
    pub event: JourneyEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum JourneyEvent {
    /// Review fork created a new agent skill (stamp saw no prior `created-at`).
    SkillLearned { name: String },
    /// Review fork updated an existing agent skill or its support files.
    SkillUpdated { name: String },
    /// Curator promoted on telemetry (≥5 invocations, success rate ≥ 0.8).
    SkillPromoted { name: String },
    /// Curator or user retired the skill (`disabled: true` flip).
    SkillRetired { name: String, reason: SkillRetireReason },
    /// User restored a retired skill via /journey (`disabled` flipped back).
    SkillRestored { name: String },
    /// Memory extract fork wrote topic files (memdir-relative paths).
    MemoryWritten { files: Vec<String> },
    /// Dream consolidation touched files (aggregate fact for merges/renames/deletes).
    MemoryConsolidated { files_touched: i32 },
    /// User deleted a memory via /journey.
    MemoryDeleted { file: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillRetireReason {
    /// ≥5 invocations with success rate < 0.34.
    FailureRate,
    /// Previously used, then idle for 90 days.
    Inactivity,
    /// User action via /journey.
    Manual,
}

/// Typed node addressing (fixes G6): canonical SKILL.md path for skills
/// (unambiguous across same-named scopes), memdir-relative filename for memories.
/// In-process type — the wire payload mirrors it with String paths (D9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JourneyNodeId {
    Skill { path: PathBuf },
    Memory { filename: String },
}

/// In-process mutation request carried by UserCommand (no serde; D9).
#[derive(Debug, Clone)]
pub enum JourneyAction {
    RetireSkill { path: PathBuf },
    RestoreSkill { path: PathBuf },
    DeleteMemory { filename: String },
    OpenInEditor { id: JourneyNodeId },
}
```

Notes:

- Wire-tagged union (`#[serde(tag = "event")]`) per crate convention; variant names form a
  closed set (type-safety rule: no bare strings for closed sets).
- Skill events carry `name` only, not a path: the `.agent/` directory is flat
  (name = directory basename) and the journal is that directory's sibling — context is
  self-evident. Memory events carry memdir-relative paths.
- **No version field on `JourneyRecord`**: with compatibility disregarded, a schema change
  simply means the reader skips old lines (serde failure → skip). The timeline is an
  observability surface, not a ledger; losing pre-change history is acceptable.

**`maintenance/src/journal.rs` (new, policy-free):**

```rust
/// Append one JSON line to an append-only journal. Best-effort: any failure is
/// tracing::warn only, never propagated (an observability plane must not
/// disturb the main loop). POSIX O_APPEND makes the offset positioning atomic;
/// small single write()s on local filesystems do not interleave in practice
/// (no hard spec guarantee — NFS can break it). The reader's skip-corrupt-line
/// behavior absorbs the residual risk.
pub fn append_jsonl<T: Serialize>(path: &Path, record: &T);

/// Read the whole journal back. Corrupt lines are skipped with a
/// tracing::debug count. A missing file yields an empty Vec.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Vec<T>;

/// Size guard: when the file exceeds max_bytes, rename it to `<path>.1`
/// (single generation; an existing `.1` is overwritten). Called by write
/// sites before appending; the 4 MiB default constant lives at the call site.
/// Concurrent rotation from two processes is benign: rename is atomic and the
/// loser's failure is ignored.
pub fn rotate_if_over(path: &Path, max_bytes: u64);
```

The `T: Serialize` generic keeps `coco-maintenance` free of a `coco-types` dependency
(consistent with the policy-free positioning of `lock`/`write_fence`); the concrete type is
bound by callers. **`maintenance/Cargo.toml` gains `serde.workspace = true`** (it has only
`serde_json` today — verified).

**Blocking-I/O rule:** `append_jsonl`/`read_jsonl` are sync file I/O. Callers in async
contexts (`stamp`, service finalize, CLI assembly) wrap them in `tokio::task::spawn_blocking`
— appends are tiny but the house async convention applies uniformly, and the read side walks
whole files.

### Component 1 — Four write sites

All four are *additions inside existing functions* (host-side post-fork code — never inside
a fork); zero new control flow:

| Write site | Location | Event |
|---|---|---|
| After stamping | `skill-learn/src/stamp.rs::stamp_written_skills` (:20), after each successfully processed SKILL.md | prior `created-at` absent → `SkillLearned`; present → `SkillUpdated` (stamp already distinguishes these two cases, :75-100) |
| Curator retire/promote | `skill-learn/src/curator.rs::retire` (:237-245) and the promotions write | `SkillRetired { reason }` / `SkillPromoted` |
| Extract completion | `memory/src/service/extract.rs` where `response.paths_written` is handled (:753-770) | `MemoryWritten { files }` (memdir-relativized) |
| Dream completion | `memory/src/service/dream.rs` (:585-602; `files_touched_count` already in scope) | `MemoryConsolidated { files_touched }` |

Journal path resolution:

- Skill side: `agent_scope` gains `pub fn agent_journal_path(config_home: &Path) -> PathBuf`
  (= `skills/.agent-journal.jsonl`), next to `promotions_path` (`agent_scope.rs:52-54`).
- Memory side: `memory/src/path/resolve.rs` gains
  `pub fn memory_journal_path(memdir: &Path) -> PathBuf`
  (= `memory-journal.jsonl` in memdir's parent, i.e. `projects/<san>/memory-journal.jsonl` —
  named after what it journals, next to the `memory/` dir it observes).

**Fence verification (why forks cannot write these):** the memory forks' outer ring is
`allowed_write_roots = vec![memory_dir]` (`extract.rs:711-713`, `dream.rs:536-541`) and the
inner ring requires `.md` under memdir (`can_use_tool.rs:150,358-364`; the dream `rm`
allowance is `.md`-only, non-recursive, glob-free, `:300-343`) — a memdir *sibling* `.jsonl`
fails both rings. The skill fence contains writes to `.agent/` with extension whitelist
`[md,txt,json,yaml,yml,toml]` and denies dot-prefixed components (`fence.rs:52`) —
`.agent-journal.jsonl` is outside the root, dot-prefixed, *and* has a non-whitelisted
extension. Triple protection.

`session_id`: the stamp/extract call chains already carry a `SessionId` (spawn request);
the curator has no session context → `None`.

### Component 2 — `coco-journey` assembler (`journey/src/snapshot.rs`)

```rust
/// One learning-timeline node (a skill or a memory topic file).
#[derive(Debug, Clone)]
pub struct JourneyNode {
    pub title: String,            // skill display_name / memory frontmatter name
    pub description: String,      // detail panel; the presentation layer truncates
    /// When it entered the timeline: earliest journal event > provenance created_at > mtime.
    pub first_seen_ms: i64,
    /// Latest activity: max(latest journal event, telemetry last_*_at, mtime).
    pub last_activity_ms: i64,
    /// Kind + kind-specific data. Illegal states unrepresentable (D8): telemetry
    /// exists exactly for skills, lifecycle exactly for agent skills.
    pub body: JourneyNodeBody,
    /// This node's journal history (detail panel), newest first, capped at 20.
    pub history: Vec<JourneyRecord>,
}

#[derive(Debug, Clone)]
pub enum JourneyNodeBody {
    AgentSkill {
        path: PathBuf,                       // canonical SKILL.md path
        lifecycle: AgentSkillLifecycle,
        telemetry: SkillTelemetryStats,      // reused from coco_skills — no mirror type
    },
    UserSkill {
        path: PathBuf,
        telemetry: SkillTelemetryStats,
    },
    Memory {
        filename: String,                    // memdir-relative
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSkillLifecycle {
    /// Quarantined (not yet promoted).
    Learning,
    /// Promoted.
    Learned,
    /// disabled: true.
    Retired,
}

impl JourneyNode {
    /// Derived addressing for mutations (single construction site keeps id/body coherent).
    pub fn id(&self) -> JourneyNodeId { /* from body */ }
}

#[derive(Debug, Clone, Default)]
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

pub struct JourneySnapshot {
    pub nodes: Vec<JourneyNode>,   // ascending last_activity_ms
    pub stats: JourneyStats,
}

/// Assembly entry point. Infallible: any missing directory / corrupt file means
/// that source contributes nothing (plus tracing) — never an error. Sync,
/// blocking I/O: async callers wrap in spawn_blocking.
pub fn build_journey(paths: &JourneyPaths) -> JourneySnapshot;

pub struct JourneyPaths {
    pub config_home: PathBuf,     // skills/.agent, telemetry, skill journal
    pub memdir: Option<PathBuf>,  // current project memdir; None (no git project) = empty memory side
}
```

Assembly rules (Hermes' learned-filter, rewritten in coco semantics):

1. **Agent skills**: via the new shared API
   `coco_skills::agent_scope::scan_agent_skills(config_home, IncludeDisabled::Yes)`
   (D7) — location-keyed walk of `.agent/*/SKILL.md` including `disabled: true` files (G5);
   returns parsed definitions + provenance. The curator is refactored onto the same function
   (single scan implementation). `lifecycle` derives from the promotions set + the disabled
   bit.
2. **User/project skills**: enumerate via `SkillManager::all()` (`lib.rs:654-659`), filter
   `source != Bundled && telemetry.total_invocations() > 0` — the coco equivalent of Hermes'
   "non-base AND used": skills you actually used are part of your journey; decorative ones
   are not.
3. **Memories**: `scan_memory_files(memdir, max_files)` (cap parameterized, see R2); one
   node per file.
4. **Journal join**: `read_jsonl` both journals, merge-sort by `at_ms`; a node's
   `first_seen_ms` prefers the earliest matching journal event (`SkillLearned` /
   `MemoryWritten` containing that file), falls back to `provenance.created_at`, then mtime.
   Each node gets its capped `history` (newest 20 matching events). Events whose node no
   longer exists on disk feed `stats` only — **the snapshot is disk-authoritative** (R5).
   A memory renamed by dream will not match its old events → treated by new-file mtime
   (accepted; see R1).

### Component 3 — Timeline bucketing (`journey/src/timeline.rs`, pure functions)

```rust
pub struct TimelineBucket {
    pub start_ms: i64,
    pub label: String,        // "4 Jul" / "Jul 2026" / "2026" (local timezone)
    pub skills: i32,
    pub memories: i32,
    /// Recency of the newest node in the bucket (0..=1); drives the row's ink.
    pub recency: f32,
}

/// Hermes-style adaptive granularity: try day; if rows exceed max_rows try month,
/// then year. Spans ≤ 32 days lock to day granularity (scroll rather than lose
/// day resolution).
pub fn bucketize(nodes: &[JourneyNode], max_rows: usize, now_ms: i64) -> Vec<TimelineBucket>;
```

Pure, no I/O, `now_ms` injected (testable clock). Recency mapping
(`[0.06, 1.0]` linear over the time span, ordinal fallback for a single-instant span) is a
**private helper** inside `timeline.rs` — a parallel-`Vec<f32>` public API would couple
callers to index order for no benefit; buckets carry their own `recency`. The presentation
layer only performs the mechanical bucket→span mapping.

### Component 4 — `/journey` command + wire chain

The command side stays **thin** (D2); assembly happens in `app/cli`:

| Link | Location | Content |
|---|---|---|
| Registration | `commands/src/implementations.rs` | `RegisteredCommand { name: "journey", command_type: CommandType::LocalOverlay, handler }` — the handler is trivial (`Ok(CommandResult::OpenDialog(DialogSpec::Journey))`) and lives inline in `implementations.rs`; no new handler file, **no `coco-journey` dependency in `commands`** |
| DialogSpec | `commands/src/lib.rs:162` | payload-less `DialogSpec::Journey` variant |
| CLI translation + assembly | `app/cli/src/tui/slash_execution.rs:661` match | `DialogSpec::Journey` → resolve `JourneyPaths` (config_home + current memdir) → `spawn_blocking(build_journey)` → snapshot → wire payload → `event_tx.send(CoreEvent::Tui(TuiOnlyEvent::OpenJourney { payload }))` |
| Wire types | `common/types/src/event.rs` | `TuiOnlyEvent::OpenJourney { payload: JourneyDialogPayload }` + wire mirrors of nodes/buckets/stats. Convention (verified, D9): snake_case fields, `String` paths, `#[serde(tag = "kind", rename_all = "snake_case")]` for the node-body enum, `Option` as `#[serde(default, skip_serializing_if = "Option::is_none")]`, `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]`. Event-name mapping added at `app/query/src/emit.rs:236` and `app/tui/src/server_notification_handler.rs:191` |
| TUI fold | `app/tui/src/server_notification_handler/tui_only.rs` (`OpenMemoryDialog` precedent :521-532) | `state.ui.show_modal(ModalState::Journey(JourneyState::from_wire(payload)))` |

Buckets are pre-computed at assembly time with a nominal `max_rows`; the TUI does not
re-bucket on resize in v1 (bars re-scale to width at render; row count is fixed at open —
see R6b).

### Component 5 — TUI overlay (PermissionsEditor shape)

**Three new TUI files** (each with a companion `.test.rs`):

```
app/tui/src/state/journey.rs          — JourneyState
app/tui/src/modal_pane/journey.rs     — map_key + intercept
app/tui/src/presentation/journey.rs   — journey_lines projection (+ insta snapshots)
```

```rust
// app/tui/src/state/journey.rs
pub struct JourneyState {
    pub buckets: Vec<TimelineBucketWire>,
    pub nodes: Vec<JourneyNodeWire>,   // list order = ascending last_activity_ms
    pub stats: JourneyStatsWire,
    pub selected: usize,
    pub mode: JourneyMode,
}

pub enum JourneyMode {
    List,
    Detail,                              // Enter: full description + telemetry + event history
    /// Memory deletion only (irreversible). Skill retire/restore is immediate (D6).
    DeleteMemoryConfirm { yes_selected: bool },  // defaults to No
}
```

**Key routing** — a new TUI-local context, four exhaustive sites (verified; the
user-rebindable `keybindings` crate enum is NOT touched — journey uses a Global-only stack,
the PermissionsEditor precedent):

1. variant on the TUI-local enum (`app/tui/src/keybinding_bridge.rs:19`);
2. arm in `active_context(state)` (`keybinding_bridge.rs:76`, matches `ModalState`);
3. arm in the Layer-2 per-surface `map_key` match (`keybinding_bridge.rs:~344-367`);
4. arm in `context_stack()` (`app/tui/src/keybinding_resolver.rs:212`) →
   `vec![KbContext::Global]`.

**Never the generic `Picker` context** (letters get eaten by its filter; `j`/`k` would
break, `modal_pane/mod.rs:49`). `map_key`: `j`/`k`/`↑`/`↓` → nav; `Enter` → detail/confirm;
`e` → edit; `d` → retire/restore (skills, immediate) or DeleteMemoryConfirm (memories);
`Esc` → back/close. Intercept hooked after `update.rs:147`.

**Rendering** (`presentation/journey.rs`, styled path — add an arm at
`surface_content/mod.rs:124`):

```
 ✦ Journey · learned skills & memories
   ● skills 12   ◆ memories 34   ★ promoted 3   ✕ retired 2

   4 Jul │━━━━━━━◆━◆           3+2
  12 Jul │━━━━                 2
  15 Jul │━━━━━━━━━━━◆━━◆      5+3   ◀ now
          oldest ──────────→ now
 ──────────────────────────────────────────────
 ❯ ● fix-nextest-filter        (learning 2/5)    15 Jul
   ● wt-rebase-conflicts       (learned) ★       14 Jul
   ◆ coco-voice-and-disk-gotcha                  12 Jul
   ✕ parse-log-format          (retired: failures) 30 Jun
 ──────────────────────────────────────────────
  j/k select · enter detail · e edit · d retire/delete · esc close
```

- Bars are `Span::styled` runs of block glyphs (the `/context` grid precedent,
  `context_view.rs:26-29`); skill segments use the accent color, memory segments a secondary
  color; each row's foreground linearly interpolates between two RGB endpoints by `recency` —
  endpoint literals are defined inside the renderer and emitted through `color::rgb()`.
  **Never extract channels from `Theme` fields**: under Ansi256 the theme is already
  downsampled to `Indexed` (`theme.rs:529`).
- All clipping through `truncate_to_width`; byte slicing is forbidden
  (root CLAUDE.md UTF-8 rule).
- The list reuses `render_select_list` (`tui-ui/src/widgets/select_list.rs:71`) or an
  equivalent hand-rendered form.
- Badges: `Learning` shows quarantine progress `n/5` (`telemetry.success_count`; shared
  rendering with §7 L6); `Retired` shows the reason (i18n key).
- **Animated reveal is deferred to P2 polish**: `ui_animation()` requires `modal.is_none()`
  to arm frames (`state/mod.rs:284-304`, gate at :297); v1 renders statically.
- i18n: add `dialog.title_journey` etc. to `locales/en.yaml` + `zh-CN.yaml`.

### Component 6 — Mutations (edit / delete round-trip)

**New domain APIs (G7):**

```rust
// coco-skills (Tier-2; crate::Result<T> = Result<T, SkillsError>)
/// Atomically flip the `disabled` frontmatter of a SKILL.md. Pure extraction of
/// the existing pattern at curator.rs:243 (coco_frontmatter::parse + emit_frontmatter
/// + write_atomic; frontmatter_keys::DISABLED already lives in this crate) —
/// zero new dependencies. curator::retire is refactored onto it (single write point).
pub fn set_skill_disabled(skill_md: &Path, disabled: bool) -> crate::Result<()>;

// coco-memory — NEW module memory/src/mutate.rs (NOT store/: store is a documented
// pure data layer with no I/O). Sync fn; async callers use spawn_blocking.
/// Delete one memory: remove the topic file and prune MEMORY.md index lines whose
/// link target is the filename (parse via store::parse_memory_index, rewrite via
/// coco_utils_common::write_atomic). Idempotent: a missing file still prunes the index.
///
/// Invariant note: coco-memory's documented rule is that the runtime never
/// auto-regenerates MEMORY.md (read + truncate only). A user-initiated prune of
/// now-dangling index lines is a *user* mutation, not auto-regeneration — the
/// invariant is about autonomous rewrites. Recorded here so the boundary stays sharp.
pub fn delete_entry(memdir: &Path, filename: &str) -> Result<(), MemoryError>;
```

**TUI → CLI round trip** (permissions-editor model): the TUI sends
`UserCommand::ApplyJourneyAction { action: JourneyAction }` over the in-process mpsc
(`UserCommand` lives at `app/tui/src/command.rs:93`, carries `PathBuf` freely — no serde;
verified D9). The CLI handles it in the `driver.rs` match (the `ApplyPermissionUpdate`
precedent at `driver.rs:1319-1331`): execute the action → append the matching journal event
(`SkillRetired { reason: Manual }` / `SkillRestored` / `MemoryDeleted`) →
`spawn_blocking(build_journey)` → re-send `OpenJourney` (in-place refresh precedent
`tui_only.rs:585-595`; never mutate the TUI's in-memory copy directly). `OpenInEditor`
follows the editor workflow (`editor_workflows.rs:199-211`; external editor launch precedent
`run_open_memory_file` :3-26) and refreshes on return. Refresh preserves selection by node
id (fall back to clamped index). A failed action surfaces as a status toast; the overlay
stays open.

Deletion semantics (vs. Hermes):

- **Skills are never physically deleted** (curator write-only philosophy; Hermes moves to
  `.archive/`, coco's `disabled: true` is equivalent recoverability with less machinery —
  file stays in place, one-line reversal). Retire/restore is immediate — no confirm (D6):
  the same view shows the Retired node and `d` flips it back.
- **Memories are hard-deleted** (Hermes-equivalent) behind a default-No confirm, with a
  `MemoryDeleted` journal fact; there is no recycle bin (memdirs are usually not in a repo).

---

## 5. Configuration & Feature Gating

- **No new `Feature`.** Journey is a passive observability view (like `/context`); the
  journal write sites live behind the existing `Feature::SkillLearning` (stamp/curator) and
  `Feature::AutoMemory` (extract/dream) gates — feature off ⇒ those events simply never occur.
- **No new config section in v1.** Constants converge in their owning crates:
  `JOURNAL_MAX_BYTES = 4 MiB` (write sites), the relaxed memory-scan cap passed by journey
  (see R2). When L5 lands, `journal_enabled` joins `SkillLearnConfig` — no standalone
  `JourneyConfig`.
- **No new EnvKey** (the journal is a best-effort observability plane; no env escape hatch
  needed).

## 6. Error Handling Tiers

| Layer | Location | Convention |
|---|---|---|
| `coco-journey` | root | **No crate error type** (skill-learn precedent, `lib.rs:20-23`): `build_journey` is infallible — missing/corrupt sources contribute nothing + `tracing::warn`; `bucketize` is pure |
| `coco-maintenance::journal` | L0 | `append_jsonl` returns `()` (best-effort); `read_jsonl` skips corrupt lines; `rotate_if_over` failure is tracing-only |
| `coco_skills::set_skill_disabled` | Tier-2 | `crate::Result<()>` (thiserror `SkillsError`) |
| `coco_memory::mutate::delete_entry` | Tier-3 | snafu `MemoryError`, Resource-class `StatusCode` |
| TUI/CLI round trip | Tier-1 | anyhow; a failed action surfaces as a status-line toast, overlay stays open |

Zero `.unwrap()`/`.expect()` outside tests (workspace clippy denies both); poisoned locks
recover via `PoisonError::into_inner` (memory precedent).

---

## 7. skill-learn Optimizations (Workstream B)

> Root cause both workstreams attack: the **liveness deadlock** (§2.3 G8). Priorities below
> are ordered by value/effort.

### L1 — Notices dual channel (S, highest priority)

Lands design-01 component 8 (review corrections #4/#5 already enumerate the mechanical
work): a `SkillLearnNotice` inbox; stamp pushes on success; engine finalize projects
(a) a user-visible `SystemMessage` — "Learned skill: X — quarantined until 5 successful
uses"; (b) a model-visible `<system-reminder>` (new `AttachmentKind::SkillLearnedReminder`,
filling in **every** exhaustive match arm in `attachment_kind.rs`). Same site and moment as
the Phase-3 journal write (after stamp) — one wiring pass feeds both channels.

**Delivery latency (verified against the memory pattern):** the review fork is a detached
background task; the engine drains inboxes at each turn's finalize *immediately after
scheduling* (`memory/src/runtime.rs:1395-1397` schedule → `:862` drain), so a notice pushed
by a fork that completes after turn N's drain surfaces at turn **N+1**'s (or a later turn's)
finalize — exactly like memory's `SystemMemorySavedMessage`
(`app/query/src/engine_finalize_turn.rs:656-665`). This is inherent to background work; the
doc states it so nobody "fixes" it into a same-turn block.

### L2 — User-initiated `/learn` (M, Hermes' flagship)

`/learn [free-form description]` — Hermes' prompt-injection distillation, rebuilt on coco
architecture:

- Command: `RegisteredCommand("learn")` behind `Feature::SkillLearning`; the handler returns
  a new typed `CommandResult::TriggerSkillLearn { directive }`. (The existing
  runtime-behavior precedent is `/dream`'s *sentinel-text* classification,
  `slash_execution.rs:560-567` → `SlashOutcome::TriggerDream`; a typed variant is chosen
  instead — closed sets are enums, not magic strings.)
- Routing (verified seams): `dispatch_slash_command` (`slash_execution.rs:305-312`) already
  holds the `SessionHandle`, which exposes `history_messages()`
  (`session_handle/history.rs:45`) for the fork-context snapshot — **no CommandQueue detour
  needed**. `SkillReviewRuntime` is currently `pub(in crate::session::session_runtime)`
  (`resources/handles.rs:212-213`); add a public `skill_review_runtime()` accessor on
  `SessionHandle` (one-liner, mirrors the existing `memory_runtime()` accessor at
  `session_handle/capabilities.rs:85`).
- Execution: `SkillReviewRuntime` gains
  `manual_review(directive, session_id, fork_context) -> ReviewTrigger`:
  **bypasses the throttle, respects the single-flight flag**; `directive` is injected into
  the review prompt as the top-priority instruction ("the user explicitly asked to learn:
  {directive} — honor their named sources; the preference order still applies").
- Source semantics (Hermes-aligned, fence-constrained): conversation history is naturally in
  `fork_context_messages`; local dirs/files are freely readable by the fork (the fence allows
  reads, `fence.rs:75-124`); **URLs get no fence hole** — WebFetch stays denied; the user has
  the main agent fetch first (content enters history), then runs `/learn`. This is exactly
  Hermes' "gather with the tools you already have" philosophy, at zero fence change.
- Audit: `SkillAuthor` gains a `Manual` variant (`created-by: manual`). Quarantine applies
  unchanged (the trust model does not relax because the user asked; the user can immediately
  invoke the skill to push it toward promotion).
- Journal: records `SkillLearned` with the originating `session_id`.

### L3 — Description budget enforcement (XS)

Stamp-time hard truncation of `description` to the listing budget (UTF-8-safe via
`coco_utils_string::truncate_str`), plus one review-prompt line: "description ≤ 60
characters, one sentence" (Hermes' 60-char rule; same reason — the index loads every
session, overlong descriptions burn budget silently).

### L4 — Review signal + cursor + failure backoff (M)

The three unlanded pieces of design-01 components 1/2, scoped to what the finalize seam can
see **today** (verified):

- **Signal v1** = (a) tool-call count — `tool_calls_last_turn` is already computed at
  `engine_finalize_turn.rs:613-614` a few lines above the trigger call (:697); passing it
  into `run_skill_review_finalize` is a one-line signature change (the text-tail call site
  passes 0 by definition); (b) "a skill was invoked this turn" via a cheap last-turn history
  scan for SkillTool `tool_use` blocks. **Deferred**: "a task-list item completed this turn"
  — `TaskListHandleRef` exposes only async CRUD with no per-turn completion delta
  (`core/tool-runtime/src/task_list_handle.rs:55-95`); it needs a revision marker and is not
  worth the plumbing for v1.
- Empty signal → `ReviewTrigger::Skipped` at zero cost before the throttle.
- Cursor: fork only the message delta since the last review (advances only on `Completed`).
- Failure backoff: effective throttle `<< min(consecutive_failures, 5)`.

Turns "blind-fire every 5 turns" into "fire when there is material", directly saving
`ModelRole::Memory` calls.

### L5 — `SkillLearnConfig` + EnvKey (S)

Lands the design-01 config table: define in `common/config/src/sections.rs`, mount on
`RuntimeConfig`, add `COCO_SKILL_LEARN_*` EnvKeys. Moves every hardcoded constant into
config (defaults unchanged): `DEFAULT_REVIEW_THROTTLE = 5` (`runtime.rs:27`),
`DEFAULT_REVIEW_MAX_TURNS = 6` (`review.rs:37`), the curator's five thresholds
(`curator.rs:35-50`). L4's signal thresholds and `journal_enabled` also live here.

### L6 — `/skills` quarantine section (S)

The `/skills` dialog gets an explicit "Learning" section for quarantined skills, each row
showing quarantine progress (`success_count`/5) and a "try it" hint — the lightweight coco
counterpart of Hermes' write-approval gate (coco's gate is on the *invocation* side, not the
write side; the section makes the gate visible). The journey detail panel reuses the same
progress rendering.

---

## 8. Execution Plan

### 8.1 Work-package map & dependencies

```
Workstream A (journey)                        Workstream B (skill-learn opts)

A1 coco-journey crate: snapshot (scan)        B1 notices dual channel
 │   + scan_agent_skills + set_skill_disabled B3 description budget
 │   + scan cap param                         B5 SkillLearnConfig ──► B4 signal/cursor/backoff
 ▼                                            B2 /learn command
A2 timeline bucketing (same crate)            B6 /skills quarantine section
 │
 ▼
A3 /journey command + wire chain (thin commands; assembly in app/cli)
 │
 ▼
A4 TUI overlay (read-only)
 │
 ├────────► A6 mutations (needs A4 + A1's set_skill_disabled)
 │
A5 journal substrate (independent of A3/A4; its reader join lands in A1's snapshot)
     — shares the stamp write site with B1: coordinate or land B1 first
```

- Critical path for a visible feature: **A1 → A2 → A3 → A4** (read-only `/journey`).
- A5 can proceed in parallel with A3/A4 (different files except the snapshot join in
  `journey/`).
- B1/B3/B5/B6 are independent of Workstream A and of each other; B4 wants B5's config keys;
  B2 benefits from B1 (its outcome becomes visible).
- Suggested PR order: `A1+A2` → `A3+A4` → `B1` → `A5` → `A6` → `B3` → `B5` → `B4` → `B2` → `B6`.

Per-iteration verification is `just quick-check`; `just pre-commit` runs exactly once per PR
immediately before commit (house rule). TUI snapshot flow:
`cargo test -p coco-tui` → `cargo insta pending-snapshots --manifest-path app/tui/Cargo.toml`
→ review → `cargo insta accept -p coco-tui`.

---

### WP-A1 — `coco-journey` crate: snapshot assembler (scan-based)

**Scope:** new crate skeleton + disk-scan assembly (no journal yet) + the three enabling
domain changes in `coco-skills` / `coco-memory`.

**New files**
- `journey/Cargo.toml` — deps: `coco-skills`, `coco-memory`, `coco-types`,
  `coco-utils-string`, `chrono`, `serde`, `tracing`
- `journey/src/lib.rs` — module docs + re-exports (`JourneySnapshot`, `JourneyNode`,
  `JourneyNodeBody`, `AgentSkillLifecycle`, `JourneyStats`, `build_journey`)
- `journey/src/snapshot.rs` + `snapshot.test.rs`

**Modified files**
- `coco-rs/Cargo.toml` — workspace member + `coco-journey.workspace`
- `skills/src/agent_scope.rs` — **new shared scan API** (D7, fixes G5 without exposing the
  private parser):
  `pub fn scan_agent_skills(config_home: &Path, include_disabled: IncludeDisabled) -> Vec<AgentSkillScan>`
  (location-keyed walk + parse + provenance + disabled bit; `IncludeDisabled` is a two-variant
  enum per the no-bool-params rule)
- `skill-learn/src/curator.rs` — refactored onto `scan_agent_skills` (single scan
  implementation; behavior identical)
- `skills/src/lib.rs` — `pub fn set_skill_disabled(skill_md: &Path, disabled: bool) -> crate::Result<()>`
  (pure extraction of the `curator.rs:243` pattern: `coco_frontmatter::parse` +
  `emit_frontmatter` + `write_atomic`; zero new deps — verified);
  `curator::retire` refactored to call it
- `memory/src/scan.rs:15,35` — **breaking signature change**:
  `scan_memory_files(dir: &Path, max_files: usize)`; existing callers pass the old
  `MAX_SCANNED_FILES` constant explicitly; journey passes `2000` and traces on overflow

**Checklist**
1. Crate skeleton; layer check: no `coco-messages`/`coco-inference` deps (skill-learn parity).
2. Agent-skill nodes via `scan_agent_skills(IncludeDisabled::Yes)`; lifecycle from
   promotions set + disabled bit.
3. User/project skill selection via `SkillManager::all()` + telemetry filter
   (`source != Bundled && total_invocations > 0`).
4. Memory nodes from `scan_memory_files`.
5. Telemetry join (`skill_telemetry.json` load) → `SkillTelemetryStats` embedded in
   `JourneyNodeBody` (no mirror type).
6. `first_seen_ms` / `last_activity_ms` derivation (provenance → mtime for now; journal slot
   wired in WP-A5).

**Tests** (`snapshot.test.rs`, tempdir fixtures)
- Node filter matrix: bundled excluded; unused user skill excluded; used user skill included;
  disabled agent skill included with `Retired`.
- Lifecycle derivation matrix (promotions × disabled).
- Timestamp preference: provenance beats mtime; mtime fallback for human skills.
- Empty/missing dirs → empty snapshot, no error.
- `scan_agent_skills` parity test: curator sees identical candidates before/after refactor.

**Acceptance gate:** `just quick-check` green; `just test-crate coco-journey`,
`coco-skills`, `coco-skill-learn`, `coco-memory` green (curator behavior unchanged after
both refactors). ~550 LoC.

---

### WP-A2 — Timeline bucketing (pure functions)

**New files:** `journey/src/timeline.rs` + `timeline.test.rs`.

**Checklist**
1. `bucketize(nodes, max_rows, now_ms)` — day→month→year adaptive granularity; ≤32-day span
   locks to day; local-timezone labels; per-bucket `recency` computed by the private helper.
2. `JourneyStats::busiest_day` computed at day granularity independent of display buckets.

**Tests:** granularity switch boundaries (32/33 days; month overflow → year); single node;
identical timestamps; recency endpoints (single-instant span → ordinal fallback); CJK label
sanity (no control chars; width handling stays in the presentation layer).

**Acceptance gate:** pure-function tests green with injected clock; no `SystemTime` usage
inside the module. ~230 LoC. Shares a PR with WP-A1.

---

### WP-A3 — `/journey` command + wire chain

**Modified files:** `commands/src/lib.rs:162` (payload-less `DialogSpec::Journey`),
`commands/src/implementations.rs` (registration + trivial inline handler — no new handler
file, no new deps), `common/types/src/event.rs` (`TuiOnlyEvent::OpenJourney` +
`JourneyDialogPayload` + node/bucket/stats wire structs per D9 conventions),
`app/query/src/emit.rs:236` (event name), `app/tui/src/server_notification_handler.rs:191`
(event name), `app/cli/src/tui/slash_execution.rs:661` (translation arm: resolve
`JourneyPaths`, `spawn_blocking(build_journey)`, map to wire, emit),
`app/cli/Cargo.toml` (+`coco-journey`).

**Checklist**
1. Wire structs: snake_case fields, `String` paths, node-body enum
   `#[serde(tag = "kind", rename_all = "snake_case")]`, schemars cfg_attr — match the
   surrounding file exactly (verified conventions, D9).
2. Assembly in `spawn_blocking` (blocking dir walks; do NOT repeat the `/memory` handler's
   sync-walk-in-async wart).
3. Every exhaustive match over `DialogSpec` / `TuiOnlyEvent` updated (compiler-driven; no
   wildcard arms added).

**Tests:** wire round-trip serde test alongside existing event tests; translation-arm unit
test (fixture snapshot → payload shape).

**Acceptance gate:** `/journey` reaches the TUI fold point (temporary debug fold acceptable
until WP-A4); `just quick-check` green. ~300 LoC.

---

### WP-A4 — TUI overlay (read-only)

**New files:** `app/tui/src/state/journey.rs` (+ `.test.rs`),
`app/tui/src/modal_pane/journey.rs` (+ `.test.rs`),
`app/tui/src/presentation/journey.rs` (+ `.test.rs`, insta).

**Modified files:** `app/tui/src/state/modal.rs:28,71` (`ModalState::Journey` + priority),
`server_notification_handler/tui_only.rs` (fold → `show_modal`), the **four** keybinding
sites (D5/§4 C5: `keybinding_bridge.rs:19` variant, `:76` active_context arm, `~:344-367`
map_key arm, `keybinding_resolver.rs:212` Global-only stack arm), `update.rs:147`
(intercept hook), `surface_content/mod.rs:124` (styled arm), `modal_pane/mod.rs:179`
(Esc dismissal), `locales/en.yaml` + `zh-CN.yaml`.

**Checklist**
1. `JourneyState` (List/Detail modes in this WP; `DeleteMemoryConfirm` arrives with A6) +
   selection clamp.
2. Key mapping: `j`/`k`/`↑`/`↓`, `Enter`, `Esc`; `e`/`d` stay unmapped until A6 (no dead UI).
3. `journey_lines(state, styles, width) -> (String, Vec<Line<'static>>, Color)`:
   header/legend → bars (span runs, recency-interpolated fg via local RGB endpoint literals +
   `color::rgb()`) → axis → separator → `render_select_list` rows with badges → footer keys.
4. All clipping through `truncate_to_width`; every user-visible string through rust-i18n keys.

**Tests**
- insta snapshots (≥6): list, empty state, detail, narrow (60 col), normal (90), wide (140);
  `locale_test_guard("en")` + `Theme::default()`.
- `modal_pane/journey.test.rs`: key-routing table; Esc from Detail returns to List, from
  List closes.
- `state/journey.test.rs`: clamp on shorter refreshed payload; mode transitions.

**Acceptance gate:** manual run — `/journey` opens, navigation works, badges and quarantine
progress render, resize does not panic (CJK titles in fixtures); snapshots reviewed and
accepted. ~700 LoC.

---

### WP-A5 — Journal substrate

**New files:** `common/types/src/journey.rs` (+ `.test.rs`: serde round-trip per variant),
`maintenance/src/journal.rs` (+ `.test.rs`).

**Modified files:** `common/types/src/lib.rs` (module + re-exports),
`maintenance/Cargo.toml` (**+`serde.workspace = true`** — verified missing),
`skill-learn/src/stamp.rs:20` (emit `SkillLearned`/`SkillUpdated`; it already distinguishes
create vs update at :75-100), `skill-learn/src/curator.rs` (emit `SkillRetired{reason}` /
`SkillPromoted`), `memory/src/service/extract.rs:753-770` (emit `MemoryWritten`),
`memory/src/service/dream.rs:585-602` (emit `MemoryConsolidated`),
`skills/src/agent_scope.rs:52` (`agent_journal_path`), `memory/src/path/resolve.rs`
(`memory_journal_path`), `journey/src/snapshot.rs` (journal join: merge, sort,
`first_seen_ms`/`last_activity_ms` preference order, per-node capped `history`).

**Checklist**
1. `append_jsonl` uses `OpenOptions::append(true).create(true)`; one `write_all` per record
   with trailing `\n`; `rotate_if_over` before append at each write site
   (`JOURNAL_MAX_BYTES = 4 * 1024 * 1024` const at the call sites).
2. Write sites are fire-and-forget additions — no new control flow, no `?` on journal
   calls; async contexts wrap in `spawn_blocking`.
3. Snapshot join prefers journal events over provenance over mtime; orphan events (node
   deleted outside /journey) count toward stats only; per-node `history` capped at 20,
   newest first.

**Tests**
- `journal.test.rs`: append/read round-trip; corrupt-line skip with count; two-thread
  concurrent append → all lines intact; rotation renames and truncates; concurrent rotate
  is benign.
- Extend `skill-learn/tests/loop_e2e.rs`: after the hostile-fork scenario, assert
  (a) `SkillLearned` exists post-stamp, (b) the fence denies the fork writing
  `.agent-journal.jsonl` itself, (c) retire/promote emit events.
- Memory-side: extract/dream service tests assert best-effort emission (journal dir missing
  → no failure).

**Acceptance gate:** e2e green; killing the process mid-append never corrupts more than one
line (reader skips it). ~470 LoC.

---

### WP-A6 — Mutations

**Modified files:** `common/types/src/journey.rs` (`JourneyAction`),
`app/tui/src/command.rs` (`UserCommand::ApplyJourneyAction` — in-process, PathBuf fine),
**new** `memory/src/mutate.rs` (+ `.test.rs`) — `delete_entry` (sync; `parse_memory_index` +
`write_atomic`; NOT in `store/`, which stays a pure data layer),
`app/tui/src/state/journey.rs` (`DeleteMemoryConfirm` mode) + `modal_pane/journey.rs`
(`e`/`d` wiring; skill retire/restore immediate, memory delete confirmed),
`app/cli/src/tui/driver.rs` (action match arm beside `ApplyPermissionUpdate` :1319-1331:
execute → journal event → `spawn_blocking(build_journey)` → re-send `OpenJourney`).

**Checklist**
1. `delete_entry`: remove file (missing = ok) + rewrite `MEMORY.md` dropping lines whose
   link target is the filename; atomic index rewrite; invariant note in module docs
   (user mutation ≠ auto-regeneration).
2. `d` on skill nodes → immediate Retire/Restore by current lifecycle (no confirm, D6);
   on memory nodes → `DeleteMemoryConfirm` (default No) → `DeleteMemory`.
3. `e` → `OpenInEditor` → suspend/editor workflow → refresh on return.
4. Refresh preserves selection by node id (fall back to clamped index).
5. Failed action → status toast, overlay stays open.

**Tests:** `mutate.test.rs` (file+index, idempotency, index-only prune, atomicity); state
tests for confirm flow; snapshot for the DeleteMemoryConfirm frame; manual: retire → restore
round-trip visible in list + journal.

**Acceptance gate:** retire/restore/delete/edit all work end-to-end; journal reflects each
mutation; `MEMORY.md` index has no dangling line after delete. ~430 LoC.

---

### WP-B1 — Notices dual channel (L1)

**New files:** `skill-learn/src/notice.rs` (+ `.test.rs`) — `SkillLearnNotice { name, verb }`,
`SkillLearnInbox` (push/drain, `Arc<Mutex<Vec<_>>>`, memory `NoticeInbox` mirror).

**Modified files:** `common/types/src/attachment_kind.rs` — `SkillLearnedReminder` variant
(**every** exhaustive arm: `as_str` :152 and the classification/override arms
:235/:333/:440/:616), `skill-learn/src/stamp.rs` (push notice on success),
`skill-learn/src/runtime.rs` (expose `drain_notices()`),
`app/query/src/engine_finalize_tail.rs:647-682` (project drained notices →
user-visible `SystemMessage` "Learned skill: X — quarantined until 5 successful uses" +
`<system-reminder>` with the new kind via `history_push_and_emit`).

**Tests:** inbox push/drain; attachment-kind round-trip; engine-side projection unit test
(mock runtime → history receives both messages); a test documenting **next-turn delivery**
(push after drain → surfaces on the following finalize).

**Acceptance gate:** a live review fork that writes a skill produces a visible system line
in the transcript on the next qualifying turn and a reminder attachment. ~250 LoC.

---

### WP-B3 — Description budget (L3)

**Modified files:** `skill-learn/src/stamp.rs` (truncate `description` frontmatter to the
listing budget with `coco_utils_string::truncate_str`; UTF-8-safe),
`skill-learn/src/review.rs:169-218` (one prompt line: "description: one sentence,
≤ 60 characters").

**Tests:** stamp test with an overlong CJK description → truncated at a char boundary.

**Acceptance gate:** `just test-crate coco-skill-learn`. ~60 LoC.

---

### WP-B5 — `SkillLearnConfig` (L5)

**Modified files:** `common/config/src/sections.rs` (`SkillLearnConfig` + `resolve`),
`common/config/src/runtime.rs` (`RuntimeConfig.skill_learn`),
`common/config/src/env.rs` (`CocoSkillLearnDisable`, `CocoSkillLearnReviewThrottle`,
`CocoSkillLearnCuratorDisable` + `as_str` arms),
`skill-learn/src/{runtime,review,curator}.rs` (constants → config fields; constructors take
the config), `app/agent-host/src/session/session_runtime/build.rs:250-262` (pass config).

Keys (defaults = today's constants): `enabled` (true — the Feature stays the coarse gate),
`review_throttle` 5, `review_max_turns` 6, `review_min_tool_calls` 3 (consumed by B4),
`curator_enabled` true, `curator_min_hours` 24, `promote_min_invocations` 5,
`promote_success_rate` 0.8, `retire_failure_rate` 0.34, `retire_inactive_days` 90,
`journal_enabled` true.

**Tests:** `sections.test.rs` resolve matrix (settings/env/override precedence).

**Acceptance gate:** behavior identical at defaults; env overrides observed. ~300 LoC.

---

### WP-B4 — Review signal + cursor + failure backoff (L4)

**Modified files:** `skill-learn/src/runtime.rs` (signal parameter + cursor state +
`consecutive_failures: AtomicI32` + `effective_throttle()`),
`app/query/src/engine_finalize_turn.rs:613-614→:697` (pass the already-computed
`tool_calls_last_turn` into the trigger helper — one-line signature change; the text-tail
site passes 0), `app/query/src/engine_finalize_tail.rs:647-682` (`compute_review_signal`:
tool-call count ≥ config threshold, or SkillTool `tool_use` present in a last-turn history
scan; **task-completion signal deferred** — no per-turn delta on `TaskListHandleRef`,
verified).

**Semantics:** empty signal → `ReviewTrigger::Skipped` at zero cost before the throttle;
cursor advances only on `Completed`; failure shifts the effective throttle
(`<< min(failures, 5)`).

**Tests:** `runtime.test.rs` extensions — signal gating, cursor advance/hold, backoff
ceiling, coalescing unchanged.

**Acceptance gate:** with no signal, no fork fires even at the throttle boundary; a failing
spawn visibly stretches the next fire. ~300 LoC.

---

### WP-B2 — User-initiated `/learn` (L2)

**New files:** none (handler is small enough for `implementations.rs`; extract to
`commands/src/handlers/learn.rs` only if it outgrows a screen).

**Modified files:** `skills/src/lib.rs` (`SkillAuthor::Manual` + `as_str` arm — closed serde
kebab-case enum; no compat concern), `skill-learn/src/runtime.rs`
(`manual_review(directive, ...)` — bypass throttle, respect single-flight),
`skill-learn/src/review.rs` (directive injection into the prompt; `created-by: manual`
passed to stamp), `commands/src/lib.rs` (`CommandResult::TriggerSkillLearn { directive }`),
`app/agent-host/.../session_handle/capabilities.rs` (**new public
`skill_review_runtime()` accessor** — mirrors `memory_runtime()` at :85; the field is
currently `pub(in crate::session::session_runtime)`, `resources/handles.rs:212-213`),
`app/cli/src/tui/slash_execution.rs` (match the typed variant; snapshot history via
`SessionHandle::history_messages()`, `session_handle/history.rs:45`; call `manual_review`),
registration in `implementations.rs` behind `Feature::SkillLearning`.

**Tests:** runtime test (manual bypasses throttle, blocked by in-flight review);
review prompt contains the directive verbatim; stamp writes `created-by: manual`;
handler registration/availability test.

**Acceptance gate:** `/learn how we fixed the nextest filter` fires a fork immediately,
produces a quarantined skill stamped `manual`, journal records it with the session id,
and (with B1) the transcript announces it on the next turn. ~350 LoC.

---

### WP-B6 — `/skills` quarantine section (L6)

**Modified files:** the `/skills` dialog payload builder (agent-host/commands side) — group
`Learning` skills into a dedicated section with `success_count/5` progress; TUI skills
dialog renderer + snapshots; share the progress-format helper with
`presentation/journey.rs` (single definition — house rule: no duplicated single-use
helpers).

**Acceptance gate:** snapshot-reviewed section; progress matches telemetry fixture.
~200 LoC.

---

### 8.2 Rust-practice checklist (applies to every WP)

- Companion tests only: `#[cfg(test)] #[path = "x.test.rs"] mod tests;` — never inline.
- Zero `.unwrap()`/`.expect()` outside tests (workspace clippy denies); proven invariants
  use `match`/`let-else` + `panic!` with a message.
- Closed sets are enums with `as_str()`; no bare strings for discriminators; exhaustive
  matches without wildcard arms so the compiler drives every new-variant update.
- No bool parameters that produce opaque call sites — two-variant enums instead
  (`IncludeDisabled::Yes`), or `/*param*/` argument comments where unavoidable.
- Illegal states unrepresentable: kind-specific data lives in enum payloads
  (`JourneyNodeBody`), not `Option` fields with prose invariants.
- In-process vs wire types kept distinct (D9): `PathBuf` in-process, `String` + snake_case +
  `tag = "kind"` on the wire.
- UTF-8 safety: all computed byte cuts through `coco_utils_string`; all terminal-column
  clipping through `truncate_to_width`.
- Blocking I/O in async contexts goes through `tokio::task::spawn_blocking` — including the
  journey assembly and journal reads (do not copy the `/memory` handler's sync-walk wart).
- `format!("{name}")` inline captures; collapsed `if`; method references over closures.
- Serde: `#[serde(default)]` for optional config fields; `rename_all = "snake_case"` on
  enums; typed structs over `serde_json::Value` for payloads produced and consumed in-repo.
- No cross-layer errors as anyhow: journey/maintenance per §6; anyhow only at Tier-1.
- Modules stay under ~800 LoC; presentation/state/modal_pane split keeps each file small.
- i18n: every user-visible string in `locales/*.yaml`; snapshot tests pin `en`.
- Iteration loop: `just fmt` after edits, `just quick-check` per iteration,
  `just pre-commit` exactly once per PR before commit.

---

## 9. Risks & Open Questions

1. **R1 — Pre-journal history distortion.** Bootstrap timelines reconstruct from
   provenance/mtime; dream-rewritten memories look "recent". **Accepted** (Hermes-grade
   fidelity on day one); the journal makes all *new* history accurate, and the distortion
   dilutes over time. Rejected alternative: teaching the dream prompt to preserve
   timestamps — prompt-enforced invariants are unreliable and rename/merge is inherently
   lossy.
2. **R2 — Memory scanner 200-file cap** (`scan.rs:15`). Journey needs the full set.
   Fixed by the breaking signature change in WP-A1 (`max_files` parameter; journey passes
   2000 + traces overflow).
3. **R3 — Journal growth.** Low-frequency events (< 300 B/line); 4 MiB ≈ 15k events;
   single-generation rotation suffices. **Accepted.**
4. **R4 — Cross-process concurrent appends.** O_APPEND offset positioning is atomic;
   small local-fs writes do not interleave in practice (no hard guarantee; NFS can break
   it) — a pathological interleave produces one skippable bad line. Best-effort
   observability — **accepted** (same argument as telemetry lost-increments,
   design-03 §risk 5).
5. **R5 — Journal/disk reconciliation.** A node whose file was deleted outside `/journey`
   still has journal events → the snapshot is disk-authoritative (orphan events feed stats
   only); no GC. **Accepted.**
6. **R6 — Animated reveal.** Requires making `ui_animation()` modal-aware
   (`state/mod.rs:284-304`, gate at :297). v1 is static. **Open** (P2 polish).
   **R6b — Resize staleness:** buckets are computed at open with nominal rows; a terminal
   resize re-scales bar widths but does not re-bucket rows until the overlay is reopened.
   **Accepted** for v1.
7. **R7 — Headless `coco journey`.** Snapshot/bucketing are pure, so a CLI subcommand is
   cheap later. **Open.**
8. **R8 — Graph edges.** Hermes has `related_skills` + lexical memory↔skill edges; coco has
   no such frontmatter. `JourneySnapshot` can grow an `edges` field if the frontmatter is
   ever added. **Open.**
9. **R9 — Multi-project view.** Journey shows the current project's memdir + global skills.
   Cross-project aggregation (walking `projects/*/memory-journal.jsonl`) belongs to an
   Event Hub consumer. **Open.**

---

## 10. File Change Manifest (absolute paths, implementation reference)

New:
- `coco-rs/journey/` (new crate: `snapshot.rs`, `timeline.rs`, companion tests) — consumed
  by `app/cli` only
- `coco-rs/common/types/src/journey.rs` (`JourneyRecord`/`JourneyEvent`/`SkillRetireReason`/
  `JourneyNodeId`/`JourneyAction`)
- `coco-rs/maintenance/src/journal.rs` (append/read/rotate primitives)
- `coco-rs/memory/src/mutate.rs` (`delete_entry`)
- `coco-rs/skill-learn/src/notice.rs`
- `coco-rs/app/tui/src/state/journey.rs` · `modal_pane/journey.rs` · `presentation/journey.rs`

Modified (one arm/section each):
- `coco-rs/skill-learn/src/stamp.rs:20` (journal + notice + description budget) ·
  `curator.rs:237-245` (retire → `set_skill_disabled`, scan → `scan_agent_skills`,
  + journal) · `runtime.rs` (`manual_review`, signal/cursor/backoff, config) ·
  `review.rs` (directive injection, prompt line)
- `coco-rs/memory/src/service/extract.rs:753-770` · `dream.rs:585-602` (journal) ·
  `scan.rs:15,35` (cap parameter) · `path/resolve.rs` (`memory_journal_path`)
- `coco-rs/skills/src/agent_scope.rs:52` (`agent_journal_path`, `scan_agent_skills`) ·
  `lib.rs` (`set_skill_disabled`, `SkillAuthor::Manual`)
- `coco-rs/maintenance/Cargo.toml` (+serde)
- `coco-rs/commands/src/lib.rs:162` (DialogSpec, `TriggerSkillLearn`) ·
  `implementations.rs` (registrations; inline thin handlers)
- `coco-rs/common/types/src/event.rs` (`TuiOnlyEvent::OpenJourney`, wire payloads) ·
  `attachment_kind.rs` (`SkillLearnedReminder`, all arms)
- `coco-rs/common/config/src/sections.rs` · `runtime.rs` · `env.rs` (`SkillLearnConfig`)
- `coco-rs/app/query/src/emit.rs:236` · `engine_finalize_tail.rs:647-682` ·
  `engine_finalize_turn.rs:613-614→:697` (signal pass-through)
- `coco-rs/app/cli/src/tui/slash_execution.rs:661` (assembly + translation) ·
  `driver.rs` (journey-action arm beside :1319-1331) · `app/cli/Cargo.toml` (+coco-journey)
- `coco-rs/app/agent-host/src/session/session_runtime/build.rs:250-262` ·
  `session_handle/capabilities.rs` (`skill_review_runtime()` accessor)
- `coco-rs/app/tui/src/command.rs` (`ApplyJourneyAction`) ·
  `server_notification_handler.rs:191` · `server_notification_handler/tui_only.rs` ·
  `state/modal.rs:28,71` · `keybinding_bridge.rs:19,76,~344-367` ·
  `keybinding_resolver.rs:212` · `update.rs:147` · `surface_content/mod.rs:124` ·
  `modal_pane/mod.rs:179` · `locales/{en,zh-CN}.yaml`

---

## 11. Adversarial Review Log (2026-07-16)

The draft was reviewed adversarially: two verification passes against the live tree
(30-item citation audit: 28 confirmed, 2 corrected; 12-item integration-seam
verification) plus an architecture/logic critique. Every accepted finding is already
integrated into the body above. Recorded here so the reasoning survives:

| # | Severity | Finding | Resolution |
|---|---|---|---|
| RV1 | High | `commands` → `coco-journey` would drag `coco-memory`'s heavy transitive tree (`tool-runtime`, `shell`, `sandbox`, `apply-patch`, …) into every `commands` consumer; `commands` does not depend on `coco-memory` today | Thin payload-less `DialogSpec::Journey`; assembly moved to the `app/cli` translation site (D2, §4 C4) |
| RV2 | High | `parse_skill_markdown` is **private** (`skills/src/lib.rs:1131`) — the draft's "journey parses `.agent/` itself" was unimplementable as written | New shared `agent_scope::scan_agent_skills` API; curator refactored onto it (D7) |
| RV3 | High | `JourneyNodeStatus` conflated kind with lifecycle (`Memory`/`UserAuthored` beside `Learning/Learned/Retired`) and `telemetry: Option<_>` carried a prose invariant — illegal states representable | Restructured to `JourneyNodeBody` enum payload + `AgentSkillLifecycle` (D8, §4 C2) |
| RV4 | High | Memory fences verified: `allowed_write_roots = [memory_dir]` + `.md`-only inner ring — confirms journal siblings are fork-unwritable, but also means journal writes **must** be host-side | Stated explicitly; write sites are all post-fork service code (§4 C1) |
| RV5 | Medium | Notices from a detached fork can only drain at a **later** turn's finalize (verified against memory: schedule `runtime.rs:1395-1397` → drain `:862`); draft implied same-turn | Latency documented in L1 + next-turn test required in WP-B1 |
| RV6 | Medium | `SessionHandle` has no public `SkillReviewRuntime` accessor (field is `pub(in …)`, `resources/handles.rs:212-213`); `/learn` routing as drafted didn't compile conceptually | New `skill_review_runtime()` accessor (mirrors `memory_runtime()` :85); history snapshot via `history_messages()` — no CommandQueue detour (L2, WP-B2) |
| RV7 | Medium | B4's "task item completed" signal has no per-turn seam (`TaskListHandleRef` is async CRUD only); "skill invoked" flag not on the engine | Signal v1 = `tool_calls_last_turn` pass-through (already computed at `engine_finalize_turn.rs:613-614`) + last-turn history scan; task signal deferred (L4, WP-B4) |
| RV8 | Medium | memory `store/` is a pure no-I/O layer and MEMORY.md has a documented "never auto-regenerate" invariant — draft put async `delete_entry` "in the store" | Sync `delete_entry` in new `memory/src/mutate.rs`; invariant boundary documented (user mutation ≠ auto-regeneration) (§4 C6) |
| RV9 | Medium | `SkillTelemetrySummary` referenced but never defined | Dropped; `coco_skills::SkillTelemetryStats` embedded verbatim (§4 C2) |
| RV10 | Medium | Full `records: Vec<JourneyRecord>` on snapshot + wire = unbounded payload for a ~15k-event journal | Per-node `history` capped at 20; raw records never cross the wire (§4 C2) |
| RV11 | Medium | O_APPEND "atomic < 4 KiB" overclaim (PIPE_BUF is a pipe guarantee, not regular files) | Reworded honestly; reader-skip absorbs residual risk (§4 C0, R4) |
| RV12 | Medium | `maintenance` has `serde_json` but **not** `serde` — the `T: Serialize` bound wouldn't resolve | `serde.workspace = true` added in WP-A5 |
| RV13 | Low | Two `KeybindingContext` enums exist; adding a TUI overlay context = 4 exhaustive TUI-local sites, keybindings crate only if user-rebindable | Four-site checklist + Global-only stack (PermissionsEditor precedent) (§4 C5, WP-A4) |
| RV14 | Low | `UserCommand` is in-process (no serde) so PathBuf is fine there; wire payloads use String paths + snake_case + `tag="kind"` (verified conventions) | D9 split codified; wire structs specified accordingly |
| RV15 | Low | Confirm-on-retire punished a reversible action while the truly irreversible one (memory delete) was the real risk | Skill retire/restore immediate; only `DeleteMemoryConfirm` remains (D6) |
| RV16 | Low | `compute_recency(nodes) -> Vec<f32>` parallel-array public API was index-coupled for no benefit | Privatized into `timeline.rs`; buckets carry `recency` (§4 C3) |
| RV17 | Low | Inconsistent journal naming (`.agent-journal.jsonl` vs `journey.jsonl`); blocking I/O in async command handlers (the `/memory` handler already has this wart — verified sync walk in async fn); resize staleness and rotate race unstated; `JourneyStats` undefined; TUI event-name mapping line drifted (:212 → :191) | Renamed `memory-journal.jsonl`; `spawn_blocking` mandated at assembly/read sites; R6b + rotate note added; `JourneyStats` defined; line refs corrected |

---

> [← Design #1: Skill Learning Loop](design-01-skill-learning-loop.md) · [Index](README.md)
