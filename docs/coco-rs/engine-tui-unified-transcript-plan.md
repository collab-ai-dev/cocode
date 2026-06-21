# Engine ↔ TUI Unified Transcript — Design + Retrospective

Status: **Shipped, with F3/F4/F7/F10/F11/F13/ErrorB hardening (2026-05-20).**
Date: 2026-05-20. Supersedes `engine-tui-message-bridge-plan.md` and
`engine-tui-transcript-economical-plan.md` (both deleted).

## Latest hardening (2026-05-20)

- **Error B (Rewind unified)**: `UserCommand::AutoTruncate` removed.
  `UserCommand::Rewind { message_id, mode: RewindMode }` is the single
  command. `RewindMode` is an ADT (`Explicit { restore_type, rewound_turn }`
  | `AutoRestore`); the auto-restore variant structurally cannot carry
  a `RestoreType`, so "auto-restore never touches files" is enforced by
  the type system.
- **F10 (bulk-replace unified)**: `history_replace_and_emit` now emits
  one `ServerNotification::HistoryReplaced { messages }` instead of
  `MessageTruncated{0} + N×MessageAppended`. Compaction and resume use
  the same wire shape; SDK consumers see one event sequence regardless
  of trigger.
- **F11 (handler signature)**: `handle_core_event` takes
  `&Sender<UserCommand>`. The `pending_auto_restore_truncate` /
  `pending_system_pushes` fields and their drain helpers are deleted.
  TUI handlers `try_send` directly — no two-step dance, no microrace.
- **F3 (ReasoningMetadataAttached)**: New event
  `ServerNotification::ReasoningMetadataAttached { message_uuid,
  duration_ms, reasoning_tokens }` emitted by the engine alongside
  `TurnCompleted`. TUI side-cache anchors by UUID instead of walking
  cells. The prior "I-2 exception" note in tui/CLAUDE.md is gone.
- **F4 (constant pin)**: Regression test in
  `core/messages/src/creation.test.rs` pins
  `INTERRUPT_MESSAGE{,_FOR_TOOL_USE}` to the verbatim TS literals
  (`utils/messages.ts:207-208`).
- **F7 (doc drift)**: `common/types/CLAUDE.md` `Message` variant list
  corrected from 8 to 7 (no `ToolUseSummaryMessage`).
- **F13 (TS audit)**: Confirmed TS Message union has 6 variants
  (`user`/`assistant`/`system`/`attachment`/`grouped_tool_use`/
  `collapsed_read_search`). The 5 originally-proposed new
  `SystemMessage` variants are TS-aligned NOT to add: hook outputs are
  `Attachment` subtypes; plan markers are `UserMessage` with `isMeta`;
  teammate notifications live in the parallel `TeammateMailbox`
  stream. Decision recorded.



This doc owns the cross-layer design that removes the engine/TUI
message-representation split and converges cancel-marker / resume /
auto-restore on engine authority. It is now a **design + retrospective**:
each section records what shipped and how it differs from the original
plan when it does.

Authoritative type definitions live in:

- `crate-coco-messages.md` — `Message`, `SystemMessage`, `MessageHistory`,
  `Visibility`, `MessageKind`.
- `crate-coco-tui.md` — `AppState`, `SessionState`, `StreamingState`,
  `ToolExecution`.
- `event-system-design.md` — three-layer `CoreEvent`.

## 0. Outcome Summary

| Concern | Before | Shipped | §  |
|---|---|---|---|
| Representations of one message | Two (`Message` engine; `ChatMessage`/`MessageContent` 40-variant TUI enum) | **One** (`Message`). TUI render reads `&Message` through derived `RenderedCell`. | §6 |
| Synthetic UI rows (hooks, plan markers, rate limits) | TUI-only `MessageContent` variants, never entered engine history | Engine-authored `Message::System(SystemMessage::*)` or `Message::User`/`Assistant`/`Attachment` round-tripped through history — see §3.2 on **why no new SystemMessage taxonomy was added** | §3.2 |
| Engine ↔ TUI sync | 19 ad-hoc `add_message` sites in TUI, independent of engine pushes | Engine pushes via `history_push_and_emit`; emits one `MessageAppended { message: Message }`; TUI is a pure derived view | §5 / §6 |
| Cancel marker `for_tool_use` | Computed twice (engine + TUI) — race | Computed once in `finalize_user_cancel`, stored as a typed `SystemUserInterruptionMessage` field, read by TUI | §7.1 |
| `/resume` TUI scrollback | Empty (engine seeded history, TUI not notified) | One `SessionResetForResume` + one `HistoryReplaced { messages }` rebuilds the derived view in a single cache-rebuild pass | §7.3 |
| Auto-restore round-trip | TUI truncated `session.messages` only; engine retained content | `UserCommand::AutoTruncate { message_id }` round-trips through the engine. Engine truncates, emits `MessageTruncated`. SDK + TUI both see it. | §7.4 |
| Streaming + in-flight tools | Tracked in `ui.streaming` + `tool_executions` AND mirrored as `MessageContent` rows | Stay in `ui.streaming` + `tool_executions` as widget overlays. Not part of transcript (I-3). | §6.4 |

Net code delta on landing: ~-280 LOC, zero `ChatMessage`/`MessageContent`/`add_message`
in `app/tui/src/`, no engine→TUI dual-write phase.

## 1. The Three Invariants

These are the durable rules. Everything else is mechanism.

**I-1 — Authority.** `coco_messages::MessageHistory` is the single source
of truth for transcript content. No other store may claim authority.
Every mutation goes through one of: `history_push_and_emit`,
`history_clear_and_emit`, `history_clear_and_emit_session_reset`,
`history_replace_and_emit` (in `app/query/src/history_sync.rs`). Direct
`history.clear()` / `history.messages = …` in production code is a bug.

**I-2 — Derived view.** TUI `RenderedCell`s are pure derivations of
`&Message` via `transcript/derive.rs::message_to_cells`. The TUI may cache
viewport-dependent layout at draw time but never as an authoritative
store. On any transcript mutation, the derived view reconciles via the
three lifecycle handlers (`on_message_appended`, `on_message_truncated`,
`on_session_reset`).

Reasoning aggregates are side-cache metadata keyed by message UUID, stamped by
`ReasoningMetadataAttached`; the `RenderedCell` itself remains a pure function
of `&Message`.

**I-3 — UI-only state stays UI-only.** Streaming in-flight text
(`ui.streaming`), running tool executions (`session.tool_executions`),
toasts, and `tool_group_summaries` are widget state, not transcript
content. They render as overlays between transcript and input bar.
When a streaming turn or tool call finalizes, the engine pushes a
`Message`; the overlay is then cleared by the `MessageAppended` handler.

## 2. Layer Ownership

| Crate | Owns | Forbidden |
|---|---|---|
| `coco-types::messages` (L1) | `Message`, `SystemMessage` sub-variants, `Visibility`, `MessageKind`, `MessageOrigin`, persistence DTOs. | UI rendering state, diff display state, viewport-relative state, theme/color, cell IDs. |
| `coco-messages` (L3) | History operations: `MessageHistory`, creation/normalize/filter/predicate functions; `create_user_interruption_system_message`. | Wire event emission (event emit lives in `coco-query`). |
| `coco-types::event` (L1) | `ServerNotification::MessageAppended` / `MessageTruncated` / `SessionResetForResume` / `HistoryReplaced` — typed wire payloads. | Logic; just transport shape. |
| `coco-query` (L5) | `history_sync` module (the four canonical writers + `finalize_user_cancel`); single cancel site; turn-loop pushes. | Per-cell UI choices. |
| `coco-tui` (L5) | `state::TranscriptView` (derived cell container), crate-internal `transcript::cells::{RenderedCell, CellKind, SystemCellKind}`, `transcript::derive::message_to_cells`. UI-only state. | Persistent representation of transcript content; recomputation of fields on `Message` (e.g. `for_tool_use`). |
| `app/cli` | Resume wiring (`tui_runner` emits `SessionResetForResume` + `HistoryReplaced`); the `AutoTruncate` / `Rewind` / `Compact` engine-side handlers. | Direct TUI state mutation. |

`message_to_cells` lives in `coco-tui`, **not** `coco-messages`. That's
the load-bearing layer-hygiene rule.

## 3. Data Model — Final Shape

### 3.1 `Message` envelope

`Message` is a 7-variant tagged union (`common/types/src/messages/message.rs:38-46`):
`User`, `Assistant`, **`System`**, `Attachment`, `ToolResult`, `Progress`, `Tombstone`.

`ToolUseSummaryMessage` is **not** a `Message` variant. Tool-use
summaries are a separate `ServerNotification::ToolUseSummary` event
(UI-only side-cache anchored by `preceding_tool_use_id`). They are
mobile-row labels, not transcript content (I-3).

`MessageHistory.messages` stays `Vec<Message>` (not `Vec<Arc<Message>>`).
The wire event payload (`MessageAppended { message: Message }`) clones
once at emit. In-process Arc sharing happens at the TUI cell layer
(`RenderedCell.source: Arc<Message>`, `transcript/cells.rs`), where
multiple cells derived from the same `Assistant` message share one
allocation. Wire serialization breaks Arc, so storing Arcs in
`MessageHistory` would save nothing across the SDK boundary; the
clone-at-emit design is intentionally simpler.

### 3.2 `SystemMessage` — 15 sub-variants total

The shipped set (`common/types/src/messages/message.rs:413-434`):

`Informational`, `ApiError`, `CompactBoundary`, `MicrocompactBoundary`,
`LocalCommand`, `PermissionRetry`, `BridgeStatus`, `MemorySaved`,
`AwaySummary`, `AgentsKilled`, `ApiMetrics`, `StopHookSummary`,
`TurnDuration`, `ScheduledTaskFire`, **`UserInterruption`** (added in
this refactor).

**`UserInterruption` shape:**

```rust
pub struct SystemUserInterruptionMessage {
    pub uuid: Uuid,
    pub for_tool_use: bool,
}
```

The engine cancel finalizer is the single writer; the TUI reads
`for_tool_use` from the field rather than recomputing it. **Divergence
from TS** (utils/messages.ts:207-208, 545-560): TS encodes the
distinction in message text (`INTERRUPT_MESSAGE_FOR_TOOL_USE` vs
`INTERRUPT_MESSAGE`). The Rust port chose a typed field to eliminate
the string-match dedup hazard. Resume from legacy TS-era JSONL is
preserved via `last_message_is_user_interruption` matching against both
forms (`history_sync.rs:208-227`).

**What was deliberately NOT added.** The 2026-04 draft proposed five
more variants (`HookOutcome`, `PlanEvent`, `SessionLifecycle`,
`Coordinator`, `ExternalNotification`). All were rejected — implementing
them would have *diverged* from TS, not aligned with it:

- Hook outputs in TS are regular `UserMessage`/`AssistantMessage`/
  `AttachmentMessage` (`HookResultMessage` is just `Message`, see
  `useDeferredHookMessages.ts:28-43`).
- Plan-mode banners and approval markers are normal messages with
  `isMeta`, not a separate system taxonomy.
- Teammate notifications, rate-limit banners, and channel pings flow
  as `AttachmentMessage` with the appropriate `AttachmentKind`.
- Lifecycle events (shutdown, etc.) fold cleanly into
  `SystemMessage::Informational` with the existing `level` axis.

Inventing new variants would have added engine complexity for no
behavioral benefit. The existing 14 + `UserInterruption` cover every
TS-equivalent case.

### 3.3 What got deleted from `coco-tui`

- `pub struct ChatMessage` — gone.
- `pub enum MessageContent` (40-ish variants) — gone.
- `pub fn add_message` and its 19 call sites — gone.
- `SessionState.messages: Vec<ChatMessage>` — replaced by
  `SessionState.transcript: TranscriptView`.

Verified by grep: no references in `app/tui/src/` (the only hit is the
unrelated `ChatMessageActions` keybinding enum).

## 4. Wire Protocol — Final Shape

### 4.1 Four `ServerNotification` variants for transcript lifecycle

All emitted from the `wire_tagged_enum!` table in
`common/types/src/event.rs`.

```rust
ServerNotification::MessageAppended { message: Message }                  // wire: "history/messageAppended"
ServerNotification::MessageTruncated { keep_count: i64 }                  // wire: "history/messageTruncated"
ServerNotification::SessionResetForResume { session_id: String }          // wire: "history/resetForResume"
ServerNotification::HistoryReplaced { messages: Vec<Message> }            // wire: "history/replaced"
```

`HistoryReplaced` is the bulk-load path used by `/resume` (one
cache-rebuild pass for a 5K-message transcript vs. ~20 channel-bounded
yields if each message went through `MessageAppended`). It is **not**
a violation of "no full-history broadcasts" — it's a separate operation
(wholesale replace) with a separate name. Live appends after the
initial replace still go through `MessageAppended`.

`SessionResetForResume` is the only broadcast that triggers a TUI
view rebuild. After receipt, TUI clears `TranscriptView`,
`tool_executions`, and `ui.streaming`; then processes the trailing
`HistoryReplaced` (resume) or future `MessageAppended`s (live) as
normal.

### 4.2 `UserCommand` — two disjoint commands, not one with `mode`

```rust
// app/tui/src/command.rs

UserCommand::Rewind {
    message_id: String,
    restore_type: RestoreType,   // Both / ConversationOnly / CodeOnly / SummarizeFrom / SummarizeUpTo / Nevermind
    rewound_turn: i32,
}

UserCommand::AutoTruncate {
    message_id: String,
}
```

The 2026-04 draft proposed one `Rewind { mode: RewindMode }` carrying
both shapes. Rejected: with a single command, `RestoreType` could leak
into the auto-restore path through a future refactor and silently
trigger file restoration on a Ctrl+C-while-empty. The two-command
design makes the "auto-restore never touches files" invariant
**structurally** unbreakable — the engine's `AutoTruncate` handler
literally cannot receive a `RestoreType`.

### 4.3 `UserCommand::PushSystemMessage` — typed payload

```rust
// app/tui/src/command.rs

UserCommand::PushSystemMessage { kind: SystemPushKind }

pub enum SystemPushKind {
    Informational {
        level: SystemMessageLevel,
        title: String,
        message: String,
    },
    LocalCommand { command: String, output: String },
}
```

TUI-originated transcript content (slash-command output, bash result
banners) routes through this command. The engine handler in
`tui_runner.rs:1514` constructs the matching `SystemMessage::*` and
calls `history_push_and_emit`, so the round-trip surfaces via the
standard `MessageAppended` → `TranscriptView` path. TUI never writes
to its own derived view (I-1).

The 2026-04 draft used `payload: serde_json::Value`. Rejected: lost
the compile-time kind/payload coherence check.

### 4.4 No new push paths beyond `history_sync`

All transcript-visible content goes through `MessageHistory::push`
**only** via one of the four `history_sync` writers. The agent loop
audit confirms 31 push sites across 7 files all thread through the
helpers; no direct `history.push(m)` outside `history_sync` or the
intentional resume hydration in `tui_runner.rs:339-345` (see §11 F2).

## 5. Engine — Final Shape (`coco-query`)

### 5.1 `history_sync` is the canonical writer module

```rust
// app/query/src/history_sync.rs

pub async fn history_push_and_emit(history, msg, event_tx);
pub async fn history_clear_and_emit(history, event_tx);                    // → MessageTruncated{0}
pub async fn history_clear_and_emit_session_reset(history, sid, event_tx); // → SessionResetForResume
pub async fn history_replace_and_emit(history, new_messages, event_tx);    // → MessageTruncated{0} + N MessageAppended (compaction path)
pub async fn finalize_user_cancel(history, in_flight_tool_calls, event_tx);
pub fn last_message_is_user_interruption(history) -> bool;
```

The `tracing` target is the canonical `coco::history_sync` so operators
can pivot a single filter to trace the full authority round-trip.

### 5.2 Cancel paths collapsed into one finalizer

Two engine cancel exits (`engine.rs:726` and `engine.rs:1815`) both
call `finalize_user_cancel`. `for_tool_use` is computed once from the
engine's authoritative view (`false` for pre-API-call cancel, the
turn-local `had_tool_use` flag for mid-streaming cancel). The TUI never
recomputes it.

Dedup is in `last_message_is_user_interruption`: it recognises both
the typed `SystemMessage::UserInterruption` form and the legacy
`INTERRUPT_MESSAGE*` text-form (for resumed older JSONL). It skips
trailing `Progress` / `Tombstone` rows when scanning, so a late
progress emit between two rapid Ctrl+Cs cannot break dedup.

### 5.3 Rewind handlers

Two engine-side handlers, mirroring the two `UserCommand` variants:

- **Explicit rewind** (`tui_runner.rs::handle_rewind`): consults
  `RestoreType`, restores files if requested, runs partial compaction
  for `SummarizeFrom` / `SummarizeUpTo`, emits both
  `Protocol::MessageTruncated` (SDK observers) and
  `Tui::RewindCompleted` (TUI overlay).
- **Auto-truncate** (`tui_runner.rs::handle_auto_truncate`, line 2929):
  truncates history at the target user-message UUID, emits
  `MessageTruncated`. Never touches the workspace.

Both converge on the same `MessageTruncated` event so SDK observers
see one truncation signal regardless of trigger.

### 5.4 Resume seeding

```rust
// app/cli/src/tui_runner.rs:325-374

runtime.start_new_session(plan.session_id.clone()).await;
{
    let mut history = runtime.history.lock().await;
    history.clear();
    for m in plan.prior_messages.iter().cloned() { history.push(m); }  // direct push — no per-message emit
}
runtime.seed_transcript_dedup(...).await;                              // JSONL on-disk dedup, not UI dedup
runtime.seed_tool_result_replacement_state(...).await;
notification_tx.send(SessionResetForResume { session_id }).await;       // teardown signal
notification_tx.send(HistoryReplaced { messages }).await;               // bulk rebuild
```

Direct `history.push` here intentionally bypasses
`history_push_and_emit` — the bulk event is `HistoryReplaced`, not
N per-message events. See §11 F2 for the proposed
`history_replace_silent` rename.

## 6. TUI — Final Shape (`coco-tui`)

### 6.1 `TranscriptView`

```rust
// app/tui/src/state/transcript_view.rs

pub struct TranscriptView {
    cells: Vec<RenderedCell>,
    by_uuid: HashMap<Uuid, usize>,  // engine message UUID → head cell index
}

impl TranscriptView {
    pub fn on_message_appended(&mut self, msg: Arc<Message>);
    pub fn on_message_truncated(&mut self, keep_count: usize);
    pub fn on_session_reset(&mut self);
    pub fn replace_from_messages(&mut self, messages: &[Message]);  // bulk path for HistoryReplaced
}
```

```rust
// app/tui/src/transcript/cells.rs

pub struct RenderedCell {
    pub message_uuid: Uuid,
    pub kind: CellKind,
    pub source: Arc<Message>,       // back-pointer for engine-authoritative fields
}
```

`message_to_cells(Arc<Message>) -> Vec<RenderedCell>` lives in
`transcript/derive.rs`. One `Message::Assistant` may yield multiple cells
(text + thinking + tool_use blocks); `by_uuid` indexes the **head**
cell of the group so renderers can find it.

Layout (`cached_lines` / `cached_height`) is intentionally **not** on
`RenderedCell`. Viewport-dependent caching lives in the renderer at
draw time, per the layer-hygiene rule.

### 6.2 `CellKind` and `SystemCellKind`

`CellKind` discriminates render dispatch (11 variants + nested
`System(SystemCellKind)`). `SystemCellKind` mirrors the 15
`SystemMessage` sub-variants. `UserInterruption` carries
`for_tool_use: bool` extracted from the engine field.

The `From<&SystemMessage> for SystemCellKind` impl is the single
mapping point (`transcript/cells.rs`).

### 6.3 `SessionState` shape

- `pub transcript: TranscriptView` — replaced the deleted `messages: Vec<ChatMessage>`.
- `pub tool_executions: Vec<ToolExecution>` — kept (UI-only widget state).
- `pub pending_auto_restore_truncate: Option<String>` — set by the
  `MessageAppended` handler when the protocol layer decides an
  `UserInterruption` warrants auto-restore; drained by the App loop
  via `drain_pending_auto_restore_truncate` into a `UserCommand::AutoTruncate`.
- `pub pending_system_pushes: VecDeque<SystemPushKind>` — set by
  TUI-only notification handlers; drained into
  `UserCommand::PushSystemMessage`.

### 6.4 Event handlers

`MessageAppended` handler atomically appends the cell **and** clears
`ui.streaming` when the appended message is an assistant push — same
frame, no flicker. `MessageTruncated` retains only `tool_executions`
whose anchor UUID is still present, prunes `reasoning_metadata`,
clears `ui.streaming`. `SessionResetForResume` clears all of the above
and rotates `conversation_id`.

### 6.5 Render order

```
[transcript.cells() rendered top-to-bottom]
[ui.streaming overlay if Some]
[tool_executions widgets if any Running/Queued]
[input bar]
[modal overlays / toasts on top]
```

## 7. Critical Flows

### 7.1 Cancel during streaming

```
User Ctrl+C
  → TUI sends UserCommand::Interrupt
  → engine cancel checkpoint (engine.rs:726): in_flight_tool_calls = false
  → finalize_user_cancel(history, false, event_tx)
      → dedup check (skip if tail already a UserInterruption / legacy text marker)
      → create_user_interruption_system_message(false)
      → history_push_and_emit → MessageAppended
  → TUI handler:
      transcript.on_message_appended(Arc<Message>)
      ui.streaming = None
```

### 7.2 Cancel during tool execution

Same path, `in_flight_tool_calls = had_tool_use` (engine.rs:1815).
Cell `for_tool_use = true`. **Computed once, stored on Message, read by
TUI.** No race possible.

### 7.3 `/resume` shows scrollback

```
tui_runner loads plan.prior_messages → MessageHistory (direct push, intentional)
  → emit SessionResetForResume → TUI clears TranscriptView, tool_executions, streaming
  → emit HistoryReplaced { messages } → TUI replace_from_messages rebuilds in one pass
```

TUI scrollback is fully populated with all prior content **including
typed `UserInterruption` markers and legacy text-form interrupt
markers** (the latter render via `CellKind::UserText` since they're
just `Message::User`).

### 7.4 Auto-restore

```
User Ctrl+C with empty input on tail boundary
  → MessageAppended { UserInterruption } handler:
      checks input empty + queue empty + only-synthetic-after preconditions
      sets state.session.pending_auto_restore_truncate = Some(last_user_uuid)
  → App loop drain: dispatches UserCommand::AutoTruncate { message_id }
  → engine handle_auto_truncate truncates history at that UUID
  → emits MessageTruncated { keep_count }
  → TUI handler: transcript.on_message_truncated, clears overlays
  → SDK consumer also sees MessageTruncated → consistent
```

Engine is authoritative for truncation. TUI converges by reading the
event. SDK never desyncs.

## 8. Implementation Phases (historical)

All shipped. Listing for archaeology:

- **Phase 1**: `SystemMessage::UserInterruption`, four
  `ServerNotification` variants in `wire_tagged_enum!`, `RewindMode`
  proposal — last item rejected; replaced by two-command design (§4.2).
- **Phase 2**: `history_sync` module, engine push-site migration (31
  sites), `finalize_user_cancel` collapse of `engine.rs:708`/`1787`
  (now at 726/1815 after intervening commits).
- **Phase 3**: TUI rewrite (single atomic landing). `TranscriptView`,
  `RenderedCell`, `derive::message_to_cells`. `ChatMessage` /
  `MessageContent` / `add_message` deleted.
- **Phase 4**: `/resume` hydration via `SessionResetForResume` +
  `HistoryReplaced`. The plan originally proposed N `MessageAppended`s;
  was replaced with the bulk path for perf.
- **Phase 5**: Auto-restore as a separate `UserCommand::AutoTruncate`
  (originally proposed as `Rewind { mode: AutoRestore }`).
- **Phase 6**: Cleanup gate — verified no `ChatMessage` /
  `MessageContent` / `add_message` survives; `just quick-check` and
  `just pre-commit` green.

## 9. Tests

Cross-layer end-to-end live in `app/query/tests/` and
`app/tui/src/state/`. Coverage spot-check:

- `history_sync::tests` — push/emit, clear/emit, replace/emit dedup of
  rapid double-cancel, legacy text-form recognition.
- `transcript_view::tests` — append, truncate (single + multi-cell
  messages), session reset, bulk replace.
- `state/rewind` + `protocol::tests` — auto-restore precondition
  checking, `pending_auto_restore_truncate` round-trip.
- Resume integration test (under `app/cli/tests/`) — JSONL with
  interrupt marker resumes with the dim cell present.

Snapshot tests for cell rendering live under `app/tui/snapshots/`.

## 10. Acceptance Criteria — All ✅

1. ✅ `coco-tui` contains no `ChatMessage`, no `MessageContent`, no
   `add_message`.
2. ✅ `coco-messages` contains no `bridge.rs`, no `message_to_rows`,
   no `MessageRowKind`, no UI rendering state.
3. ✅ `session.messages: Vec<ChatMessage>` replaced by
   `session.transcript: TranscriptView`.
4. ✅ Every `MessageHistory::push` in `app/query/` is paired with a
   `MessageAppended` emission (or an intentional bulk path).
5. ✅ Single `finalize_user_cancel` site computes `for_tool_use` once.
6. ✅ Two disjoint commands (`Rewind { restore_type }`,
   `AutoTruncate { message_id }`) — no `RewindMode` enum.
7. ✅ `/resume` populates TUI scrollback fully via
   `SessionResetForResume` + `HistoryReplaced`.
8. ✅ SDK NDJSON observers receive `MessageTruncated` for both
   explicit and auto-restore rewinds.
9. ✅ Cross-layer + snapshot tests pass.
10. ✅ `just quick-check` and `just pre-commit` green.

## 11. Open Hardening Items (deferred with concrete plans)

The findings below are real but require larger refactors than this
session's scope. Each entry names the **exact files** to touch, the
**design** to apply, and the **acceptance test** to confirm landing.

### F8 — `Vec<Arc<Message>>` storage + Arc-payload wire envelope

**Why it matters.** `history_sync.rs::history_push_and_emit` currently
runs `let notif_msg = msg.clone()` per push, performing a full
`Message` deep-clone (LlmMessage + content parts, often KBs). For a
500-push session that's ~500 wasted clones; for a 5K-message resume,
~5K. The TUI then wraps the cloned `Message` into a new `Arc` via
`Arc::new(message)` for `RenderedCell.source` — a second allocation
that would be free with shared Arc.

**Target shape.**
- `coco_messages::MessageHistory::messages: Vec<Arc<Message>>`.
- `ServerNotification::MessageAppended { message: Arc<Message>, session_id: String }`
  (paired with F9 below).
- `ServerNotification::HistoryReplaced { messages: Arc<[Arc<Message>]>, session_id: String }`.
- `RenderedCell.source: Arc<Message>` (already is).
- Push API: `MessageHistory::push(Arc<Message>) -> Arc<Message>` returns
  the same Arc so the helper can broadcast without an extra clone.

**Blast radius.** ~40 files touched. The biggest ripple is
`history.as_slice() -> &[Message]` consumers in `coco-compact`,
`coco-context`, `coco-permissions`, `coco-query::engine_*`. Strategy:

1. Phase 1 — internal storage swap. `MessageHistory.messages` becomes
   `Vec<Arc<Message>>`. Add `iter_messages() -> impl Iterator<Item = &Message>`
   that derefs each Arc, and `iter_arcs() -> impl Iterator<Item = Arc<Message>>`
   for emit paths.
2. Phase 2 — wire change. Flip `MessageAppended`/`HistoryReplaced` to
   `Arc<Message>` payloads. Update emit sites in `history_sync.rs`
   (4 functions).
3. Phase 3 — consumer migration. Replace `history.as_slice()` calls
   with `history.iter_messages().collect()` where a `Vec<Message>` is
   needed transiently, or `history.iter_arcs()` for stream-style
   consumers. Touches `coco_compact::estimate_tokens` signature
   (`&[Message]` → `&[Arc<Message>]` or generic over `impl Iterator`)
   — propagates ~15 callsites in `coco-query`, ~5 in `coco-context`.
4. Phase 4 — TUI handler. `on_message_appended(msg: Arc<Message>)`
   already takes Arc; just pipe the wire payload through without
   re-wrapping.

**Acceptance test.** Benchmark: 100-turn synthetic session + 5K-message
resume. Compare allocation count via `dhat`. Target: zero `Message`
clones in the push→emit→TUI path.

### F9 — `session_id` (+ `Option<agent_id>`) on transcript-lifecycle envelopes

**Why it matters.** Current `MessageAppended { message: Message }`
carries no session context. The receiver assumes "active session".
`AgentTeams`-spec says merged-timeline consumers will read the same
event stream — so the consumer must be able to route per-session.
Adding the field today is forward-compat; AgentTeams ships without
breaking changes.

**Target shape.**
```rust
ServerNotification::MessageAppended {
    message: Arc<Message>,         // F8 pairing
    session_id: SessionId,         // active session
    agent_id: Option<AgentId>,     // None = main agent; Some = teammate / subagent
}
// Same `session_id` field on MessageTruncated, HistoryReplaced,
// SessionResetForResume (already), ReasoningMetadataAttached.
```

**Blast radius.** Engine emit sites (4 in `history_sync.rs`, 2 in
`tui_runner.rs`) need to thread `session_id`. `coco_query::QueryEngine`
already exposes `config.session_id`. Pass through helper signatures:
`history_push_and_emit(history, msg, event_tx, session_id, agent_id)`.

**Acceptance test.** Wire roundtrip test: emit `MessageAppended` from
two distinct sessions; assert SDK consumer can demultiplex by
`session_id`.

### F12 — `MessageMode` ADT replacing multi-bool flags on `UserMessage`

**Why it matters.** Current `UserMessage` carries `is_meta`,
`is_virtual`, `is_compact_summary`, `is_visible_in_transcript_only`,
`permission_mode`, `origin` — 6 dimensions, some mutually exclusive,
some interacting (`is_meta` AND `is_virtual` produces undefined
semantics). Every `normalize_messages_for_api`, render filter, and
persistence predicate has to enumerate combinations.

**Target shape.**
```rust
enum MessageMode {
    Normal,
    Meta,             // hidden from UI, visible to API
    Virtual,          // visible to UI, not sent to API
    TranscriptOnly,   // recorded but neither rendered nor sent
    CompactSummary,   // compaction artifact
    SystemReminder,   // wrapped in <system-reminder>
}
// Visibility { api, ui } derives from MessageMode at filter sites.
```

**Blast radius.** This is the largest of the three deferred items.
Touches every consumer of `UserMessage` (~80 files). Strategy:

1. Add `mode: MessageMode` field alongside existing flags (don't
   delete yet). Derive its value from current flags at construction
   sites.
2. Migrate filters one at a time: `normalize_messages_for_api`,
   `is_meta()` predicate, `is_virtual()`, persistence filters. Each
   migration is independently testable.
3. Once all consumers read `mode`, delete the old flags.
4. Update construction sites (`create_user_message`,
   `create_meta_message`, etc.) to take `MessageMode` directly.

**Acceptance test.** Property test: for every `MessageMode` variant,
`Visibility::from_mode(mode)` produces the same `(api, ui)` tuple as
the prior boolean combination would have computed. Run all existing
predicate tests with both old and new code paths; identical outputs.

### Closed (no longer applicable)

- ~~F1 / F2 (bulk-replace inconsistency, silent push naming)~~ →
  resolved by 2026-05-20 `history_replace_and_emit` unification.
  Compaction and resume both emit `HistoryReplaced` now. The "silent"
  push path in resume hydration still exists (still doesn't emit
  per-message) but its sole `HistoryReplaced` is the bulk event the
  consumer expects.
- ~~F3 (record_reasoning_tokens I-2 violation)~~ → resolved by
  `ReasoningMetadataAttached`. TUI side-cache is event-driven.
- ~~F4 (constant pin)~~ → regression test landed.
- ~~F5 (`on_message_truncated` O(N))~~ → revisited; current
  implementation is fine up to ~10K messages and the parallel index
  is straightforward to add later. Not architecturally significant.
- ~~F6 (cross-runtime resume)~~ → confirmed out-of-scope per F13
  audit. TS reads typed `system` messages via discriminator; Rust's
  `UserInterruption` variant follows the same `system.subtype`
  convention. No incompatibility introduced.
- ~~F7 (CLAUDE.md doc drift)~~ → fixed.

## 12. Out of Scope (still)

- Multi-session shared transcript (`parent_session_id` filtering).
- SDK transcript pagination — existing `session/read` API is
  unchanged.
- AgentTeams coordinator merged-timeline view — will consume the
  same event stream.
- Remote mirror behavior — `MessageHistory` remains the outbound
  source.
- Markdown / diff rendering improvements — handled inside
  `message_to_cells` and the renderer; no protocol surface change.

## 13. References

- `crate-coco-messages.md` — canonical operations
- `crate-coco-tui.md` — canonical `SessionState`, `TranscriptView`
  invariants section
- `event-system-design.md` — three-layer `CoreEvent` catalog
- `streaming-metadata-roundtrip-plan.md` — `Arc<…>` event payload
  precedent
- `audit-gaps.md` — `/resume` TUI hydration gap (closed by Phase 4)
- TS reference:
  - `utils/messages.ts:207-208` `INTERRUPT_MESSAGE*` constants
  - `utils/messages.ts:545-560` `createUserInterruptionMessage`
  - `query.ts:1044-1050`, `1499-1505` cancel flow (TS yields per
    abort-signal `reason`)
  - `screens/REPL.tsx:1182-1222` `setMessages` single mutation point
  - `screens/REPL.tsx:3010-3022`, `3712-3739` auto-restore +
    `restoreMessageSync`
  - `hooks/useDeferredHookMessages.ts:28-43` hook messages append as
    regular `Message`s
  - `utils/conversationRecovery.ts:144-247` deserialization /
    rehydration

## 14. Why this design differs from earlier drafts

Both `engine-tui-message-bridge-plan.md` (formalized split with a
`MessageRowKind` enum) and `engine-tui-transcript-economical-plan.md`
(kept the split, fixed only the visible bugs) preserved the engine/TUI
representation divide. They are deleted.

This design rejects the divide entirely. The shipped result is also
materially cleaner than the 2026-04 plan version:

- TS reference: one `Message` union. `HookResultMessage` is just a
  `Message`. `createUserInterruptionMessage` returns a `UserMessage`.
  `<InterruptedByUser />` is a stateless presenter.
- coco-rs reality: 14 existing `SystemMessage` sub-variants already
  carry every TS-equivalent system row. Adding 5 more was vestigial.
  The single new variant (`UserInterruption`) replaces the text-based
  marker with a typed field — the only place a typed Rust shape beats
  the TS string-content approach.
- The plan's `Rewind { mode }`, `HistoryReplaced` prohibition, untyped
  `PushSystemMessage` payload, `Vec<Arc<Message>>` storage, and N-event
  resume were each replaced during implementation. In every case the
  rejection produced a smaller, safer, or faster design.
