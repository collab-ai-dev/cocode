# coco-skill-learn

Skill autonomous learning loop — the capability-layer analogue of `coco-memory`
(fenced background review fork + periodic curation), pointed at skills. LLM and
spawn interaction go only through `coco-tool-runtime` traits; no
`coco-messages` / `coco-inference` dep. Fire-and-forget by design: public entry
points return outcome enums (`ReviewTrigger`, `SkillReviewOutcome`,
`CuratorOutcome`) and there is deliberately **no crate-level error type** —
failures become traces plus backoff, never a caller-visible `Err`.

## Key Types

| Type | Purpose |
|------|---------|
| `SkillReviewRuntime` | Turn-end trigger: signal gate → throttle → single-flight → detached fork. Owns failure backoff and the review cursor. `manual_review` backs `/learn`. |
| `SkillReviewService` | Spawns the fenced review fork (`ForkLabel::SkillReview`, pinned to `ModelRole::Memory` — background self-improvement rides the memory knob, not the Review role) and runs post-fork trusted stamping. |
| `fence::SkillWriteHandle` | `CanUseToolHandle` confining the fork to `<config_home>/skills/.agent`. |
| `SkillCurator` | Periodic write-only retire/promote pass (time gate + `MaintenanceLock`). |
| `SkillLearnInbox` | Notice mailbox drained at turn finalize ("Learned skill: X"); pushed from a detached fork, so notices surface a turn late. |
| `journal::append_event` | The single write site for the skill learning journal. |

## The write/read split with `coco_skills::agent_scope`

- This crate owns the **write side**: the fence is pure spatial containment —
  symlink-aware root containment, dot-prefixed component deny, a
  documentation-extension allowlist, read-only bash via `coco_shell_parser`.
  It never reads target frontmatter (no per-write I/O, no TOCTOU). Inner ring
  to `AgentSpawnConstraints.allowed_write_roots`; `require_can_use_tool = true`
  keeps it running even when a hook pre-approves a tool.
- `agent_scope` owns the **read side** and the directory geometry:
  location-keyed inert load (force-drops `hooks`/`shell`/`allowed_tools`,
  force-stamps `origin: agent`) and quarantine via the promotions store.
- Loop metadata lives **outside** the fenced root as dot-prefixed siblings of
  `.agent` (curator lock, promotions store, `.agent-journal.jsonl`): the fork
  can neither self-promote, suppress curation, nor forge journal entries. The
  fence's dotfile deny is what makes that geometry hold.

## Invariants

- **Trusted provenance stamping** (`stamp`, private module): fork-written
  frontmatter is LLM-authored and never trusted. Learned-vs-Updated is decided
  by a host-captured **pre-fork snapshot** of existing skill names — never by
  the file's `created-at` (a fork could disguise a birth as an update).
  `origin` is always force-set; a birth force-sets `created-by`/`created-at`;
  an update preserves the original and only backfills. Writes are atomic, and
  the journal event + notice fire only when the stamp actually persisted.
- **Cursor semantics** (`runtime`): the review cursor advances only on a
  `Completed` fork, so a failed window is re-reviewed. History shrinking below
  the cursor (compact / `/clear` / rewind) resets it to 0 — clamping would
  permanently skip the post-compact window. Single-flight is released by a
  `Drop` guard so a panicking fork can't wedge later reviews into
  `InProgress`. Consecutive failures shift the effective throttle (capped).
- **Curator is location-keyed and write-only**: every directory under `.agent`
  is managed; frontmatter cannot opt out (an unstamped artifact must not be
  immortal). Retire = in-place `disabled: true` flip, never delete. The
  inactivity gate runs first — failure telemetry is infrastructure-level only,
  so "runs fine but unhelpful" skills age out via inactivity, not the rate
  gate. Promotions are journaled only after `save_promotions` persisted.

## Entry Points

- Auto: `app/query`'s finalize tail calls `maybe_review` from both mutually
  exclusive turn-end paths, gated on `Feature::SkillLearning` (re-checked each
  turn for settings hot-reload) plus non-bare, non-subagent. The fork-context
  closure is invoked only when actually firing.
- `/learn [directive]` → `CommandResult::TriggerSkillLearn` → `app/cli` slash
  execution → `manual_review`: bypasses throttle and signal gate, respects
  single-flight, stamps `created-by: manual`. There is no `/skill-review`
  command.
- Curator ticks at `bootstrap()` and piggybacks after every review fork.
  `AgentSlot` starts as `NoOpAgentHandle` until the CLI bootstrap calls
  `install_agent`.
