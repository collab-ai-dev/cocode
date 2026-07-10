# Tool Result Offload v2 — Unified Recoverable Offload (Design)

> Status: **draft v2 — adversarial review incorporated** (2026-07-10; review record in §13)
> Baseline: [tool-result-budget-plan.md](tool-result-budget-plan.md) (Level 1/2 landed per that plan, enabled by default)
> Inspiration: Hermes Agent's `web_extract` truncate-and-store rework (their repo #54843; measured
> 11.7x speedup, equal quality, 4/4 truncated answers recoverable from the stored full text) plus
> its three-layer tool-result budget system.
> This design **disregards backward compatibility** (user directive) — old paths may be deleted
> and names changed outright.

---

## 1. Current State and Gaps

What coco-rs already has (verified against code, not docs):

| Capability | Location | Status |
|---|---|---|
| Level 1 per-result persistence | `core/tool-runtime/src/tool_result_storage.rs:176` `persist_to_disk` → `<session_dir>/tool-results/<id>.{txt,json}`; `render_persisted_reference` (:252) emits the `<persisted-output>` reference | ✅ live by default (when a transcript is wired), applied to every tool result via `app/query/src/tool_outcome_builder.rs:78,114` |
| Per-tool cap declaration | `Tool::max_result_size_bound()` (traits.rs:653), `ResultSizeBound::{Chars,Unbounded}` | ✅ Bash 30k, Grep 20k, Read exempt, etc. |
| Level 2 per-turn aggregate budget | `apply_tool_result_budget` (tool_result_storage.rs:474), wired at `engine_prompt.rs:229`; config `ToolResultBudgetConfig` (compact_settings.rs:399, default enabled=true, 200_000) | ✅ live by default |
| After-the-fact cleanup | micro-compact (`services/compact/src/micro.rs:58`) clears old results of 9 tool kinds to `[Old tool result content cleared]` | ✅ |

Gaps (the complete problem set this design addresses):

1. **WebFetch is stuck in the "old paradigm"** (`core/tools/src/tools/web.rs:798-1035`):
   - Every non-preapproved fetch runs a side-query LLM extraction (Stage 4, web.rs:984) — even
     small pages burn a side-model call (seconds of latency + cost);
   - Content over 100k is **silently truncated** (Stage 3, web.rs:926-939) — only a JSON
     `"truncated": true` flag; the model-visible text carries no "how much was lost, how to
     recover" information;
   - Full text lives only in a 15-minute in-memory cache (web.rs:145-171), gone on expiry;
   - Binary content lands in `std::env::temp_dir()/coco-web-fetch/` (web.rs:1231) — a second
     spill path decoupled from the session lifecycle.
2. **The Level 2 budget does not scale with the model window**: 200_000 bytes ≈ 50k tokens; for a
   32k–65k-window model a single turn's tool output can approach or exceed the whole window,
   leaving only the 95%-threshold reactive compact as a late fire brigade — while the model's
   context window (`ModelInfo.context_window`, common/config/src/model/mod.rs:53) is resolvable
   at runtime via the model-runtime snapshot.
3. **The persisted reference is a head-only preview**: `render_persisted_reference` gives the
   first 2000 bytes + a path — no tail, no precise continue-reading offset. Bash self-truncation
   (bash.rs:1593) is likewise head-only — build/test errors live at the tail.
4. **No base64/data-URI defusing**: inline images in fetched content (a single one can be tens of
   thousands of chars) enter context verbatim.
5. **micro-compact clearing is permanently lossy**: it destroys pointer-bearing
   `<persisted-output>` references along with everything else, even though clearing an
   already-persisted ~2KB reference frees almost nothing and burns the only pointer to the data.
6. Naming wart: `ResultSizeBound::Chars` actually measures **UTF-8 bytes** (`content.len()`);
   docs and name contradict each other (tool_result_storage.rs:66 itself says "UTF-8 bytes").

## 2. Design Overview

**One sentence: build no parallel system. Add three orthogonal primitives to the existing
`tool_result_storage` — a head+tail window view (`WindowedView`), an artifact naming policy
(`ArtifactKey`), and a window-scaled budget (`scaled_per_message_bytes`) — then migrate
WebFetch / the Level-1 renderer / Bash / micro-compact onto them.**

```
common/config
  └─ compact_settings.rs      per_message_chars → per_message_bytes: Option<i64>
                              (None = scale by window, new default)
  └─ sections.rs              WebFetchConfig: + inline_byte_budget (15_000),
                              + extraction: WebFetchExtraction { Auto | Windowed | Llm }
                              (enum defined HERE, in coco-config — consumed by coco-tools;
                              same pattern as other config enums);
                              max_content_length re-purposed as "retained full-text cap",
                              default 2_000_000
core/tool-runtime                                   ← mechanism core
  ├─ tool_result_storage.rs   existing primitives untouched (persist_to_disk / ToolOutputStore
  │                           / apply_budget); ResultSizeBound::Chars → ::Bytes (name follows
  │                           reality); new atomic-publish write path for Named keys
  └─ tool_result_offload.rs   new module (storage.rs is already 637 lines; new file per the
                              <800 LoC rule)
       InlineBudget           newtype over i64; two constructors (§3)
       WindowedView<'a>       pure window computation, zero I/O, zero allocation
       ArtifactKey            enum { ToolUse{id,is_json}, Named{file_name} } (§3)
       offload_windowed(store: Option<&ToolOutputStore>, …)   free fn: works without a store
       scaled_per_message_bytes()            Level 2 budget scaling, pure function
       REFERENCE_BUDGET       fixed 4_000-byte budget for Level-1/Level-2 reference renders
core/tools
  ├─ tools/web.rs             WebFetch Stage 3/4 re-railed (§4); data-URI defusing; hard-wrap;
  │                           content-addressed artifact names; binary spill folded into store
  └─ tools/bash.rs            full output routed through the offload seam (§7)
app/query
  ├─ tool_outcome_builder.rs  Level 1 spill rendering switched to the windowed reference render
  └─ engine_prompt.rs         budget resolution fed with resolved_context_window()
services/compact
  └─ micro.rs                 skips pointer-bearing results instead of clearing them (§6)
```

Dependency direction unchanged: `tool-runtime` gains no upward dependency and does **not** learn
URL semantics (see `ArtifactKey::Named` — the caller computes the name; the runtime only
validates it). `tools` / `query` / `compact` consume it. No new crate.

## 3. Core Types (`core/tool-runtime/src/tool_result_offload.rs`)

```rust
/// Inline byte budget for a single result. Newtype over i64 (repo integer
/// convention); converted to usize only at slicing sites.
///
/// Two constructors, split by caller intent (mirrors ResultSizeBound::try_chars
/// precedent):
///  - `try_new(i64) -> Option<Self>`: for CONFIG resolution — invalid (<= 0)
///    yields None and the caller keeps its default (matches the repo's
///    `.filter(|v| *v > 0)` config convention; garbage is rejected, not
///    "helpfully" clamped).
///  - `from_request(i64) -> Self`: for MODEL-supplied per-call params, where
///    failing the tool call over a budget argument would be wrong — clamps
///    into [2_000, 500_000].
///
/// The type-level ceiling is a sanity bound only. The BINDING ceiling is
/// applied at each offload call site: effective = min(requested, configured,
/// resolve_persistence_threshold(tool_bound) - FOOTER_RESERVE) — see §9's
/// no-double-persist invariant, which holds by construction, not by hope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InlineBudget(i64);

pub const FOOTER_RESERVE: i64 = 1_000;

/// Fixed budget for Level-1 spill references and Level-2 eviction
/// replacements (≈ today's 2KB preview, now split ~3k head + 1k tail).
/// Deliberately small and CONSTANT: Level-2 convergence requires each
/// replacement to free nearly the entire candidate, and replacements are
/// re-sent every turn. Tool-level inline budgets (WebFetch 15k, Bash 30k)
/// apply only to a tool's OWN inline output, never to reference renders.
pub const REFERENCE_BUDGET: InlineBudget = InlineBudget(4_000);

/// Pure computation result of a head+tail window. Borrows slices of the
/// original — no allocation, no I/O — unit-testable in isolation.
#[derive(Debug, PartialEq)]
pub struct WindowedView<'a> {
    pub head: &'a str,
    pub tail: &'a str,
    /// Byte length of the text the window was computed from (== the text
    /// that gets persisted; see §4 step 2 for the pre-cap page size, which
    /// is reported separately in the footer when the retained copy was capped).
    pub total_bytes: usize,
    /// Total '\n'-terminated line count of that same text (footer display).
    pub total_lines: usize,
    /// 1-indexed first line of the omitted middle:
    ///   omitted_start_line = count('\n' in head) + 1
    /// Correct in BOTH cases: if head ends at a newline (snapped), lines
    /// 1..=n are complete and line n+1 is the first omitted line; if head
    /// ends mid-line (snap skipped), line n+1 is the partially-shown line and
    /// the pointer re-reads it, so its unseen remainder is never skipped.
    /// (v1 said "head line count + 2" — off by one; caught in review.)
    pub omitted_start_line: usize,
    /// 1-indexed last omitted line (= first tail line - 1). Both endpoints go
    /// into the footer so the model knows the exact gap.
    pub omitted_end_line: usize,
}

impl<'a> WindowedView<'a> {
    /// Returns None when content fits the budget (whole-inline zero-cost
    /// path — caller returns the content unchanged).
    ///
    /// Split: 75% head + 25% tail. UTF-8 safety via
    /// `coco_utils_string::take_bytes_at_char_boundary` /
    /// `take_last_bytes_at_char_boundary` (the repo's blessed primitives),
    /// then snapped to line boundaries: head retreats to its last '\n', tail
    /// advances past its first '\n'. Snapping happens only when it costs less
    /// than half the respective sub-budget; head and tail ranges never
    /// overlap (tail_start is clamped to >= head_end).
    pub fn compute(content: &'a str, budget: InlineBudget) -> Option<Self>;
}

/// Artifact naming policy. The runtime owns NO URL/domain semantics — callers
/// compute names; the runtime validates and writes.
#[derive(Debug, Clone)]
pub enum ArtifactKey {
    /// Regular tool result: named by tool_use_id (globally unique — keeps the
    /// current create_new path and resume semantics). `is_json` selects the
    /// extension, mirroring tool_result_path.
    ToolUse { id: String, is_json: bool },
    /// Caller-computed file name for shareable artifacts. Validated by the
    /// runtime: [A-Za-z0-9._-]+ only, <= 100 bytes, must not start with '.',
    /// and callers MUST prefix a fixed literal (WebFetch uses "url-") — the
    /// prefix also neutralizes Windows reserved device-name stems
    /// (con./nul./com1.) wholesale.
    ///
    /// Named keys are written via ATOMIC PUBLISH: write to
    /// `<name>.tmp-<uuid>` in the same directory, then rename over the
    /// target. Rename is atomic on one filesystem, so a concurrent reader
    /// never observes a partial file, and last-writer-wins is safe because
    /// WebFetch names are content-addressed (§4) — same name ⟹ same bytes.
    /// The footer is ALWAYS computed from the in-memory string that was
    /// written, never from re-reading disk.
    Named { file_name: String },
}

/// Offload output. stored_path = None in two distinguishable situations,
/// both degrade to "window without pointer" and are model-visible in the
/// footer ("full text not saved — persistence unavailable"):
///  - no store (forks with transcript disabled, SDK embeddings, tests),
///  - store present but the write failed (best-effort, same policy as
///    Level 2's freeze-on-persist-failure).
/// A missing store must NEVER become a tool error (v1 precedent audit found
/// the Level-1 path can hard-error today; this contract supersedes it).
pub struct OffloadedText {
    pub model_text: String,
    pub stored_path: Option<PathBuf>,
    pub was_windowed: bool,
}

/// Free function, not a ToolOutputStore method: callable with store = None.
pub async fn offload_windowed(
    store: Option<&ToolOutputStore>,
    key: &ArtifactKey,
    content: &str,
    budget: InlineBudget,
) -> OffloadedText;

/// Level 2 budget scaled to the model window. Pure function, table-driven
/// tests. Models with >= 200k-token windows are byte-identical to today
/// (e.g. 200_000 tokens → 240_000 → clamped to 200_000 = current default);
/// small models get protection: 32_768 tokens → 39_321; floor 16_000.
pub fn scaled_per_message_bytes(context_window_tokens: i64) -> i64 {
    const BYTES_PER_TOKEN: i64 = 4;      // matches the budget-plan estimator
    const WINDOW_PCT: i64 = 30;          // one turn's tool output ≤ 30% of the window
    let scaled = context_window_tokens * BYTES_PER_TOKEN * WINDOW_PCT / 100;
    scaled.clamp(16_000, DEFAULT_MAX_PER_MESSAGE_BYTES)
}
```

Model-visible layout (**normative**): `model_text` = head, then an omission marker line, then
tail, then the reference block. The `<persisted-output>` tag wraps **only the trailing reference
block** — the text intentionally does *not* start with the tag, so windowed output stays eligible
for Level-2 accounting (§9):

```
<page head content …>

[... middle omitted — see footer ...]

<page tail content …>

<persisted-output>
Showing 11,180 bytes (head, lines 1–214) + 3,720 bytes (tail, lines 1530–1600)
of 84,532 bytes (1,600 lines). Omitted: lines 215–1529.
Full text saved to: <session_dir>/tool-results/url-docs.rs-a1b2c3d4e5-9f8e7d6c.md
To read the omitted middle: Read file_path="<same path>" offset=215 limit=200
(raise offset to page onward)
</persisted-output>
```

**The design's core (Hermes's most valuable lesson): the pointer must be a directly
copy-executable call whose offset lands exactly on the information gap** — not a vague
"content too large, saved somewhere". Read's offset is 1-based (read.rs:52-56,
read_loader.rs `start_index = offset - 1`), matching `omitted_start_line` exactly.

**Read-navigability by construction**: persisted artifact text is hard-wrapped at 400 bytes per
line (at char boundaries, preferring whitespace) *before* windowing and persisting, and the
window is computed from the **same wrapped text**, so footer line numbers and artifact line
numbers agree by construction. This guarantees `Read offset=… limit=200` slices stay ≈ ≤ 80KB —
under Read's hard caps (25k-token slice cap in `enforce_token_cap`, 256KB full-read cap in
`MAX_READ_OUTPUT_BYTES`) — even for single-line JSON or minified pages. (v1 wrongly assumed Read
had per-line truncation; it hard-errors instead. Caught in review.)

### 3.1 Renames Done Alongside (no back-compat constraints)

The v1 draft renamed `Chars→Bytes` and then introduced *new* byte quantities named "chars" —
half a rename is worse than none (review finding). The full vocabulary moves in one pass:

| Old | New |
|---|---|
| `ResultSizeBound::Chars` / `as_chars()` | `ResultSizeBound::Bytes` / `as_bytes()` |
| `DEFAULT_MAX_RESULT_SIZE_CHARS` (50_000) | `DEFAULT_MAX_RESULT_SIZE_BYTES` |
| `DEFAULT_MAX_PER_MESSAGE_CHARS` (200_000) | `DEFAULT_MAX_PER_MESSAGE_BYTES` |
| `ContentReplacementState.per_message_chars` | `.per_message_bytes` |
| `ToolResultCandidate.content_chars` | `.content_bytes` |
| config `compact.tool_result_budget.per_message_chars` | `.per_message_bytes` (now `Option<i64>`) |
| env `COCO_COMPACT_TOOL_RESULT_BUDGET_PER_MESSAGE_CHARS` | `…_PER_MESSAGE_BYTES` |
| (new) WebFetch config | `inline_byte_budget`; tool param `inline_bytes` |

- The head-only `render_persisted_reference` is **deleted**; Level-1 spill and Level-2 eviction
  replacements both render through the windowed reference at the fixed `REFERENCE_BUDGET`
  (4_000 bytes ≈ today's 2KB preview, now head+tail). Replacement strings already persisted in
  transcripts are unaffected (they are data, replayed byte-identically — never re-rendered).
- `PersistedToolResult.preview/has_more` reshaped into head/tail segments.

## 4. WebFetch Re-Rail (first consumer)

The current pipeline's Stage 3 (silent truncation, web.rs:926) + Stage 4 (side-query LLM
extraction, web.rs:984) are replaced by a config-dispatched stage:

```rust
// Defined in coco-config (common/config/src/sections.rs, next to WebFetchConfig);
// coco-tools consumes the resolved value. (v1 showed it inside web.rs — that
// would invert config→tools layering. Caught in review.)
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebFetchExtraction {
    /// Default: Windowed for content that is already clean (text/markdown,
    /// text/plain, JSON and other non-HTML passthrough); Llm for text/html.
    /// Rationale: the Hermes 11.7x/equal-quality evidence was measured on
    /// clean-markdown backends; coco's html2text output keeps nav/footer
    /// chrome that clusters exactly in the head window. Flipping the HTML arm
    /// to Windowed is gated on the §12 telemetry (or on adding a
    /// readability-style main-content pass before windowing).
    #[default]
    Auto,
    /// Deterministic window + persisted pointer for ALL content types.
    Windowed,
    /// The previous behavior: side-query extraction for everything
    /// non-preapproved.
    Llm,
}
```

The windowed path, in order:

1. After HTML→markdown, run **data-URI defusing** (private fn in `web.rs`):
   `![alt](data:image/...;base64,...)` → `[IMAGE: alt]`; bare `data:image/...` → `[IMAGE]`;
   real http(s) image links preserved. Applies to the JSON/text passthrough path too.
   The defusing scan is linear (memchr for `data:image/` then bounded base64 run) — **no
   backtracking regex** over multi-MB strings.
2. **Hard-wrap** the defused text at 400 bytes/line (§3) — this wrapped text is the canonical
   "retained text": the window, the persisted artifact, and all footer numbers derive from it.
   Retained cap: `web_fetch.max_content_length` re-purposed as "retained-text cap", default
   100_000 → **2_000_000** (HTTP-level 10MB hard cap unchanged). Pages larger than the cap are
   truncated *before* windowing, so window/footer/artifact always describe the same text; the
   footer then reports "first 2,000,000 of M bytes saved" so the model knows the tail window is
   of the retained copy, not the raw page.
3. `retained_text ≤ effective inline budget` → **returned verbatim, zero LLM calls, zero
   persistence**. Effective budget = `min(inline_bytes param, web_fetch.inline_byte_budget
   [default 15_000], resolve_persistence_threshold(WebFetch bound) − FOOTER_RESERVE)` — the last
   term makes the §9 no-double-persist invariant structural (resolved threshold is 50_000 today,
   so the per-call ceiling is 49_000 regardless of the InlineBudget type's 500k sanity bound).
   **Preapproved docs hosts keep their own, larger verbatim window**: preapproved + text/markdown
   + `≤ preapproved_verbatim_budget` (default **100_000**, i.e. today's behavior) returns whole —
   the flagship docs-reading case must not regress from ~100k verbatim to a 15k window (review
   finding).
4. Over budget → `offload_windowed(store, ArtifactKey::Named{file_name}, retained_text, budget)`.
   **Content-addressed name**: `url-<host-slug>-<sha256(url)[..10]>-<sha256(retained_text)[..8]>.md`,
   built by WebFetch (which already owns host parsing, web.rs `extract_host`) — the runtime only
   validates (§3). Same URL + same content dedups to the same file; changed content gets a NEW
   file, so pointers frozen into earlier turns keep referencing the bytes they were computed from
   (v1's URL-only key served stale artifacts after the 15-min cache TTL; caught in review —
   confirmed by 5 independent lenses). `extraction_mode: "windowed"`.
   Store absent/write failed → window with the "not saved" footer variant; never a tool error.
5. Binary spill moves from `temp_dir()/coco-web-fetch/` into `ToolOutputStore::persist_binary`
   (unified lifecycle; `web.rs:1231 persist_binary_content` deleted). Store-None keeps the
   temp-dir fallback for binaries only.
6. The 15-minute in-memory cache is **kept but re-scoped**: it caches the final rendered
   `OffloadedText` (model_text + stored_path), keyed by `(url, effective_inline_budget)` — NOT
   the multi-MB retained text. Worst case returns to ~6.4MB (128 × ≤50k), *below* today's
   documented 12.8MB bound (v1 would have silently raised it to ~256MB; caught in review).
   A budget-mismatched call is a cache miss.
7. The `prompt` param becomes **optional** and is dropped from validation in windowed mode (it
   has zero effect there; a required-but-inert param is schema dishonesty — review finding).
   Under `extraction = llm` it remains required and drives extraction as today.

**Why the default flips for clean content but not (yet) for HTML**: Hermes's measured results
transfer directly to content that arrives clean (markdown/plain/JSON — this includes the
preapproved-docs flow, arxiv/PDF-adjacent text endpoints, raw API responses). For scraped HTML,
coco's html2text is a whole-DOM dump; until a main-content extraction pass exists or telemetry
shows head+tail windows satisfy real HTML tasks, the side-model extract stays the HTML default.
`extraction = windowed` forces the flip for users who want it; `extraction = llm` restores v0
everywhere.

## 5. Level 2 Budget Scaling Wiring

- `ToolResultBudgetConfig.per_message_bytes: Option<i64>`; `None` (new default) = scaled;
  `Some(n)` = fixed (explicit configuration). Env
  `COCO_COMPACT_TOOL_RESULT_BUDGET_PER_MESSAGE_BYTES` parses into `Some`.
- Wiring (corrected — v1 cited a nonexistent `QueryEngineConfig.context_window` field; caught in
  review): the window comes from the engine's **live resolved-model snapshot**,
  `self.resolved_context_window()` (engine_builder.rs:231, reading
  `ModelRuntimeSnapshot.model_info.context_window`), sampled at each prompt build inside
  `apply_tool_result_budget_to_prompt`. When the snapshot or `ModelInfo` is unavailable, fall
  back to `DEFAULT_MAX_PER_MESSAGE_BYTES` (200_000) — the budget pass must never abort a turn.
- The window is **time-varying** (mid-session `/model` switches, plan-mode role swaps, provider
  window clamps). That is safe by the existing freeze semantics: already-replaced ids replay
  their cached byte-identical strings, `seen_ids` stay frozen; only new turns budget under the
  new window. Stated here explicitly so nobody "fixes" the variance later.
- The `ContentReplacementState` `i64::MAX` "off" sentinel behavior is unchanged.

## 6. micro-compact: Preserve Pointers Instead of Clearing Them

v1 proposed a placeholder-with-path sourced from `ContentReplacementRecord`s. Review killed it
three ways: records are written only by Level 2 (Level-1 persists and WebFetch offloads produce
none); for exactly the recorded ids the prompt projection replays the frozen replacement over
whatever micro-compact writes (the placeholder could never reach the model); and
`ContentReplacementRecord` carries no path field, while `services/compact` has no dependency
route to the session store. All confirmed; design replaced by something strictly simpler:

**micro-compact skips pointer-bearing results.** A result whose content is already a persisted
reference or a windowed render (prefix `<persisted-output>` **or** suffix `</persisted-output>`
— the offload footer sits at the end) is left intact: it is already small (≈2–15k), already
self-describing, and clearing it frees almost nothing while destroying the only pointer.
Everything else clears to the unchanged `[Old tool result content cleared]` placeholder —
no second placeholder form, no byte-stability risk, no new cross-crate data flow.

The suffix/prefix predicate is a new `is_pointer_bearing(content)` helper in
`tool_result_storage` (exported for `coco-compact`). It is **deliberately distinct** from
`is_content_already_persisted` (prefix-only), which Level 1/2 keep using so that windowed
inline output remains eligible for Level-2 accounting and eviction (§9).

## 7. Bash Re-Rail

v1 claimed "the truncated content is already gone from memory" — false: `truncate_output` runs
on the complete in-memory captures (bash.rs:1108-1110, :1321; the shell executor collects
unbounded). Caught in review. Since the full output is right there, Bash routes through the same
seam instead of hand-rolling truncation:

- Store available: `offload_windowed(store, ArtifactKey::ToolUse{id, is_json:false},
  full_output, InlineBudget(min(bash.max_output_bytes, resolved_bound − FOOTER_RESERVE)))` —
  the model gets a head+tail window (build errors at the tail survive) plus a real pointer to
  the **complete** output; Level 1 never re-persists it (budget ≤ bound − reserve, §9).
- Store absent: pure `WindowedView` truncation with the omission marker, no pointer (footer's
  "not saved" variant).

This also fixes the v1 stacking wart where the in-tool 30k budget equaled the declared Level-1
30k bound, so Level 1 could persist an *already-truncated* text as if it were the full output.

## 8. Non-Goals

- **No token-accurate accounting**: byte measurement + the 4 bytes/token estimate, consistent
  with existing Level 1/2 and the budget plan.
- **Read's `Unbounded` exemption stays**: the counterpart of Hermes's `read_file: inf` guard
  against the persist→read→persist loop.
- **No cross-session artifact reuse**: artifacts live in the session directory and follow the
  session lifecycle (Hermes's global `cache/web` needs a separate GC policy — not worth it).
- **No browser-snapshot-style mechanism**, no diff/dedup snapshots.
- **WebSearch untouched** (already metadata-only + default 5 results).
- **No readability-style HTML main-content extraction in this iteration** — it is the named
  unlock for flipping `Auto`'s HTML arm to Windowed, tracked in §12, not scoped here.

## 9. Invariants (each held by construction, not by convention)

| Invariant | How it holds |
|---|---|
| Windowed output is never re-persisted by Level 1 | Effective inline budget at every offload call site is capped at `resolve_persistence_threshold(tool_bound) − FOOTER_RESERVE`; a windowed render is therefore always under the tool's Level-1 threshold. (v1 instead *claimed* "15k < 50k" while shipping a 500k-capable `inline_bytes` param, and mis-cited the prefix-only tag guard as a backstop for a suffix footer — both caught in review.) |
| Windowed output still counts toward Level 2 | The reference block is a *suffix*; `is_content_already_persisted` (prefix check) stays false for windowed inline text, so Level 2 accounts for it and may evict it on small-window models (eviction renders the `REFERENCE_BUDGET` window of it; the footer path usually survives in the tail segment — acceptable, rare). |
| Pointer and artifact always describe the same bytes | Window and footer computed from the exact in-memory string that is atomically published (tmp + rename; content-addressed names for WebFetch); no read-back, no AlreadyExists-keeps-old-bytes hazard. |
| Suggested Read call always succeeds | 400-byte hard-wrap before window+persist ⟹ `limit=200` slices ≈ ≤ 80KB, under Read's 25k-token slice cap and 256KB full-read cap. |
| Prompt-cache stability | Level 2's byte-identical replay of cached `replacements` unchanged; windowed output is fixed at result-generation time; micro-compact placeholder text unchanged (§6 skips instead of rewriting). |
| resume/fork | `ContentReplacementRecord` layout unchanged; seeding untouched. Stored replacement strings are replayed as data — old-format strings from pre-change transcripts coexist with new renders without re-rendering. |
| Budget pass can never abort a turn | Missing snapshot/ModelInfo → fixed 200k fallback (§5); missing store → window-without-pointer (§3), never an error. |

## 10. Test Plan

- `tool_result_offload.test.rs` (companion-file convention):
  - **Reconstruction property** (the core one): for arbitrary content,
    `head + artifact.lines[omitted_start_line-1 ..= omitted_end_line-1] + tail` reconstructs the
    wrapped text exactly — no gap, no overlap. Run over: CJK/emoji/`─` at the 75% cut and tail
    start (UTF-8 boundary class has shipped panics before), head ending mid-line (snap skipped),
    single-line >100KB input, empty content, CRLF content, content exactly at budget (not
    windowed).
  - `omitted_start_line == count('\n' in head) + 1` in both snap cases.
  - `scaled_per_message_bytes` table: 32_768 → 39_321; 65_536 → 78_643; 200_000 → 200_000
    (clamped); 1_000_000 → 200_000; 0/None-window → default fallback.
  - `InlineBudget::try_new` rejects ≤ 0 (config keeps default); `from_request` clamps.
  - Reference render length ≤ `REFERENCE_BUDGET + FOOTER_RESERVE`; Level-2 convergence test at
    the 16_000 floor (replacements must free enough to converge).
- Offload degradation: store = None → "not saved" footer, no error; store write failure → same.
- Atomic publish: concurrent same-key offloads → every returned footer points at a complete file.
- WebFetch integration (wiremock): 3k page (zero side-query, zero persistence); 40k page
  (windowed + artifact exists + footer format + suggested Read succeeds); same URL re-fetched
  with **changed body** → new artifact name, old artifact untouched; page with data-URIs
  (defusing); single-line 300KB JSON (wrap → Read-navigable); preapproved 80k markdown →
  verbatim (no window); `inline_bytes=500_000` request → effective 49_000, output passes
  `bound_text_for_model` unchanged.
- micro-compact: pointer-bearing results (prefix and suffix forms) survive; others clear;
  placeholder bytes unchanged from today.
- Bash: full output persisted via offload seam; inline window shows tail errors
  (`cargo build`-style output); store-None falls back to pointer-less window.

## 11. Deletion List (no back-compat)

- `web.rs`: Stage 4 becomes config-dispatched; `WEB_FETCH_EXTRACT_SYSTEM` / `extract_guidelines`
  used only under the Llm arm; `persist_binary_content` + the `coco-web-fetch` temp dir deleted
  (kept only as the binaries-without-store fallback); the silent Stage-3 truncation deleted;
  `prompt` requiredness dropped outside the Llm arm.
- `tool_result_storage.rs`: head-only `render_persisted_reference` deleted;
  `PersistedToolResult.preview/has_more` reshaped into head/tail segments.
- `bash.rs`: `truncate_output` deleted (offload seam replaces it).
- Vocabulary rename per §3.1 (mechanical workspace-wide).
- Stale docs corrected: `core/tool-runtime/CLAUDE.md`, `core/tools/CLAUDE.md`,
  `services/compact/CLAUDE.md` (old `max_result_size_chars()` / `i64::MAX` sentinel / "inert by
  default" / Bash temp_dir stub), and `app/query/CLAUDE.md`'s stale `QueryEngineConfig.context_window`
  row.

## 12. Open Questions

1. The 30% factor in `scaled_per_message_bytes` is lifted from Hermes; coco's system prompt +
   tool schemas take a larger window share — drop to 25%?
2. Telemetry to gate flipping `Auto`'s HTML arm to Windowed: record post-window Read frequency
   on artifact files + windowed-result token share per turn. Ship gate, not a nice-to-have.
3. Does the `extraction = llm` arm stay long-term, or get a deletion milestone once §12.2
   telemetry settles `Auto`?
4. Should Level-2 eviction skip windowed results below some size to avoid pointer-to-pointer
   renders entirely (currently rare and acceptable)?

## 13. Adversarial Review Record (2026-07-10)

Six independent lenses (architecture, Rust idioms, logic/math, cache-replay, product/token
economics, ops/failure modes), 44 agents total; every blocker/major finding was independently
re-verified by an adversarial refuter before acceptance. Confirmed findings and their
resolutions, all incorporated above:

| # | Finding (confirmed) | Resolution |
|---|---|---|
| 1 | `omitted_start_line = head lines + 2` off by one; pointer skipped a never-shown line (found independently by 5 lenses) | Formula corrected to `count('\n')+1`, both-endpoint footer, reconstruction property test (§3, §10) |
| 2 | URL-keyed `create_new` + AlreadyExists served stale artifacts once page content changed; plus partial-file race under concurrent fetches | Content-addressed names + atomic tmp+rename publish + footer computed from written string (§3, §4.4) |
| 3 | Windowed output could exceed WebFetch's 50k Level-1 threshold (`inline_bytes` up to 500k) → double-persist; the claimed `is_content_already_persisted` backstop is prefix-only and cannot fire on a suffix footer | Effective budget capped at `threshold − FOOTER_RESERVE` at every call site; tag layout made normative; Level-2 accounting of windowed output made intentional (§3, §4.3, §9) |
| 4 | §6 recoverable-clearing was a dead mechanism: records only exist for Level-2 ids, whose projection replay overwrites any placeholder; no path field; no dependency route from compact to the store | Replaced with "skip pointer-bearing results" via new `is_pointer_bearing` predicate — simpler and actually reachable (§6) |
| 5 | §5 wiring cited a nonexistent `QueryEngineConfig.context_window`; real source is the live model-runtime snapshot (Option, time-varying) | Rewired to `resolved_context_window()` with explicit 200k fallback and a stated time-variance contract (§5) |
| 6 | OQ1's premise false: Read has no per-line truncation — it hard-errors on oversized slices; single-line artifacts (minified JSON) made the suggested Read fail outright | 400-byte hard-wrap of artifact text before window+persist; Read caps now cleared by construction (§3, §4.2) |
| 7 | `preapproved_verbatim` regressed from ~100k verbatim to a 15k window — the flagship docs case got worse | Dedicated `preapproved_verbatim_budget` (default 100_000) preserves today's behavior (§4.3) |
| 8 | Hermes's evidence doesn't transfer to coco's html2text noise (verifier: partial) — head window catches nav chrome on scraped HTML | `Auto` default: Windowed for clean content types, Llm kept for text/html; telemetry ship-gates the flip (§4) |
| 9 | Level-1/Level-2 reference render budget unspecified; generous windows would break Level-2 convergence at the 16k floor | Fixed `REFERENCE_BUDGET` (4k) for all reference renders, convergence test (§3, §10) |
| 10 | Bash §7 premise false (full output IS in memory); 30k-in-tool == 30k-bound stacking persisted truncated text as "full output" | Bash routed through the offload seam; `truncate_output` deleted (§7) |
| 11 | Store-unavailable path unspecified; today's Level-1 precedent can hard-error | `offload_windowed` is a free fn over `Option<&ToolOutputStore>` with a model-visible "not saved" footer; never an error (§3, §9) |
| 12 | Chars→Bytes rename was half-done (new byte quantities named `*_chars`) | Full vocabulary rename table (§3.1) |
| 13 | `InlineBudget(usize)` + silent clamp inverted misconfiguration handling vs the repo's config convention (verifier: minor) | i64 newtype, `try_new` (config, reject) vs `from_request` (model param, clamp) (§3) |
| 14 | Minor batch: ArtifactKey URL parsing didn't belong in tool-runtime; cache worst case silently grew 12.8MB→256MB; `WebFetchExtraction` enum ownership implied a config→tools inversion; Windows reserved device-name stems (`con.`, `nul.`) passed the slug filter | Caller-computed `Named` keys (runtime validates only); cache re-scoped to rendered results (~6.4MB); enum owned by coco-config; mandatory literal prefix (`url-`) neutralizes reserved stems (§2, §3, §4.6) |
