# Tools System (incl. structure-aware grep): jcode vs coco-rs

Source-level comparison of the two harnesses' tool layers, with emphasis on
code-search ergonomics. Every claim below is anchored to source read on both
sides. The agentgrep *engine* is an external crate (`agentgrep`,
`jcode/Cargo.toml:212`, git tag `v0.1.2`) not vendored in the jcode checkout;
its internals were verified against a clone probed at `/tmp/agentgrep_probe`
(`src/structure.rs`, `src/search.rs`), and the file:line refs to that engine
are tagged `[engine]`.

---

## jcode approach

### Tool trait — minimal, untyped, single-tier errors

`jcode-tool-core/src/lib.rs:71-93`: `Tool` is `name() -> &str`,
`description() -> &str`, `parameters_schema() -> Value`, and
`async execute(Value, ToolContext) -> anyhow::Result<ToolOutput>`, plus a
default `to_definition()`. Input is an untyped `serde_json::Value`; there is no
validation phase, no permission phase, and no concurrency/read-only/destructive
metadata on the trait. Errors are `anyhow::Result` everywhere
(`lib.rs:1` `use anyhow::Result`) — a single-tier error model with no
classification or retryability surface.

`ToolContext` (`lib.rs:29-68`) carries `session_id` / `message_id` /
`tool_call_id`, `working_dir`, a `stdin_request_tx`, a
`graceful_shutdown_signal`, and an `execution_mode` (`AgentTurn` / `Direct`);
`resolve_path` joins relative paths against `working_dir`. `ToolOutput`
(`jcode-tool-types/src/lib.rs:1-58`) is `{ output: String, title, metadata:
Value, images: Vec<ToolImage> }`.

### Registry, dispatch, and the harness-level overflow guard

`src/tool/mod.rs`: `Registry` is `Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>`
plus a shared `SkillRegistry` and a per-clone fresh `CompactionManager`. Tool
definitions are sorted by name "critical for prompt cache hits"
(`mod.rs:337`). `resolve_tool_name` (`mod.rs:367-386`) maps Claude-Code OAuth
tool names (`file_grep`→`grep`, `shell_exec`→`bash`, `task`→`subagent`) to
internal names. Session policy is a flat allow/deny set checked inside
`execute()` (`mod.rs:485-494`).

The distinctive piece is **`guard_context_overflow`** (`mod.rs:544-629`), run
on *every* tool output from `execute()` (`mod.rs:527`). It reads the live
`CompactionManager::token_budget()` and `effective_token_count()`, estimates
the output's tokens (chars/4), and truncates when
`projected > 0.90 * budget` (`CONTEXT_GUARD_THRESHOLD`, `mod.rs:475`) OR
`output_tokens > 0.30 * budget` (`SINGLE_OUTPUT_MAX_FRACTION`, `mod.rs:479`).
It keeps the *prefix* and appends either an instructive "OUTPUT TRUNCATED … use
more targeted queries" notice (`mod.rs:598-610`) or a hard "CONTEXT LIMIT
REACHED" message when nearly full (`mod.rs:613-620`). This is generic,
proportional, every-tool truncation that runs in the harness and is aware of
*how full the context currently is*.

### `batch` meta-tool

`src/tool/batch.rs`: runs up to `MAX_PARALLEL = 10` (`batch.rs:10`)
heterogeneous sub-tool-calls concurrently via `FuturesUnordered`, emitting
`BatchProgress` bus events as each completes. `normalize_batch_input`
(`batch.rs:104-147`) repairs common LLM mistakes — `name`→`tool`,
`arguments`/`args`/`input`→`parameters`, flat params→nested under
`parameters`. The model can run *different* tools in one tool_use block, and
malformed sub-calls are auto-corrected before dispatch.

### Other built-ins

`edit` (`src/tool/edit.rs`) is literal `old_string`/`new_string` replace with an
occurrence-count guard, flexible-match diagnostics, a compact `42-`/`42+` diff,
and a post-edit `Bus::FileTouch` event for swarm coordination. `glob`
(`src/tool/glob.rs`) is a parallel `ignore::WalkBuilder` walk, mtime-sorted,
100-cap — functionally identical to coco-rs's GlobTool. `codesearch`
(`src/tool/codesearch.rs`) is a thin client to Exa's hosted MCP, not a local
index.

### agentgrep — the "structure-aware grep" headline

agentgrep is jcode's primary code-navigation surface, with four modes
(`src/tool/agentgrep.rs:181-234`): `grep` (default), `find` (rank file
names/paths), `outline` (one file's structure), and `trace`/`smart` (a DSL
relationship search, e.g. `subject:auth relation:rendered support:ui`). A
`path` pointing at a file is auto-narrowed to its parent dir + an exact-filename
glob (`resolved_search_scope`, `args.rs:8-40`).

**Structure in returns (engine + render).** The engine extracts a per-file
symbol skeleton with **hand-rolled per-language regex/heuristics**, *not*
tree-sitter — `extract_rust` / `extract_ts_js` / `extract_python` /
`extract_markdown` / `extract_generic` `[engine structure.rs:85-181]`, plus an
`infer_role` path-to-purpose heuristic `[engine structure.rs:62]`. To bound CPU
on dense files it *skips* structure extraction entirely past
`DENSE_MATCH_SKIP_STRUCTURE_THRESHOLD = 24` matches `[engine search.rs:13,534,590]`
and caps grouping (`DENSE_GROUPS_LIMIT = 8`, `OTHER_SYMBOLS_LIMIT = 4`)
`[engine search.rs:11,14]`. `render_grep_file` (`render.rs:80-166`) prints, per
file, a `symbols: N total, M matched, K other` header (`render.rs:88-97`),
groups matches under their enclosing `- {kind} {label} @ {start}-{end}`
(`render.rs:120-132`), and an `other:` summary of unmatched symbols
(`render.rs:146-165`). So a single `grep` call yields each hit file's symbol
skeleton. `compact_rendered_match_line` (`render.rs:175-218`) centers a 240-char
window on the literal-match offset (`render.rs:181-192`), and non-code
(json/yaml/markdown/text) files are capped at 3 match lines/file
(`MAX_NON_CODE_MATCH_LINES_PER_FILE`, `render.rs:5,168-173`).

`render_find_file` (`render.rs:242-266`) emits per ranked file a `role:` + `why:`
+ token-budgeted `structure:` listing. `render_outline_output`
(`render.rs:268-304`) is a standalone file-skeleton renderer
(`language` / `role` / `lines` / `symbols` + the structure list).

**Exposure-aware adaptive truncation.** For `outline` / `trace` / `smart`
**only** (`maybe_write_context_json` gate, `context.rs:7`), jcode writes a
transient harness-context JSON and passes its path to the engine. To build it
(`build_harness_context`, `context.rs:28-106`) it replays the whole session
transcript via `Session::load` (`context.rs:32`) and mines prior tool
exposures:
- `read` calls → `known_region` + `known_file` (`collect_read_exposure`, `context.rs:155-205`),
- prior agentgrep results → parsed back out of rendered text (`collect_agentgrep_exposure`/`collect_trace_exposure`),
- `bash` commands → parses `sed -n A,Bp`, `cat`, `git show X:f`, `git diff`, `path:line:` stdout (`collect_bash_exposure`).

Each entry gets four confidences, then `apply_exposure_tuning`
(`context.rs:726-776`) decays them by **recency** (position-ratio:
×1.0/×0.88/×0.72), **compaction cutoff** (entries before the cutoff slashed to
×0.42, tagged `compacted_history`, `context.rs:743-748`), and **file
freshness** (`file_freshness_multiplier`, `context.rs:778-810` — current mtime
vs exposure time, ×0.92 if changed ≤5s ago down to ×0.25 if >6h). The engine
then collapses high-`prune_confidence` regions the model already holds to
one-line references. Tests at `agentgrep_tests.rs:684-752` exercise the
compaction penalty and mtime-change detection.

**Critical scope correction (verified):** this exposure-aware collapsing does
**not** apply to plain `grep`. `build_grep_args` (`args.rs:42-59`) takes no
`context_json`; only `build_outline_args` (`args.rs:87-100`) and
`build_smart_args_and_query` (`args.rs:102-134`) thread it. So jcode suppresses
already-seen code in its `trace`/`smart`/`outline` *navigation* tools, not in
grep.

---

## coco-rs approach

### Tool trait — rich, typed, telemetry-grade errors

`core/tool-runtime/src/traits.rs`: `Tool` is a typed trait with associated
`type Input` (`Deserialize + JsonSchema`) and `type Output`. Beyond
`execute(Input, &ToolUseContext) -> Result<ToolResult<Output>, ToolError>` it
exposes a wide lifecycle surface: `validate_input() -> ValidationResult`
(with an `error_code`), `check_permissions() -> ToolCheckResult`,
`is_read_only` / `is_concurrency_safe` / `is_destructive`,
`interrupt_behavior`, `is_search_or_read_command() -> SearchReadInfo`,
`max_result_size_bound() -> ResultSizeBound`,
`render_for_model() -> Vec<ToolResultContentPart>`, `modify_context_after`,
and `is_enabled(ctx)` (the `Feature` gate). Errors are the 3-tier model
(snafu / thiserror / anyhow by layer); `ToolError` + `ValidationResult{error_code}`
give the schema/permission failure classification jcode's `anyhow` model lacks,
and they fire *before* execution.

### ToolUseContext already holds the raw materials jcode mines

`core/tool-runtime/src/context.rs`: `messages: Arc<Vec<Arc<Message>>>` (line
199 — the full conversation), `file_read_state: Option<Arc<RwLock<FileReadState>>>`
(line 505 — every read file/range + mtime), `cwd_override` (worktree
isolation, line 429), plus all callback handles
(Agent/Hook/Mcp/Task/Mailbox/Permission). `FileReadState`
(`core/context/src/file_read_state.rs:46-52`) stores `content`, `mtime_ms`,
`offset`, `limit`, and the literal `read_input_ranges`. **coco-rs already holds
at the tool-call seam exactly the inputs jcode's agentgrep replays the
transcript to recover** — it simply does not use them to shape search output.

### Registry & streaming executor

`core/tool-runtime/src/registry.rs`: `ToolRegistry::definitions(ctx)` filters
by `is_enabled` per turn (and is the deferred-tool / ToolSearch promotion seam).
The streaming executor (`executor_streaming.rs:39-51`) implements the
safe-concurrent / unsafe-queued split: safe (read-only, `is_concurrency_safe`)
plans are `tokio::spawn`ed onto a `JoinSet` and run *during* the model stream;
once any unsafe plan is fed, all subsequent plans hold for serial
`commit_flush`. Concurrency caps at `COCO_MAX_TOOL_USE_CONCURRENCY` (default
10). **There is no `batch` meta-tool** — parallelism comes from the model
emitting multiple tool_use blocks the executor batches, which is the
Claude-Code-faithful design.

### GrepTool — in-process ripgrep core, flat output, no structure

`core/tools/src/tools/grep.rs`: built directly on `grep-regex` /
`grep-searcher` / `ignore` (no `rg` subprocess, `grep.rs:53-59`). It honors
`output_mode` (content / files_with_matches / count), `-A/-B/-C`, `-n`, `-i`,
`glob`/`type`, `multiline`, and `head_limit`/`offset` pagination — a faithful
port of Claude Code's GrepTool wire schema and output format. `is_concurrency_safe = true`
(`grep.rs:344-346`), runs in `spawn_blocking` under a 20s timeout, checks
`ctx.cancel` per file, applies VCS excludes + file-read-ignore patterns via
`check_permissions` (`grep.rs:379-395`), caps the in-memory match count at
100K (`MAX_IN_MEMORY_MATCHES`, `grep.rs:81`), sorts `files_with_matches` by
mtime, and sets `max_result_size_bound = Chars(20_000)` (`grep.rs:349-351`).

The match-line plumbing is allocation-frugal: `GrepMatchLine.file_path` is an
`Arc<str>` shared across a file's matches (`grep.rs:107-116`). But
`format_content` (`grep.rs:853-903`) emits flat `path:line:content` /
`path-line-content` (`grep.rs:871-878`) with **zero structure** — no symbol
header, no per-file skeleton. And `decode_sink_bytes` (`grep.rs:175-179`) calls
`truncate_to_width(trimmed, 500)` (`MAX_COLUMN_WIDTH = 500`, `grep.rs:78`),
which **hard-cuts at the first 500 bytes** (`grep.rs:182-191`) *before any
formatter sees the line*; the `ContextAwareSink` stores only `line_content`
(`grep.rs:128-140`), discarding the match position.

### Structure/symbol awareness exists — but not at the model's search seam

Two subsystems extract code structure:
- `utils/symbol-search` (`extractor.rs:3,24,49`) uses **tree-sitter-tags**
  (`TagsContext::generate_tags`) to produce `SymbolTag{name,kind,line,is_definition}`
  — but per its CLAUDE.md it powers only the TUI `@#SymbolName` mention feature,
  and it is **not** a `core/tools` dependency (`grep -n symbol-search core/tools/Cargo.toml`
  returns nothing).
- The `retrieval` crate (`retrieval/src/`) has AST tags, a PageRank RepoMap
  (`repomap/{pagerank,renderer}.rs`; `renderer.rs:51-75` groups RankedSymbols
  by file, sorted by max PageRank), BM25 (`search/bm25.rs`), hybrid + RRF
  fusion, a vector store, and rerankers, all behind `RetrievalFacade`
  (`facade.rs:180`) with `search()` (`facade.rs:277`) and `generate_repomap()`
  (`facade.rs:313`).

**But the retrieval crate is not reachable from the agent's tool path.**
`grep -rn "RetrievalFacade|coco_retrieval|generate_repomap|RetrievalTool"
core/tools/src app/query/src` returns **zero hits**; `retrieval` is absent from
both `core/tools/Cargo.toml` and `app/query/Cargo.toml`; and `RetrievalEvent`
is "intentionally isolated" from `CoreEvent` (`retrieval/CLAUDE.md:84`). The
model has no callable path to it. So coco-rs's structure-awareness is real
engineering that lives *beside*, not *inside*, the agent's search path.

### Output / context governance

Per-tool `max_result_size_bound()` (Grep 20K, Bash/PowerShell 30K, Glob 100K)
drives Level-1 persistence in `app/query/src/tool_outcome_builder.rs:167-192`:
when the rendered output exceeds
`resolve_persistence_threshold(tool.max_result_size_bound())` (a *fixed* value,
`tool_outcome_builder.rs:176-181`), the full text is written via
`persist_to_disk` and replaced by a `<persisted-output>` reference. A Level-2
per-message aggregate budget (`apply_tool_result_budget`) runs in `app/query`.
The `BudgetTracker` (`app/query/src/budget.rs`) only emits
`Continue`/`Stop`/`Nudge` from `consumed_tokens` vs `max_tokens`
(`budget.rs:22-130`) — it **never shapes a single tool's inline window**. So a
20K grep is emitted identically at 10% and 95% context usage. This is the
analog of jcode's `guard_context_overflow`, but per-tool-threshold +
persist-to-disk rather than live-budget proportional truncation.

---

## Head-to-head comparison

| Concern | jcode | coco-rs | Edge |
|---|---|---|---|
| Tool trait | untyped `Value` in, `anyhow` out, no lifecycle phases (`tool-core/lib.rs:71-93`) | typed `Input`/`Output`, `validate_input`/`check_permissions`/concurrency metadata, `ToolError` classification (`traits.rs`) | **coco-rs** |
| Grep core | external `agentgrep` engine + hand-rolled `grep.rs` walker | in-process `grep-regex`/`grep-searcher`, `Arc<str>` paths, 100K cap (`grep.rs`) | **coco-rs** (portable, allocation-frugal) |
| Structure in grep returns | symbol skeleton per hit file (`render.rs:80-166`) | flat `path:line:text` (`grep.rs:853-903`) | **jcode** |
| Exposure-aware search collapse | yes, but only `trace`/`smart`/`outline` (`context.rs`; not grep — `args.rs:42-59`) | none; `FileReadState` is same-turn Read self-dedup only | **jcode** (where applicable) |
| `find` filename ranking | single tool with role + structure preview (`render.rs:242-266`) | split across Glob (no ranking) + non-model BM25 | **jcode** (niche) |
| `outline` single-file skeleton | model-callable (`render.rs:268-304`) | none model-facing (LSP `documentSymbol` is double-gated on a running server) | **jcode** (niche) |
| Heterogeneous parallel calls | explicit `batch` (≤10), input-repair (`batch.rs:104-147`) | model emits parallel safe tool_use blocks; executor batches | **jcode** (helps weak models); coco-rs is CC-faithful |
| Long match-line truncation | match-centered 240-char window (`render.rs:175-218`) | hard-cut at byte 500 (`grep.rs:182-191`) — drops far matches | **jcode** |
| Large-output truncation | live-budget proportional, instructive notice (`mod.rs:544-629`) | fixed char caps + persist-to-disk (`tool_outcome_builder.rs`) | **mixed** (see below) |
| Recoverability of trimmed output | prefix + notice; rest is gone (`mod.rs:594-621`) | full text persisted to disk, retrievable later | **coco-rs** |
| Per-call cost of search shaping | replays + re-parses whole transcript per outline/trace call (`context.rs:28-106`) | no transcript replay; tight in-process grep | **coco-rs** |

**The two genuinely novel jcode mechanisms** are (1) the symbol skeleton in
grep/find/outline returns, which lets the model locate a function and infer
file shape from a single call (often skipping a follow-up read), and (2)
exposure-aware region collapse in its navigation tools, which de-duplicates
*search* output against the model's working memory — a more aggressive
token-saver than per-tool size caps. Both, on the engineering merits, are worth
adopting for code-navigation UX. Note the second is *not* applied to plain grep
on jcode's side, so the natural coco-rs home is a future navigation tool, not
GrepTool.

---

## Where coco-rs already matches or wins

- **Typed tool contract + telemetry-grade errors.** `validate_input ->
  ValidationResult{error_code}`, `check_permissions -> ToolCheckResult`, and
  `ToolError` with `StatusCode` classification let coco-rs reject bad
  schema/permissions *before* execution and classify failures for telemetry.
  jcode's `execute(Value) -> anyhow::Result` (`tool-core/lib.rs:83`) conflates
  everything into one untyped error tier.

- **Permission pipeline + read-ignore enforcement.** GrepTool's
  `check_permissions` (`grep.rs:379-395`) refuses ignored roots via the unified
  in-process `coco-file-ignore` path (no `git check-ignore` subprocess — a
  documented design win). jcode tools have no per-tool permission method; ignore
  handling is only the `ignore` crate's default gitignore behavior.

- **In-process ripgrep core (no subprocess).** coco-rs uses `grep-regex` /
  `grep-searcher` directly with shared `Arc<str>` paths (`grep.rs:107-116`), a
  100K-match cap, per-file cancellation, and a 20s timeout. For the plain-grep
  path this is cleaner and more OS-portable than jcode's hand-rolled walker.

- **Structure extraction is actually present.** The premise "coco-rs lacks
  structure-awareness" is false: `utils/symbol-search` (tree-sitter-tags) and
  the `retrieval` crate (AST tags + PageRank RepoMap + BM25 + vector + RRF +
  rerankers) are a *deeper* code-intelligence stack than agentgrep's
  regex-heuristic outline. coco-rs's gap is purely *wiring it into a
  model-facing tool*, not building it.

- **Retrievable persisted output beats lossy truncation.** Level-1 persistence
  (`tool_outcome_builder.rs:167-192`, `<persisted-output>` + on-disk full text)
  means a giant grep/bash result is *recoverable* later. jcode's
  `guard_context_overflow` (`mod.rs:594-621`) permanently keeps only the prefix
  + a notice — strictly more lossy for workflows that need the full result.

- **Worktree-aware paths + LSP on edits.** coco-rs tools honor
  `ctx.cwd_override` for worktree-isolated subagents (`grep.rs:419-435`), and
  Edit/Write/NotebookEdit fire `textDocument/didSave` + file-history
  checkpoints. jcode's edit emits a swarm `FileTouch` bus event but has no LSP
  notify or undo-checkpoint.

- **Engine internals are not auditable on the jcode side.** agentgrep's
  tree-sitter-free regex structure parser and trace-DSL relevance scoring live
  in an external git crate (`Cargo.toml:212`), and the "saves context a lot"
  magnitude is benchmarked nowhere in source. coco-rs's stack is fully
  in-repository.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only confirmed/nuanced suggestions are carried forward; nuanced ones fold in
their correction.

### R1 — Add an opt-in per-file symbol skeleton to GrepTool content output (confirmed)

**Why.** jcode's `render_grep_file` (`render.rs:80-166`) gives the model each
hit file's symbol skeleton (`symbols: N total, M matched`, matches grouped
under `{kind} {label} @ start-end`) in one call, so it can locate the right
function and infer file shape without a follow-up read. coco-rs's
`format_content` (`grep.rs:853-903`) emits flat `path:line:content`; the
tree-sitter machinery exists (`utils/symbol-search/src/extractor.rs`,
`retrieval/src/tags`) but is not a `core/tools` dependency and is wired to the
TUI `@#mention` feature.

**Concrete change.** In `core/tools`, add an opt-in flag to `GrepTool` (e.g. a
`structure: bool`, default off). When set, for `content`/`files_with_matches`
results, run a symbol extractor on each hit file, map each match line to its
enclosing symbol, and prepend a `symbols: N total / M matched` header with
`- {kind} {label} @ s-e` grouping — reusing `retrieval/src/repomap/renderer.rs`
grouping style.

**Crucial mechanism note (from the verifier):** jcode's engine uses *hand-rolled
per-language regex* `[engine structure.rs:85-181]`, which is far cheaper than
coco-symbol-search's `TagsContext::generate_tags` tree-sitter parse
(`extractor.rs:49`). If coco-rs wires in the tree-sitter extractor it MUST
adopt jcode's per-file CPU guards: skip extraction past a dense-match threshold
(`DENSE_MATCH_SKIP_STRUCTURE_THRESHOLD = 24` `[engine search.rs:13]`) and cap
grouping (`DENSE_GROUPS_LIMIT = 8`, `OTHER_SYMBOLS_LIMIT = 4`
`[engine search.rs:11,14]`), bounded by `head_limit` and the existing 20s
`spawn_blocking` budget. A lighter alternative is to port jcode's
regex-heuristic approach for the common languages, which sidesteps the
tree-sitter cost concern entirely.

**Impact** high · **Effort** medium · **Risk** per-file parse CPU on large
result sets; mitigate with the dense-skip guard above. Must be additive/opt-in
so the byte-for-byte GrepTool prompt + schema parity (relied on for cache hits)
is preserved when the flag is off. Respects all documented non-goals.

### R2 — Exposure-aware search collapse, scoped correctly (nuanced)

**Why (and the correction).** jcode's exposure mining is real
(`build_harness_context`, `context.rs:28-106`; `apply_exposure_tuning`
`context.rs:726-776` with the ×0.42 compaction penalty; `file_freshness_multiplier`
`context.rs:778-810`), but — verified — it is consumed **only** by
`outline`/`trace`/`smart`, never plain grep (`build_grep_args`, `args.rs:42-59`,
takes no context JSON). So the analyst's framing "agentgrep does adaptive
truncation *of grep*" is wrong on the target surface. The natural home for
exposure-aware collapse in coco-rs is therefore a future
structure-navigation/RetrievalTool (R3), **not** GrepTool.

coco-rs's gap is still real: `FileReadState`
(`file_read_state.rs:46-52`, content + mtime + ranges) and
`ToolUseContext.messages` (`context.rs:199`) carry the inputs, but neither
shapes any search output; the only analog
(`read.rs` `file_unchanged`) is same-turn identical-range Read dedup with no
cross-tool mining and no grep effect.

**Concrete change (narrowed).** Add a `core/tools` helper
`recently_seen_ranges(ctx) -> Map<path, Vec<(start,end,freshness)>>` reading
`ctx.file_read_state` only (no transcript replay — coco-rs treats
`ctx.messages` as immutable and should not pay jcode's per-call `Session::load`
tax). If applied to Grep at all, collapse **only context lines** whose
`(file,line)` falls inside a *fresh*-mtime seen-range into a
`…(already shown @ path:line)…` reference — **never the match line itself**.
Decay by mtime-vs-read-time like `file_freshness_multiplier`. Strict no-op when
`file_read_state` is `None` (SDK/headless).

**Impact** high · **Effort** high · **Risk** hiding a needed line; mitigated by
never collapsing match lines and only on high freshness. Defer behind R3.
Respects all documented non-goals.

### R3 — Expose the retrieval RepoMap / hybrid search as a model-callable tool (confirmed; highest value)

**Why.** jcode's `find` (ranked file paths + role + structure preview,
`render.rs:242-266`) and `trace`/`smart` give the model a relevance-ranked,
structure-first repo map from one call. coco-rs already ships a *superior*
engine — `RetrievalFacade::search` (`facade.rs:277`) and `generate_repomap`
(`facade.rs:313`), BM25 + RRF fusion + PageRank RepoMap — but
`grep -rn RetrievalFacade core/tools/src app/query/src` returns zero hits, there
is no `RetrievalTool`, and `RetrievalEvent` is isolated by design
(`retrieval/CLAUDE.md:84`). The model cannot reach it.

**Concrete change.** Add a `RetrievalTool` in `core/tools` (read-only,
concurrency-safe) that calls `RetrievalFacade::search` / `generate_repomap`,
gated on `Feature::Retrieval` (`is_enabled`) AND `config.enabled`, injected via
a callback handle mirroring `AgentHandle`/`McpHandle` so `core/tools` keeps no
hard dep on the retrieval crate. Surface a `find`-like ranked-files mode and a
repomap mode.

**Impact** high · **Effort** medium · **Risk** retrieval needs an index (off by
default) — degrade gracefully on `RetrievalErr::NotReady` by instructing
fallback to Grep. Must stay opt-in so default (no-embeddings) builds are
unaffected. Adds a small callback handle to `ToolUseContext`. Conflicts with no
documented non-goal — this closes the navigation-UX gap with infrastructure
coco-rs already ships.

### R4 — Make inline tool-output truncation budget-aware (confirmed)

**Why.** jcode's `guard_context_overflow` (`mod.rs:544-629`) trims any tool's
output against the live remaining budget (>0.90 projected, >0.30 single-output
caps), so a large result late in a session is trimmed harder than the same
result early. coco-rs uses fixed char caps (Grep `Chars(20_000)`,
`grep.rs:349-351`) + Level-1 persist-to-disk; `tool_outcome_builder.rs:176-181`
compares against a *fixed* threshold, and `BudgetTracker` (`budget.rs:22-130`)
only emits Continue/Stop/Nudge — neither scales a single tool's inline window
to context fullness.

**Concrete change (placed precisely).** In `app/query/src/tool_outcome_builder.rs`
at the resolved-threshold site, additionally clamp the *inline preview* to
`min(per-tool bound, live-remaining-budget share)`, reading the budget snapshot
already available in `app/query`. **Keep persist-to-disk for the full text** —
that retrievability advantage over jcode must not be sacrificed; only the inline
portion shrinks when the window is nearly full. Avoid double-counting with the
Level-2 `apply_tool_result_budget` pass (feed this as an input to it) and with
micro-compact (`services/compact/src/micro.rs` clears *old* tool results — a
different axis). Mirror jcode's 0.90/0.30 conservatism; read the snapshot (not
wall-clock) to stay deterministic for tests.

**Impact** medium · **Effort** medium · **Risk** double-counting; place as an
input to the existing budget pass. Pure query-layer change; tools untouched.
Respects all documented non-goals.

### R5 — Match-centered long-line compaction for Grep content lines (confirmed)

**Why.** jcode's `compact_rendered_match_line` (`render.rs:175-218`) centers a
240-char window on the literal-match byte offset
(`line.find(&args.query)`, `render.rs:188-190`; regex falls back to offset 0,
`render.rs:181-182`), and `non_code_match_cap` (`render.rs:168-173`) caps
json/yaml/markdown/text at 3 match lines/file. coco-rs's
`decode_sink_bytes` → `truncate_to_width(s, 500)` (`grep.rs:175-191`) hard-cuts
the first 500 bytes — if the match starts past byte 500 (minified JS, long
JSON), the matched substring is dropped from the displayed line entirely. There
is no non-code per-file cap.

**Concrete change (with the verifier's mechanical narrowing).** The per-match
column is **not** a free `SinkMatch` field: `decode_sink_bytes` truncates the
line to 500 *before* any formatter runs, and `ContextAwareSink` stores only
`line_content` (`grep.rs:128-140`), discarding the position. To center on the
match you must (a) stop truncating in `decode_sink_bytes` and keep the full
line, and (b) recompute the offset against the pattern (or capture
`mat.bytes()` range from `SinkMatch`) at format time, then emit a
match-centered window with a before/after marker. Optionally add a
non-code-extension per-file match cap for content mode. Confine the change to
`content` mode; leave `files_with_matches`/`count` untouched for TS parity.

While here, fix the inaccurate doc-comment on `truncate_to_width`
(`grep.rs:181`): it compares `s.len()` (**bytes**) to `max` but says "character
width" — harmless under `MAX_COLUMN_WIDTH = 500` (bytes ≥ chars, so it only
over-keeps) but the comment is wrong (verifier missed-finding).

**Impact** medium · **Effort** low-medium (not "low" — the offset is not free,
per above) · **Risk** approximate offset for complex/multiline regexes; fall
back to head-truncation when undetermined. Respects all documented non-goals.

### R6 — Surface a per-file `role` hint and a standalone `outline` tool (verifier missed-findings)

**Why.** jcode infers a one-word file purpose from path (`infer_role`
`[engine structure.rs:62]`, surfaced in `render_find_file` `render.rs:245` and
`render_outline_output` `render.rs:271`) — a cheap purpose hint coco-rs has no
analog for in any search/nav output. Separately, jcode's `outline` mode is a
standalone model-callable file-skeleton tool (`render.rs:268-304`); coco-rs has
no single-file outline tool exposed to the model (LSP `documentSymbol` is the
closest but is double-gated on a running LSP server per `core/tools/CLAUDE.md`).

**Concrete change.** Fold both into R1/R3 rather than as separate tools: include
a path-derived `role` line in the R1 skeleton header (a trivial regex-heuristic,
no parsing cost), and expose an `outline` mode on the R3 `RetrievalTool` (or as
a thin GrepTool sub-mode) that returns one file's `language` / `role` / `lines`
/ symbol list — giving the model a structure-first read alternative that does
not depend on a running LSP server.

**Impact** low-medium · **Effort** low · **Risk** `role` heuristic is best-effort
(label it as such). Respects all documented non-goals.

---

## Rejected after adversarial review

No suggestion in the analyst set was fully refuted — all five (M08-S1 through
M08-S5) returned `confirmed` or `nuanced` and are carried above. For
transparency, the one framing that was **corrected rather than dropped**:

- **M08-S2 as originally stated ("agentgrep does harness-level adaptive
  truncation *of grep*") — partially refuted on the target surface.** Verified
  against `maybe_write_context_json` (`context.rs:7`, modes `trace|smart|outline`
  only) and `build_grep_args` (`args.rs:42-59`, no `context_json`): the
  exposure-aware collapse never touches plain grep. The coco-rs *gap* is still
  real (no cross-tool exposure shaping anywhere), so the suggestion survives as
  R2 — but re-homed to a future navigation/RetrievalTool and narrowed to
  context-lines-only, FileReadState-only (no transcript replay), with a strict
  no-op when `file_read_state` is absent. Readers should not implement
  exposure-aware collapse on GrepTool expecting jcode parity; jcode itself does
  not do that.
