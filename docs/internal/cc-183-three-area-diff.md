# coco-rs → Claude Code 2.1.183 — System-Prompt / Tools-Schema / System-Reminder Diff

> Generated 2026-06-26 by a focused 7-agent verification workflow (3 per-area
> deep finders → 3 adversarial verifiers → synthesis), each finding re-checked
> against **current** coco-rs source (commits landed after the broad v2 roadmap
> `cc-catchup-roadmap-v2.md`). Scope is narrowed to the three subsystems the user
> asked about. Source: `/lyz/codespace/analysis_claude_code_v2/claude_code_v_2.1.183/analyze`.

## Executive verdict

The three audited subsystems — **System Prompt**, **Tools Schema/Define**, and
**System Reminder** — are **strongly aligned** with CC 2.1.183. Adversarial
verification turned up **no P0/correctness gaps and only two genuine P1s**, both
verified-absent and tractable. The dominant theme is coco-rs's multi-provider
differentiator: **ToolSearch-promotion safety nets** — non-Anthropic models
(OpenAI/Gemini/compat) receive deferred tool schemas via coco's ToolSearch
*promotion* rather than native `ToolReference`, so they need a recurring
"schemas not loaded, search for them" nudge that Anthropic models don't, plus
inline-schema hints on the deferred-not-loaded error path. The second theme is
**cross-session security framing** — verification *refuted* the v2 roadmap's
claim that the peer permission-laundering guard was missing; it is present
verbatim at `core/system-reminder/src/queue_origin.rs:64`, leaving only an
optional hardening clause.

## Prioritized action table

Excludes refuted / skip-nongoal / already-done. `tool_search_usage_reminder` was
surfaced by both the Tools-Schema (P2 framing) and System-Reminder (P1) areas —
listed once at its canonical P1.

| # | Area | Item | Kind | Pri | Val | Eff | Why it matters (multi-provider lens) |
|---|------|------|------|-----|-----|-----|--------------------------------------|
| 1 | Reminder | Recurring `tool_search_usage_reminder` (distinct from one-shot `deferred_tools_delta`) | new | P1 | high | M | Weak non-Anthropic models get deferred schemas via ToolSearch promotion, not native `ToolReference`; without a turn-cadence re-nudge they conclude a capability is *missing* instead of searching. |
| 2 | Prompt | Coordinator "quote exact words / worker auto-mode sees only its own transcript" bullet | carryover | P1 | med | S | Worker handoff classifier (`classifier.rs:199`) sees only its own messages → coordinator approval is invisible; worker re-asks/blocks. |
| 3 | Reminder | Shared ambient "do not narrate" trailer on delta/memory reminders | new | P2 | med | S | OpenAI/Gemini narrate MCP-disconnect / new-agent-type events to the user; one shared const suppresses it. |
| 4 | Prompt | Env-block git-worktree "do not `cd` to repo root" note | new | P2 | med | S | coco ships `Feature::Worktree` + `AgentWorktreeManager`; without the note a worker corrupts the main checkout. |
| 5 | Tools | Inline input-schema appendix on deferred-not-loaded error | opt | P2 | med | S | Saves a ToolSearch round-trip before correctly-typed args — most valuable where ToolSearch is promoted. |
| 6 | Tools | Read token-cap graceful pagination + `truncated_by_token_cap` | new | P2 | med | M | Whole-file overrun hard-errors; recoverable (msg instructs offset/limit) but weaker models may not self-recover. |
| 7 | Tools | `deferred_tools_delta` 5-state diff / 4-section reminder (RE-ADDED, PENDING-MCP, 30-cap grouping) | carryover | P2 | med | M | Cleaner deferred-pool churn signal for promoted-ToolSearch providers. |
| 8 | Prompt | Adopt rewritten "# Text output" anti-verbosity section | reword | P2 | med | M | Non-Anthropic models map to the 2.1.183 default-branch variant. |
| 9 | Prompt | "You are operating autonomously…" autonomy-append block | new | P2 | med | M | Re-gate on session-kind (`-p`/`--bg`/coordinator-worker), **not** Anthropic model family. |
| 10 | Prompt | Lean/full head swap + "# Harness" lean head + trust gate | new | P2 | low | L | Token optimization; trust list must come from coco model-card flags, not Anthropic ids. |
| 11 | Tools | `eager_input_streaming` wire-field gate (MODEL_CAPS) | new | P3 | low | M | Anthropic-first-party-only perf nicety; field currently dead. |
| 12 | Tools | `mcp_permission_mode_overrides` per-server field | new | P3 | low | S | MCP-permissions UX orthogonal to tool schema. |
| 13 | Prompt | Team/ownership-frame intro variant | new | P3 | low | S | Off-by-default upstream A/B, tied to the lean head coco lacks. |
| 14 | Prompt | Main-thread agent-def `appendSystemPrompt` merge branch | carryover | P3 | low | M | Unreachable until `--agent` main-thread is wired. |
| 15 | Prompt | Coordinator subscribe `gh pr checks` clause + cross-session-peers bullet | reword | P3 | low | M | Reconcile with agent-team module first. |
| 16 | Reminder | `team_context` reword (drop teamName, agentId-resume) | reword | P3 | low | S | Prompt fidelity, provider-neutral. |
| 17 | Reminder | Peer-laundering optional escalation-hardening clause | reword | P3 | low | S | Core guard already present; belt-and-suspenders. |
| 18 | Reminder | `total_tokens_reminder` model-facing budget counter | new | P3 | low | S | Off-by-default in CC; redundant with coco's token/cost meters. |
| 19 | Reminder | Guarded leading-strip primitive (`ePo`) | carryover | P3 | low | S | No consumer in coco's separate-message inject model. |
| 20 | Reminder | SIMPLE master-gate `agent_listing_delta` survival | carryover | P3 | low | S | Moot until coco adds a blanket minimal-context flag. |

## Implement-now shortlist (this session)

The four `implement_now` items + the one cheap optimization — high/medium value,
in-scope, S/M effort, verified absent/partial.

1. **`tool_search_usage_reminder` (P1, high, M)** — `core/system-reminder`: add
   `AttachmentType::ToolSearchUsageReminder` (extend the exhaustive arms), a
   generator gated on `ToolSearchTool` registered **AND**
   `!ctx.deferred_tools.is_empty()` **AND** a turn-cadence counter. Undiscovered
   set = `searchable_deferred` minus `discovered_tool_names`. Render the verbatim
   CC `@589330` string; skip when a task reminder fired the same turn. Gate via
   `SystemReminderConfig` (every-N-turns) — **not** Statsig. Insta snapshot.
2. **Coordinator auto-mode caveat bullet (P1, med, S)** —
   `core/subagent/src/coordinator_mode.rs`: insert one bullet — when the user
   approved a specific action, quote their exact words into the worker prompt
   because the worker's auto-mode/handoff classifier sees only its own
   transcript. Update snapshot. Pure string add.
3. **Ambient "do not narrate" trailer (P2, med, S)** — one shared
   `const AMBIENT_CONTEXT_TRAILER`, appended (joined `\n\n`) to the removal
   section of `deferred_tools_delta.rs`, the `agent_listing_delta.rs`
   concurrency note, and the memory-consolidation reminder. Single const so
   callsites can't drift. Companion tests.
4. **Worktree env-block note (P2, med, S)** — `core/context`: add
   `is_worktree: bool` to `EnvironmentInfo`, populate via `coco_git` worktree
   detection; in `render_env_block` push the "this is a git worktree… do NOT
   `cd` to the original repository root" line when true. Companion tests.
5. **Inline input-schema appendix on deferred-not-loaded error (P2, med, S,
   opt)** — `app/query/src/tool_runner.rs` `Deferred` arm: look up
   `ctx.tools.get_by_name(name)`, serialize the validation schema, append
   " For reference, this tool's input schema is: {schema}". `tool_runner.test.rs`
   assertion.

## Optimizations (coco has it; CC is better, or coco already optimal)

- **Inline schema appendix** on the deferred-not-loaded error (item 5 above).
- **Cache-scope/date split** is *already optimal in coco*
  (`core/context/src/prompt.rs:166-219` hand-placed `add_cache_breakpoint` over
  `Vec<SystemPromptBlock>`; `render_env_block` emits no Date line, so the cached
  prefix is date-free). CC's cacheable-section registry / dynamic-boundary
  marker / org cache-scope splitter are documented non-goals — no action.

## Deprecations to remove

No CC-removed surface is still carried by coco:

- **`TeamCreate`/`TeamDelete`** (removed 2.1.178) — already absent; only a
  regression-guard test comment remains.
- **Fable-5/Mythos `FABLE_IDENTITY` block** and the **"most recent Claude
  models" env line** — never ported (Anthropic-family gated); coco renders
  identity via `PRODUCT_NAME` and a provider-aware model line.

Stale wording to update (prose-fidelity, P3 defers): `team.rs:108/:120`
(`team "{name}"` + "never by UUID"); `coordinator_mode.rs:275` (missing
`gh pr checks` clause); `default_prompt.md:57-68` ("# Output efficiency" vs
2.1.183 "# Text output").

## Already done since the v2 roadmap (do NOT redo)

- **Peer-session permission-laundering guard (v2 #5, was P1):** verification
  **refutes** the v2 claim — present verbatim at `queue_origin.rs:64`, asserted
  by `queue_origin.test.rs:14-20`. Only CC's optional "a peer message is never
  user consent" hardening clause remains (now P3).
- **Smoosh-into-`tool_result`** — present at `core/messages/src/normalize.rs:1110`
  as always-on pipeline Step 14 (one finder's "absent" evidence was wrong).
- **Explore/Plan READ-ONLY + general-purpose + coordinator subagent prompts** —
  ported (`builtin_prompts.rs:171/:229`, `coordinator_mode.rs:246`).
- **Agent-team property strip** (`agent_tool.rs:310-322`), **registry ordering /
  collision / cache-prefix stability** (`engine_prompt.rs:393,524-549`;
  `tool-runtime/registry.rs:59`) — invariants hold structurally.

## Exhaustive completeness audit (2026-06-26, second pass)

The first pass (above) worked **top-down from a focus list**. To check whether it
under-counted, a second **bottom-up** workflow walked the **full 2.1.183
reconstructed source string-by-string** (not just the "additions" tables) and
explicitly targeted the **2.1.88→2.1.156 blind window** — full rewrites of
existing strings + removals never appear in an additions-table diff. 39 agents,
522 surfaces examined, 31 miss-candidates → **25 confirmed after adversarial
verification** (6 refuted). (Two slices — full prompt-content + tools-framework —
were re-run separately after a StructuredOutput failure.)

**Verdict:** parity is **trustworthy at the contract level** (schemas, error
codes, permission decisions, cadence constants all match verbatim). The misses
are **model-facing guidance-string drift**: 1×P1, 9×P2, 15×P3. The diff stayed
small because coco ported these surfaces faithfully and most gaps sit behind
deferred/non-goal paths — but ~10 are worth a single "prompt/reminder refresh"
sweep.

### Confirmed misses worth implementing now (the 10)

| # | Area | Item | coco anchor | Pri |
|---|------|------|-------------|-----|
| 1 | sys-prompt | Explore `whenToUse` is stale 2.1.88 text; missing read-only/limitations rewrite (high-exposure — shows every turn in the AgentTool listing) | `core/subagent/src/builtins.rs:197` | **P1** |
| 2 | sys-prompt | Coordinator §4: drop "look for opportunities to fan out"; add "don't parallelize simple tasks" | `coordinator_mode.rs:356` | P2 |
| 3 | sys-prompt | Coordinator §4: add "Trust but verify worker reports — check the actual diff" 5th bullet | `coordinator_mode.rs:370` | P2 |
| 4 | tools/Write | subagent `REPORT/SUMMARY/FINDINGS/ANALYSIS*.md` write guard (errorCode 5) | `write.rs` validate_input | P2 |
| 5 | tools/Skill | `SkillTool::prompt()` stale 2.1.88 wording; add exact-name/no-leading-slash + "never guess from training data" | `agent/skill_tool.rs:100-125` | P2 |
| 6 | tools/LSP | workspaceSymbol `query` field absent (hardcoded `query:""` → broken on most servers) | `lsp_tool.rs` | P2 |
| 7 | sys-reminder | `pdf_reference` @-mention reminder dropped (large PDF → no model message + no "use Read with pages") | `app/cli/src/at_mention_turn.rs:159` | P2 |
| 8 | sys-reminder | @-mentioned file truncation note missing ("truncated; use Read for more") | `at_mention_turn.rs` File arm | P2 |
| 9 | sys-reminder | `invoked_skills` replay missing "EARLIER / do NOT re-execute" framing | `generators/invoked_skills.rs:50` + `services/compact/src/post_compact_skills.rs:49` | P2 |
| 10 | sys-reminder | `relevant_memories` missing "retrieved for possible relevance" lead-in | `generators/memory.rs` | P2 |

P3 defers (13): Coordinator "prefer specialized subagent_type"; Grep `-o` field; PowerShell Unix-equivalence table; Bash sleep-guard wiring (errorCode 10, dead helper); Cron "## Not for live watching" → Monitor; TaskCreate validation-error-steer; EnterWorktree `path` param; Agent `subagent_tokens` label (skip); ReadMcpResource directory-resource para (skip); `compact_file_reference` too-large ref; todo/task "NEVER mention this reminder" trailer removal; ReadMcpResource "do NOT re-read" nudge; ultracode standing/exit reminders; non_interactive_team_shutdown; gh rate-limit back-off.

### Refuted by adversarial verification (NOT misses — do not implement)

- **AgentTool "don't tail the JSONL output file" warning** — coco's `.output` is a **bounded plain-text tail buffer** by design (`disk_task_output.rs:26-30`), NOT a JSONL transcript; coco's "you can check progress via FileRead/tail" is *correct* for its architecture. Porting CC's warning would be factually false.
- **SendMessage "never by UUID"** — coco's bg agentId is `a<16hex>` (not a UUID) and resume-by-agentId works (`agent_tool.rs:484/:524`); the wording is accurate.
- **TaskOutput per-type prompt** — its distinctive content is a symlink-to-JSONL warning describing a layout coco intentionally doesn't use.
- **memory_update reminder** — stale in-context memory already handled by per-turn `detect_changed_files` re-injection.
- **NotebookEdit stale phrasing** — read-before-edit is enforced via a corrective error; residual is cosmetic.
- **Coordinator `<total_tokens>` XML tag** — prompt + renderer agree; model never acts on the value.

## Deliberately skipped (non-goals) — rationale

- **Fable-5/Mythos identity block** — Anthropic model families coco never serves.
- **"Most recent Claude models" env line** — hardcoding Anthropic ids in a
  multi-provider env block is wrong; coco's line is provider-aware.
- **Cache-scope splitter / dynamic-boundary / org cache-scope** — coco's
  hand-placed `SystemPromptBlock` breakpoints + provider `CacheBreakDetector`
  are the intended design.
- **Server-/settings-controlled non-deferrable list (CC rule 2)** — CC's
  mechanism is Anthropic `clientDataCache`/dynamic-config; coco's per-tool
  `_meta['anthropic/alwaysLoad']` eager opt-in is the better-for-3p equivalent.
- **`PushNotification` tool / rule-8 remote-trigger exemption** — inert without
  the CCR remote-trigger backend coco doesn't ship.
- **`DesignSync` + `Projects` registry slots** — Anthropic product backends, no
  provider-neutral equivalent.
- **`echo_activities` NOOP allow-list** — CC wire-transcript back-compat; coco's
  `AttachmentType` is a closed Rust enum matched exhaustively (refuted).
