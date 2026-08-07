# codex-rs vs coco-rs — architecture / feature / TUI comparison

Baseline: `/lyz/codespace/3rd/codex` @ `4ee4192` (2026-08-07), `codex-rs` 1.32 M
Rust LoC / 2 932 files. coco-rs @ `feat/optimize` (94d9f05), 934 k LoC / 3 787
files. Comparable scale; different centre of gravity.

Prior sweep: [project-testing-borrow-codex] covered **testing** only (2026-06-27,
all P0/P1/P2 landed). This document covers architecture, features and TUI, and
does not re-litigate the testing findings.

---

## 1. Shape of the two trees

| | codex-rs | coco-rs |
|---|---|---|
| Crate count | ~110 | ~120 |
| Biggest crate | `core` (307 k) — one god-crate holding session, tools, context, sandboxing, guardian, compaction, rollout | `app/*` (269 k) split across query / agent-host / tui / server / session |
| Provider layer | `model-provider` + `codex-api` (OpenAI Responses only, plus ollama/lmstudio shims) | `vercel-ai/*` — 9 provider crates, full `@ai-sdk/provider` v4 port |
| Persistence | SQLite (`state`) + JSONL rollout (`thread-store/local`) with a live migration path | JSONL (`app/session`); SQLite only in `retrieval` + `hub/server` |
| Frontend surfaces | TUI, `exec` headless, `app-server` (JSON-RPC), MCP server, cloud-tasks | TUI, CLI headless, `sdk-server` (NDJSON), `app/server` (JSON-RPC), MCP, hub |
| Sandbox | Seatbelt / Landlock / bwrap / **Windows** (`windows-sandbox-rs`, 19 k) | Seatbelt / Landlock, `features.sandbox` off by default |

**Structural read.** codex is one very large `core` with sub-modules; coco is
layered with hard dependency rules and a seam guard. coco's layering is the
better long-term structure — codex's `core/src` has 120 top-level files and
`session/mod.rs` alone is 4 196 lines. Nothing in codex's layering is worth
importing. What is worth importing lives *inside* those crates.

---

## 2. Architecture — what codex does better

### 2.1 World State (⭐ the strongest single idea in the tree)

`core/src/context/world_state/` (4.6 k lines incl. tests).

Everything the model is *told about its world* — environment, AGENTS.md set,
model identity, permission mode, personality, plugin/app inventory, tool
inventory, collaboration mode, context-window guidance — is decomposed into
**sections** behind one trait:

```rust
trait WorldStateSection {
    const ID: &'static str;              // stable, persisted
    type Snapshot: Serialize + DeserializeOwned;
    fn snapshot(&self) -> Self::Snapshot;
    fn render_diff(&self, previous: PreviousSectionState<'_, Self::Snapshot>)
        -> Option<Box<dyn ContextualUserFragment>>;
    fn matches_legacy_fragment(role: &str, text: &str) -> bool { false }
    fn matches_retained_fragment(role: &str, text: &str) -> bool { false }
}
```

The mechanism:

- `WorldState::snapshot()` collects every section into a `WorldStateSnapshot`
  (`BTreeMap<section_id, Value>`).
- Consecutive snapshots are persisted to the rollout as an **RFC-7386 merge
  patch** (`merge_patch_from`) — so resume knows exactly what the model has
  already been told, at typed-field granularity.
- Each turn renders only the *diff*: `render_diff(previous)` emits a fragment
  only when that section actually changed. Nothing changed → nothing injected.
- `PreviousSectionState` is three-valued: `Known(snapshot)` (exact),
  `Unknown` (history has a matching fragment but no typed snapshot — migration
  path), `Absent` (never told). `render_history_diff` picks per section by
  scanning retained history when the snapshot is missing.
- Every rendered fragment carries a stable `(start_marker, end_marker)` pair
  (`ContextualUserFragment`) so it can be located and stripped from history, and
  a SHA-1 `WorldStateHash` for dedup.
- Extensions can contribute sections (`WorldStateSectionContribution`) without
  touching core.

**coco today.** `core/system-reminder` has the same *intent*, and its
generator/tier/cadence architecture is better than codex's for per-turn nudges.
Four things are already diffed properly against a **typed** baseline (not a text
scan — the `agent_listing_delta.rs` module doc saying "diffing against prior
announcements in history" is stale; the real implementation is
`app/query/src/engine_helpers.rs::compute_{tools,agents,mcp_instructions,mcp_servers}_delta`
against `ToolAppState.last_announced_*`). coco's compaction handling is
*finer-grained than codex's*: `preserved_contains_attachment_kind` keeps the
baseline per attachment kind depending on whether that specific announcement
survived the compaction, where codex blanket-resets the whole baseline on
`RolloutItem::Compacted`.

The three real gaps:

1. **The baseline is in-memory only.** `ToolAppState`
   (`common/types/src/app_state.rs:138`) derives `Debug, Clone, Default` — no
   `Serialize`/`Deserialize`, and nothing in `app/session` persists it. A
   resumed session starts with all four baselines empty and re-announces the
   full deferred-tool list, agent catalog, MCP server list, and every MCP
   server's instruction block, on top of a reloaded history that already
   contains those announcements. `is_initial` also flips back to true, so the
   agent listing re-frames as "Available agent types" rather than "New agent
   types are now available".
2. **Only four things are diffed.** Model identity, permission mode, the
   discovered CLAUDE.md/AGENTS.md set, environment info, and output style are
   all injected once into the static system prompt and never re-stated when
   they change mid-session. Switching model with `/model` mid-session tells the
   model nothing.
3. **No shared mechanism.** Adding a fifth diffed thing means touching five
   places: a `compute_*_delta` in `engine_helpers.rs`, a baseline field on
   `ToolAppState`, a `preserved_contains_attachment_kind` arm in
   `engine_compaction.rs`, a `GeneratorContext` field + builder method, and a
   generator. codex adds one file implementing one trait.

See §6 B1 and the detailed design in
[codex-worldstate-b1.md](codex-worldstate-b1.md).

### 2.2 Feature flags carry a lifecycle stage and a typed config

`features/src/lib.rs` — `Feature` is a closed enum like coco's, but each variant
also has a `Stage`:

```rust
enum Stage {
    UnderDevelopment,
    Experimental { name, menu_description, announcement },
    Stable,
    Deprecated,
    Removed,
}
```

`Stage::Experimental` *is* the `/experimental` menu — the menu is generated from
the enum, so shipping an experiment is a one-line change and can never drift
from its description. And features have typed per-feature TOML config
(`FeatureConfig` trait: `CodeModeConfigToml { enabled, default_exec_yield_time_ms,
excluded_tool_namespaces, … }`, `TokenBudgetConfigToml`, `NetworkProxyConfigToml`,
…), all `JsonSchema`, so `[features].x = { enabled = true, … }` is one coherent
surface rather than "flag here, config block there".

**coco today.** `Feature` is a bare capability gate; sub-settings live in a
separate `*Config` on `RuntimeConfig`. That split is a deliberate documented
rule ("Feature is a coarse capability gate, not a sub-toggle") and it is a
*good* rule — but coco has no experimental-stage concept at all, so there is no
mechanism for "ship dark → expose in a menu → promote → retire". Note the
`Deprecated`/`Removed` stages conflict with coco's "delete outright" hygiene
rule, and should not be imported; `Experimental` should. See §6 B4.

### 2.3 Persistent, searchable session state (SQLite)

codex runs a `state` crate on rusqlite with migrations, holding threads,
memories, goals, logs, queued items, remote-control state; `thread-store` wraps
it behind a trait with a `local` (JSONL rollout) implementation, an `in_memory`
one, and a **background paginated rollout→SQLite migration**
(`Feature::BackgroundPaginatedRolloutMigration`). That buys `search_threads`,
`archive_thread`, `list_threads` with metadata, `rollout_lineage` (fork trees),
and compression (`LocalThreadStoreCompression`).

**coco today.** `app/session/storage.rs` is JSONL with a 96-line
`storage/search.rs`. Resume/list works; search is a scan. coco already links
rusqlite (via retrieval + hub). This is a real capability gap but a *large*
change and only pays off at high session counts. See §6 C1.

### 2.4 Guardian — LLM approval reviewer, hardened

`core/src/guardian/` (8.8 k lines). coco has the equivalent concept (2-stage
auto-mode XML-LLM classifier in `core/permissions`), so this is not a gap — but
codex's hardening around it is worth copying piecemeal:

- **Bounded transcript reconstruction** with four separate token budgets
  (`GUARDIAN_MAX_MESSAGE_TRANSCRIPT_TOKENS` 10 k, `…TOOL_TRANSCRIPT…` 10 k, and
  per-entry caps of 2 k / 1 k) — so the reviewer sees intent + recent context but
  can never blow up.
- **Per-turn denial caps**: `MAX_CONSECUTIVE_GUARDIAN_DENIALS_PER_TURN = 3`,
  `MAX_CONSECUTIVE_CYBER_GUARDIAN_DENIALS_PER_TURN = 1`, plus a 50-entry
  rolling `AUTO_REVIEW_DENIAL_WINDOW`. Stops a mis-calibrated reviewer from
  deadlocking a turn, which is the classic failure mode.
- **Dedicated review session** with a prompt-cache-key override so the
  reviewer's cache never collides with the main turn's.
- Fail-closed on timeout (90 s), execution failure, *and* malformed output.
- A metrics module (425 lines) for allow/deny/timeout rates.

See §6 B3.

### 2.5 Declarative exec policy (`execpolicy`)

Starlark policy files:

```starlark
prefix_rule(
    pattern = ["git", ["status", "diff"]],
    decision = "allow",                        # allow | prompt | forbidden
    justification = "read-only git",
    match = [["git", "status"], "git diff"],   # validated at load time
    not_match = ["git push"],
)
host_executable(name = "git", paths = ["/usr/bin/git", "/opt/homebrew/bin/git"])
```

Two things make this better than hand-written Rust matchers: the `match` /
`not_match` examples are **unit tests validated when the policy loads**, and
`host_executable` closes the absolute-path-vs-basename bypass explicitly
(`/usr/bin/git` only falls back to `git` rules if that path is declared).

**coco today.** `utils/shell-parser` + `exec/shell` security analysis in Rust,
~3.7 k test lines. Strong coverage, zero extensibility — a user cannot express
"forbid `kubectl delete` in this repo" without a code change. Already flagged in
the 06-27 sweep as L-effort; still unclaimed. See §6 C2.

### 2.6 Smaller architecture wins

- **`tools/parallel.rs`: `Arc<RwLock<()>>` as the parallel-safety gate.**
  Parallel-safe tools take a read lock, unsafe ones take a write lock. One
  primitive replaces a queue + scheduler. coco's `StreamingToolExecutor` does
  safe-concurrent / unsafe-queued with more machinery.
- **`turn_diff_tracker`** — cumulative diff across a whole turn, so the UI can
  show "what this turn changed" rather than per-edit hunks. coco has
  `FileHistoryState` snapshots and computes pairwise diffs in the CLI driver;
  same numbers, less direct.
- **`context/` as a catalog** — 40 single-purpose files, one per injectable
  fragment, each ~30–60 lines with its own markers and tests. Trivially
  auditable ("what can the model be told?" = `ls`). coco's equivalents are
  scattered across `core/context`, `core/system-reminder/generators`, and
  inline strings in `app/query`.
- **`app-server-protocol/src/export.rs`** (3 k lines) — generates TS types +
  JSON schemas from the Rust protocol with `ts-rs` + `schemars`, with committed
  fixtures so drift fails CI. coco has both deps in the workspace but only uses
  them in leaf utils; the SDK wire surface is hand-maintained.

---

## 3. Features — capability diff

### 3.1 Things codex has that coco does not

| Capability | codex | coco status |
|---|---|---|
| **Persistent interactive shell** (`exec_command` + `write_stdin` on a PTY, `yield_time_ms` partial-output semantics, 64-process cap, head/tail output buffer) | `unified_exec`, 5 k lines | **Absent.** `Bash` + `background_task` can background a process and read its output, but there is no way to write to a running process's stdin. Interactive programs (REPLs, `ssh`, installers, `git rebase -i`, TUI wizards) are simply unreachable. |
| **Code Mode** — model writes JS/TS that calls tools as functions, run in a sandboxed out-of-process host; nested tool calls proxied back | `code-mode{,-host,-protocol,-runtime}`, 21 k lines, V8 | Partial. `core/workflow-runtime` (QuickJS) executes *authored* workflow scripts; the model cannot emit ad-hoc script that calls tools. Different product bet, not obviously worse. |
| **Network egress control** — MITM HTTP/SOCKS5 proxy, per-domain policy, per-process attribution, `network_approval` tool | `network-proxy`, 17 k lines | Absent, and the credential-injection half is an **explicit non-goal**. The *approval* half (prompt when a sandboxed command reaches the network) is not covered by that non-goal. |
| **Windows sandbox** | `windows-sandbox-rs`, 19 k | Absent (Seatbelt + Landlock only). |
| **Shell snapshot** — capture the user's login-shell env/aliases once, replay into every exec | `shell_snapshot.rs`, 3-day retention | Absent. coco resolves the login shell (`utils/shell-discovery`) but does not snapshot its state. |
| **`request_permissions` tool** — model explicitly asks to escalate, with justification | `handlers/request_permissions.rs` | Absent; coco escalation is user-driven only. |
| **`get_context_remaining` / `new_context_window` tools** — model can query its own budget and request a fresh window | present | Partial — coco *tells* the model via `token_usage` reminders, but the model cannot ask. |
| **Onboarding flow** | `tui/src/onboarding/` | Absent. |
| **Realtime / voice conversation** | `realtime_conversation` | coco has `voice` (STT dictation) but not a realtime duplex conversation. |

### 3.2 Things coco has that codex does not

Recording these so they are not re-flagged as gaps in future sweeps:

- **Multi-provider.** 9 provider crates ported from `@ai-sdk/provider` v4.
  codex is OpenAI-Responses-only (+ ollama / lmstudio compat shims). This is
  coco's single biggest structural advantage.
- **Hooks with SSRF guard, scoped priority, async registry** — codex `hooks` is
  11 k lines and newer; coco's is more mature on the security side.
- **Memory subsystem** — CLAUDE.md management, auto-extraction, KAIROS
  auto-dream, team sync. codex `memories` is 4.7 k lines and much narrower.
- **Retrieval** — BM25 + vector + AST + RepoMap PageRank (44 k lines). No codex
  counterpart.
- **Skills / skill-learn / plugins / output-styles / keybindings / journey** —
  richer extension surface overall.
- **Goals runtime** with sealed completion authorization + durable-before-visible
  transactions. codex has `Feature::Goals` and a `state/runtime/goals.rs`, but
  no equivalent authorization model.
- **Rewind** (`app/tui/src/update_rewind.rs`) restores *files* as well as
  conversation position; codex `app_backtrack` only forks the thread.
- **LSP-as-a-tool** (query by name+kind, 4 language servers). Absent in codex.
- **Cassette / wire-dump record-replay**, `require_live!` gating — confirmed
  ahead in the 06-27 sweep.

---

## 4. TUI

Both are ratatui + crossterm, both paint into **native scrollback** (not the
alternate screen) for committed history, both keep a live bottom region. The
architectures converged independently. Differences that matter:

### 4.1 Where codex is ahead

**Adaptive stream chunking (`tui/src/streaming/chunking.rs` + `commit_tick.rs`).**
A two-gear policy with hysteresis: `Smooth` commits one line per tick for a
readable reveal cadence; `CatchUp` drains the whole backlog when queue depth
> 8 lines or the oldest queued line is > 120 ms old; exit requires holding the
low thresholds for `EXIT_HOLD`, and re-entry is suppressed for
`REENTER_CATCH_UP_HOLD` unless the backlog is severe. Source-agnostic — it reads
only `(queue_depth, oldest_age)`. The module's doc comment includes a
symptom→knob tuning guide.

coco has no reveal pacing: stable regions are committed as fast as they are
produced. Fast providers therefore dump large blocks at once. This is arguably a
product choice rather than a bug, but the hysteresis design is the right one if
pacing is ever wanted.

**Onboarding + `keymap_setup`** — first-run auth/config flow and an interactive
keybinding editor. coco has a richer `keybindings` crate but no setup UI.

**`inline_visualization`** — the assistant emits a `::codex-inline-vis{…}`
directive; the TUI materializes an HTML/CSS document into the thread dir and
offers to open it. coco has `tui-mermaid` (renders *in* the terminal) — a
different and in some ways better answer, but no escape hatch to a real browser
for rich output.

**`pets/`** — a 1 k-line virtual pet. Noted for completeness; not a borrow.

### 4.2 Where coco is ahead

- **Seam-guarded presentational layer.** `tui-ui` is domain-free and i18n-free
  with a build-enforced seam; codex's `tui` mixes protocol types, config
  persistence, and paint. `tui/src/bottom_pane/chat_composer.rs` is **12 637
  lines** in one file.
- **VT100 cell-level test backend** (`tui-ui/src/engine/test_backend.rs`,
  landed 06-27) + insta visual snapshots.
- **i18n** (rust-i18n). codex has none.
- **Theme system** with hot reload (`theme/config.rs`, 849 lines).
- **Frame pacing already at parity.** `app/tui/src/frame_requester.rs` already
  coalesces per-iteration redraw signals through a `FrameRateLimiter` clamped to
  120 FPS — the same design as codex's `tui/src/tui/frame_rate_limiter.rs`.
  Nothing to take here.
- **Vim mode**, transcript search + search index, memory-trace panel, status-bar
  plugin surface — all absent in codex.
- **Markdown stable-prefix analysis is strictly stronger.**
  `tui-markdown/src/stable.rs` advances the committable boundary only at blank
  lines, closed fences, closed HTML blocks, and ATX headings, and additionally
  guards **list tightness** (a later sibling item flips a whole list from tight
  to loose in CommonMark, retroactively rewriting already-rendered items) and
  **reference-link definitions** (`requires_document_context`). codex commits
  line-by-line and needs a bespoke `table_holdback` scanner to avoid committing
  table rows at a column width a later row will change — a class of bug coco's
  block-boundary rule excludes by construction. **Verified: no table gap in
  coco.**

---

## 5. Verdict

coco-rs is not behind codex-rs architecturally. It is ahead on layering,
provider breadth, extension surface, retrieval, and TUI hygiene. codex is ahead
on four things worth taking (World State, guardian hardening, interactive shell,
experimental-feature staging), two things worth considering (SQLite session
store, declarative exec policy), and several things that are explicit coco
non-goals (egress credential proxy, Windows sandbox for now, browser/computer
use).

---

## 6. Prioritized borrow list

Effort: S ≤ 1 day · M ≈ 2–4 days · L ≈ 1–2 weeks.

### A — take now (high value, contained)

| # | Borrow | Effort | Why |
|---|---|---|---|
| **A1** | **Guardian hardening**: per-turn consecutive-denial cap + rolling denial window + explicit token budgets on the classifier's reconstructed transcript, in `core/permissions` | S | Pure robustness on code that already exists. The denial-deadlock failure mode is real and currently unbounded. |
| ~~A2~~ | ~~`ContextualUserFragment` marker discipline~~ | — | **Struck.** Markers exist in codex only to support matching rendered fragment text in history. coco's `Message::Attachment` already carries a typed `AttachmentKind`, so the entire text-matching layer has no consumer here. See [codex-worldstate-b1.md](codex-worldstate-b1.md) §R2. |

### B — take next (high value, real work)

| # | Borrow | Effort | Why |
|---|---|---|---|
| **B1** | **World State**: `WorldStateSection` trait + `WorldStateSnapshot` merge-patch persisted into the session file; migrate the four existing deltas onto it; add sections for model identity, permission mode, and the CLAUDE.md set | L | §2.1 + [codex-worldstate-b1.md](codex-worldstate-b1.md). Makes deltas survive **resume**, unifies five hand-wired seams into one trait, and closes the "model silently switched" hole. |
| **B2** | **Interactive shell sessions** — a `write_stdin`-capable persistent PTY process manager, exposed either as new tool verbs or as `Bash`+`background_task` extensions, with a head/tail output buffer and a process cap | M–L | §3.1. Unlocks REPLs, `ssh`, interactive installers, and `-i` git flows that are currently unreachable. coco already has `utils/pty` and `exec/shell` to build on. |
| **B3** | **`Stage::Experimental` on `coco_types::Feature`** + a generated `/experimental` menu | M | §2.2. Take `Experimental` only — `Deprecated`/`Removed` conflict with coco's delete-outright rule. |
| **B4** | **Protocol schema export** — `ts-rs` + `schemars` export of the SDK/AppServer wire types with committed fixtures gated in `just check-docs` | M | §2.6. Both deps are already in the workspace; the SDK surface is hand-maintained today. |

### C — decide, don't drift

| # | Borrow | Effort | Note |
|---|---|---|---|
| **C1** | SQLite session store + `search_threads` / `archive` / lineage, with background JSONL→SQLite migration | L | §2.3. Only pays at high session counts. Needs an explicit product call. |
| **C2** | Declarative exec policy (Starlark `prefix_rule` + load-time `match`/`not_match` validation + `host_executable`) | L | §2.5. Flagged 06-27, still unclaimed. Adds a Starlark dep. |
| **C3** | Network-access *approval* (not the credential proxy) | M | The egress proxy is a non-goal; prompting on network reach from a sandboxed command is not covered by it. |
| **C4** | Adaptive stream chunking / reveal pacing | M | Only if paced reveal is wanted as a product behaviour. |
| **C5** | Shell snapshot (login-shell env/alias capture + replay) | S–M | Cheap, but changes exec semantics — needs a call. |

### D — do not take

Code Mode (coco bets on authored workflows instead) · Windows sandbox · MITM
credential proxy (**explicit non-goal**) · `Stage::Deprecated` / `Stage::Removed`
· inline HTML visualization (coco has `tui-mermaid`) · `pets/` · codex's
`core` mono-crate layering · codex's line-by-line markdown commit + table
holdback (coco's block-boundary rule is strictly safer).
