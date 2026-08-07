# B1 — World State: verification, review, and coco design

> **Status: implemented** (feat/optimize, `just pre-commit` green).
> `coco_types::WorldStateSnapshot` replaces the five in-memory announce
> baselines, persists to the session JSONL as
> `MetadataEntry::WorldStateSnapshot`, and is restored at session build.
> `AttachmentType::ModelSwitch` + `ModelSwitchGenerator` close C2. Net −134
> lines of Rust. **Not** implemented: the `injected_memory_files` section for
> C3 (`FileReadState` persistence) — see §5.6 Deferred.

Companion to [codex-comparison-2026-08.md](codex-comparison-2026-08.md) §2.1.
Reference implementation: `codex-rs/core/src/context/world_state/` @ `4ee4192`.

This revision **re-verified every claim in the first draft against the coco
tree**. Two claims were wrong and are retracted, one was materially understated,
and one new instance of the same bug class turned up. The design section is
rewritten: no feature flag, no backward compatibility, and — after review —
substantially *simpler* than codex's, because coco has typed attachment kinds
where codex only has text.

---

## Part 1 — Claim verification

### ✅ C1. Resume re-announces all four deltas — CONFIRMED

Chain of evidence:

| Link | Evidence |
|---|---|
| Attachments *are* persisted, with their kind | `app/session/src/storage.rs:97` `entry_kind::ATTACHMENT`; write side `storage/wire.rs:67`, read side `storage/wire.rs:327` deserializes the full `AttachmentMessage` (uuid + kind + body) |
| …and restored into history on resume | `storage/wire.rs:334-336` → `Message::Attachment(att)`; `core/messages/src/resume.rs::sanitize_messages_for_resume` only filters unresolved tool-uses / orphan thinking / whitespace — no announce-state reconciliation |
| The baseline is **not** persisted | `ToolAppState` (`common/types/src/app_state.rs:137`) derives `Debug, Clone, Default` — no `Serialize`/`Deserialize`; grep for `last_announced` in `app/session/src` returns nothing |
| The baseline is **never** seeded from history | Only two write sites exist: `app/query/src/engine_turn_reminders.rs:798-817` (after emission) and `engine_compaction.rs:1170-1177` (after compaction). No resume path. |

So the first turn of a resumed session computes `compute_*_delta(current, ∅)`,
every entry looks added, and all four announcements are re-emitted on top of a
history that already contains them. `is_initial` also flips back to `true`, so
the agent listing re-frames as "Available agent types" instead of "New agent
types are now available".

Worst offender is `mcp_instructions_delta`: its baseline is
`HashMap<String, String>` of *full instruction text* per server, so an empty
baseline re-emits every server's complete instruction block.

### ✅ C2. Model switch is silent — CONFIRMED, and understated

Stronger than the first draft claimed. `/model` mid-session goes
`handle_set_model` → `session_controls::set_model`
(`app/agent-host/src/session/session_controls/model.rs:27-39`) →
`SessionRuntime::set_model_id`
(`session_runtime/state/engine_config.rs:51-58`), which mutates
`engine_config.model_id` **and nothing else**.

The system prompt is built once, at engine construction
(`app/query/src/engine_builder.rs:467`,
`app/agent-host/src/session/session_runtime/build.rs:372`) — there is no
per-turn rebuild anywhere in the query loop. And `core/context/src/prompt.rs:363`
renders:

```rust
if !env.model.is_empty() {
    s.push_str(&format!("You are powered by the model {}.\n", env.model));
}
```

So after `/model`, the prompt keeps asserting the **old** model (and the old
knowledge-cutoff line) for the remainder of the session. There is no
model-switch reminder either — grep for `model_changed` / `previous_model` /
`last_model` across `core/system-reminder`, `app/query`, and `core/context`
returns nothing.

This is not "we fail to notify"; it is "we actively state something false".

### 🆕 C3. `FileReadState` has the same defect — NEW FINDING

`common/types/src/file_read_state.rs:110` derives `Debug, Default, Clone` — no
serde — and grep for it in `app/session/src` returns nothing. It is not
persisted.

`FileReadState` is **gate 2** of the nested-memory dedup
(`app/query/src/engine_attachments.rs:150-164`, whose own comment says
"`loaded_nested_memory_paths` is rebuilt per prompt cycle and can't suppress a
CLAUDE.md shown on an earlier prompt; FileReadState survives the rebuild"). It
survives the *prompt-cycle* rebuild, but not process restart. After resume both
gates are open, so nested `CLAUDE.md` files already injected into the reloaded
history are injected again. The `already_read_file` reminder
(`AttachmentType::AlreadyReadFile`) is blind for the same reason.

**The pattern is systemic.** coco has at least three separate "what has the
model already been shown" caches, all in-memory only, all reset on resume, all
re-injecting content that is already in the reloaded history. That reframes B1:
the point is not "port codex's WorldState", it is **"coco has no persisted
model-visible-state baseline"** — and codex's World State is the proven shape
for one.

### ❌ C4. "Four generators reconstruct previous state by scanning history text" — RETRACTED

Wrong. The `agent_listing_delta.rs` module doc says this, but it is stale. The
real implementation
(`app/query/src/engine_helpers.rs::compute_{tools,agents,mcp_instructions,mcp_servers}_delta`)
diffs against typed `ToolAppState.last_announced_*` state. No text scanning
anywhere.

*(That stale doc comment is itself worth fixing.)*

### ❌ C5. "Compaction drops the announcement" — RETRACTED

Wrong, and backwards. `engine_compaction.rs:1119-1153` gates each baseline on
`preserved_contains_attachment_kind(preserved_history, AttachmentKind::X)` —
per attachment kind, based on whether *that specific* announcement survived the
compaction. codex blanket-resets its entire baseline on
`RolloutItem::Compacted`. **coco's handling is strictly better and must be kept**
— see the design below, where it replaces four of codex's mechanisms.

### ⚠️ C6. "Permission-mode changes are silent" — TRUE but DROPPED from scope

Verified: `plan_mode_reminder.rs::reconcile_mode_transition` only detects
Plan↔non-Plan and Auto↔non-Auto. `default` → `acceptEdits` →
`bypassPermissions` transitions produce nothing, and there is no
`<permissions instructions>` block in coco's prompt at all.

But this is a **codex-specific need, not a coco gap**. codex tells the model its
permission state because the codex model is expected to pass
`sandbox_permissions` on exec calls and decide whether asking is worthwhile.
coco enforces permissions at the tool-orchestration layer and prompts the user;
the model's behaviour does not depend on knowing the mode. The two modes that
*do* change model behaviour — Plan and Auto — are already covered. Importing
this would add tokens for nothing, and announcing "you are now in
bypassPermissions" is arguably harmful.

**Dropped.** No permissions section.

### ⚠️ C7. CLAUDE.md replacement / removal notices — DOWNGRADED

Real, but low value for coco. Eager memory lands in the system prompt, which is
never rebuilt, so the "these instructions replace the previous ones" case cannot
arise from a mid-session edit — the prompt simply never changes. The genuinely
useful half is the *lazy* path (`nested_memory`), and that is already covered by
fixing C3.

**Reduced to:** a `memory_files` section that records which nested memory files
have been injected, replacing the unpersisted `FileReadState` gate.

---

## Part 2 — Review of the proposed remediation

The first draft proposed a near-transliteration of codex. Reviewing it against
coco's constraints and the repo's own conventions, four pieces should go.

### R1. Drop `PreviousSectionState::Unknown` and all four text-matching hooks

codex needs `Unknown`, `matches_legacy_fragment`,
`has_retained_fragment_matcher`, and `matches_retained_fragment` because its
only handle on "is this fragment still in history?" is **matching rendered
text**. That drives the marker machinery, the `WorldStateHash` fingerprints, and
`has_legacy_fragment` / `has_retained_fragment` scans over every history item.

coco does not have that problem. `Message::Attachment` carries a typed
`AttachmentKind`, and `preserved_contains_attachment_kind` already answers the
question exactly, in one pass, with no string comparison. Keep coco's mechanism
and the whole text-matching layer disappears.

`PreviousSectionState` collapses to `Option<&Snapshot>`.

### R2. Drop the `ContextualUserFragment` marker extension (former item A2)

A2 was proposed as B1's prerequisite. It is not — markers exist in codex *only*
to serve R1's text matching. With R1 gone, `markers()` / `type_markers()` /
`matches_text()` / `WorldStateHash::from_fragment` have no consumer.

Removing them also makes the fragment trait object-safe without
`where Self: Sized` escapes, which is the reason codex's trait is awkward.

**A2 should be struck from the comparison doc's borrow list.**

### R3. Drop `serde_json::Value` erasure and RFC-7386 merge patches

codex's `ErasedWorldStateSection` + `IndexMap<&'static str, Box<dyn …>>` +
`WorldStateSnapshot(BTreeMap<String, Value>)` + `create_merge_patch` /
`apply_merge_patch_value` / `remove_null_object_fields` exist to support
**extension-contributed sections** — plugins adding sections core has never
heard of. That forces type erasure, which forces `Value`, which forces merge
patches to keep the persisted delta small, which forces the replay-ordering
hazard (`"ignored world-state patch without a full snapshot"`).

coco is not taking extension sections (see "What not to copy" below), so none of
that is load-bearing. And CLAUDE.md is explicit: *"Typed structs over
`serde_json::Value` when the payload is both produced and consumed inside
coco-rs."* This payload is.

Replace all of it with a **typed snapshot struct**, and persist the full
snapshot whenever it changes rather than a patch. That is sound only if
snapshots stay small — which codex's own principle already guarantees
("`Snapshot` should contain only the comparison data needed"), and which we
enforce by storing a **content hash** for anything large (MCP instruction
bodies, memory-file contents) instead of the text.

Result: ~250 lines of merge-patch machinery, the erasure layer, the replay
ordering hazard, and the `Value` dependency all vanish.

### R4. Drop the trait and the registry; make the compiler enforce completeness

With a typed snapshot struct there is no reason to dyn-dispatch sections.
`WorldStateSection` would be a trait with exactly one implementor per section
and no polymorphic call site — the "no single-use helpers" rule applied to
traits.

But a straight-line assembly function has a real failure mode: add a field to
the snapshot struct, forget to populate it in assembly, and it stays `None`
forever — silently disabling that section.

coco already has the tool for this. `scripts/check-live-fields.sh` (HEAD commit
`94d9f054`) fails the build on struct fields nothing consumes, with the rule for
`Serialize` structs being *"dead when nothing **constructs** it — its reader is
an off-process JSON consumer, so reads prove nothing"*. That is precisely this
struct. Add `WorldStateSnapshot` to the scanned list, build the struct with an
explicit literal (never `..Default::default()`), and a section added to the
snapshot but not to assembly **fails `just quick-check`**.

Compiler-and-CI-enforced completeness beats a runtime registry.

### R5. Placement — the first draft's layering was wrong

The draft put everything in `core/context`. But the section inputs
(`McpServerSummary`, `AgentListingDeltaInfo`, `McpServersDeltaInfo`) live in
`core/system-reminder`, which **depends on** `core/context`
(`core/system-reminder/Cargo.toml:17`). Sections referencing those types cannot
live below them.

Corrected split, mirroring the existing `core/goals` (pure) / `core/goal-runtime`
(host) pattern:

| Layer | Contents |
|---|---|
| `core/context/src/world_state.rs` | `WorldStateSnapshot` + `WorldStateDelta` — pure data, no domain deps |
| `core/system-reminder/src/world_state/` | per-section `render` functions + the assembly function; they already own the input DTOs |
| `app/query` | drives assembly, emits, commits the baseline |
| `app/session` | persists / replays the snapshot |

### R6. No feature flag, no legacy path (per instruction)

Direct cutover. The four `compute_*_delta` functions, the `last_announced_*`
fields (**including the dead unscoped `last_announced_tools`, superseded by
`last_announced_tools_by_scope`**), the `preserved_contains_attachment_kind`
call sites, the `GeneratorContext` delta fields and builder methods, and the
four delta generators are deleted in the same change.

Session files written before the change carry no world-state record → baseline
is `None` → one re-announcement on the first resume after upgrade. Acceptable,
and it deletes the entire `Unknown`/legacy-matching branch that would otherwise
be needed to avoid it.

---

## Part 3 — Revised design

### 3.1 Data

`core/context/src/world_state.rs` — pure, no domain dependencies:

```rust
/// Everything the model has been told that is not part of the conversation.
///
/// Fields hold *comparison* data only: anything whose full text would be large
/// is stored as a content hash, so one snapshot stays a few hundred bytes and
/// can be written whole on every change.
///
/// Listed in `scripts/check-live-fields.sh`: a field added here that assembly
/// never constructs fails the build.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStateSnapshot {
    /// Model slug. Identity only — the instructions text is not stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Wire-names announced as deferred, per agent scope.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub deferred_tools: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub agent_types: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_servers: BTreeMap<String, McpServerAnnouncementState>,
    /// server name → SHA-256 of its instructions. Bodies are never stored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mcp_instruction_digests: BTreeMap<String, ContentDigest>,
    /// Nested memory files already injected — replaces the unpersisted
    /// `FileReadState` gate (C3).
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub injected_memory_files: BTreeSet<PathBuf>,
}
```

Ordered collections (`BTreeSet`/`BTreeMap`) throughout, so `PartialEq` is the
change test and the serialized form is byte-stable — which makes the snapshot
tests diffable and lets "did anything change?" be one `==`.

### 3.2 Sections

`core/system-reminder/src/world_state/`, one file per section, each a plain
function with the same shape:

```rust
pub(crate) fn render(
    current: &McpServersInput,
    previous: Option<&BTreeMap<String, McpServerAnnouncementState>>,
) -> Option<SystemReminder>;
```

`previous: None` means "never told" — first turn, or the section's announcement
did not survive compaction. There is no third state.

Assembly returns the fragments *and* the new snapshot from the same pass, so
they cannot disagree:

```rust
pub struct WorldStateDelta {
    pub reminders: Vec<SystemReminder>,
    pub snapshot: WorldStateSnapshot,
}

pub fn render_world_state(
    input: &WorldStateInput<'_>,
    previous: Option<&WorldStateSnapshot>,
) -> WorldStateDelta {
    let mut reminders = Vec::new();
    // Explicit literal — no `..Default::default()`, so a new field must be
    // filled here or `check-live-fields` fails.
    let snapshot = WorldStateSnapshot {
        model: model::render(input, previous, &mut reminders),
        deferred_tools: deferred_tools::render(input, previous, &mut reminders),
        agent_types: agent_types::render(input, previous, &mut reminders),
        mcp_servers: mcp_servers::render(input, previous, &mut reminders),
        mcp_instruction_digests: mcp_instructions::render(input, previous, &mut reminders),
        injected_memory_files: memory_files::render(input, previous, &mut reminders),
    };
    WorldStateDelta { reminders, snapshot }
}
```

### 3.3 Compaction — keep coco's typed check, generalise it

Not codex's blanket reset, and not a text scan. One mapping from snapshot field
to the `AttachmentKind` that carries it, applied to the preserved history:

```rust
/// Clear each field whose announcement did not survive compaction, so the next
/// turn re-announces exactly what was lost and nothing else.
pub fn retain_surviving(
    snapshot: &mut WorldStateSnapshot,
    preserved: &[Message],
) {
    if !contains_kind(preserved, AttachmentKind::McpServersDelta) {
        snapshot.mcp_servers.clear();
    }
    // … one arm per field
}
```

This is the same information codex gets from `matches_retained_fragment`, minus
the string matching — and it is per field rather than all-or-nothing.

### 3.4 Persistence

`app/session/src/storage.rs` already has the right slot. `MetadataEntry` is a
`#[serde(tag = "type", rename_all = "kebab-case")]` enum, and
`MetadataEntry::FileHistorySnapshot` is precedent for exactly this shape,
including its rationale:

> The `snapshot` payload is a passthrough JSON blob to keep `coco-session` free
> of a `coco-context` dependency — `coco-context::FileHistorySnapshot` owns the
> typed shape and (de)serializes through this Value.

Follow it verbatim:

```rust
/// Model-visible world state as of the entry that precedes it. Replayed on
/// resume to seed the baseline so a resumed session does not re-announce
/// what the restored history already contains.
WorldStateSnapshot {
    session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_id: Option<String>,
    snapshot: serde_json::Value,
},
```

`agent_id` scopes it, matching the existing `*_by_scope` maps (and the
`agent_id` field already on `TranscriptEntry`). Write ordering mirrors codex's
and for the same reason: **append the reminders to history first, then the
snapshot record.** A crash between the two can then only cause a
re-announcement, never a silent omission.

Replay is last-record-wins per `agent_id` — no patch application, no ordering
hazard, no `"ignored patch without a full snapshot"` failure mode.

### 3.5 Runtime ownership

The baseline lives on `QueryEngine` beside the other per-session caches and is
committed in the block that already does this bookkeeping —
`engine_turn_reminders.rs:769-820`, where the four
`if fired_types.contains(&ReminderAttachmentType::X)` arms become one:

```rust
if !delta.reminders.is_empty() {
    self.world_state_baseline.store(delta.snapshot.clone());
    self.session.append_world_state_snapshot(&delta.snapshot).await;
}
```

### 3.6 Deferred: `FileReadState` (C3)

**Not implemented.** The plan called for an `injected_memory_files` section
replacing the unpersisted `FileReadState` gate for nested memory. It was left
out deliberately: `FileReadState` is *gate 2* of a two-gate dedup
(`engine_attachments.rs:150-164`) that also drives the `already_read_file`
reminder and the `Read`-tool dedup, each with its own LRU semantics. Persisting
only the memory-file half changes nested-memory injection behaviour without
covering the sibling consumers, and the blast radius is a different subsystem
from the one this change touches.

C3 is real and still open. The right shape is its own change: decide whether
`FileReadState` becomes durable as a whole, or whether only the
memory-injection ledger does.

### 3.7 Testing

Per-section transition tables, codex's best idea, kept:

```rust
// core/system-reminder/src/world_state/mcp_servers.test.rs
#[test]
fn transitions() {
    insta::assert_snapshot!(render_cases(&[
        (None,            &[]),                    // → None
        (None,            &[github()]),            // → initial announce
        (Some(&[github()]), &[github()]),          // → None
        (Some(&[github()]), &[github(), linear()]),// → added only
        (Some(&[github()]), &[]),                  // → removed
    ]));
}
```

One snapshot per section renders the whole matrix including the `None` cells, so
"does this change cause a re-announcement?" is visible in a diff.

Two integration tests carry the actual point:

- **resume**: run turns → persist → reload → assert the next turn emits **zero**
  world-state reminders. This is the C1 regression test and it does not exist today.
- **model switch**: turn on model A, `set_model_id`, turn on model B → assert
  exactly one model reminder, and that the `<env>` block agrees.

### 3.8 Effort

| Step | Effort |
|---|---|
| `WorldStateSnapshot` + assembly + `check-live-fields` entry | S |
| Six section functions + transition snapshots | M (~2 d) |
| `MetadataEntry::WorldStateSnapshot` + replay + resume seeding | M (~1.5 d) |
| Cut over `app/query`; delete `compute_*_delta`, `last_announced_*`, the four generators, the `GeneratorContext` fields | M (~1.5 d) |
| New sections: model, memory files | S (~1 d) |
| Integration tests, docs, `just pre-commit` | S |

≈ **6 days**, down from 8–10: R1–R4 removed the merge-patch machinery, the
erasure layer, the marker trait extension, and the legacy-matching branch.

---

## Part 4 — What not to copy

- **Extension-contributed sections** (`WorldStateSectionContribution`). The sole
  reason codex needs type erasure and `Value`. coco's plugin surface has no
  context-contributor concept; adding one to serve a hypothetical is what forces
  the whole dynamic design. Closed struct.
- **`should_persist()`**. Exists only so `TokenBudgetContext` can render without
  polluting rollouts. In coco those stay ordinary system-reminder generators —
  which is what they are.
- **Merge patches / `WorldStateHash` / marker matching** — R1–R3.
- **A permissions section** — C6.
- **codex's blanket compaction reset** — C5; coco's per-kind check is better.
- **Migrating per-turn nudges onto World State.** Todo reminders, plan-mode
  cadence, ultrathink, token usage are *not* world state. The line:
  *"a session fact the model should know until it changes"* → World State;
  *"a prompt we re-issue on a schedule"* → system-reminder generator. The
  existing tier/cadence architecture is better than codex's and is not touched.
