# P0-3 — Warning-First Tool-Call Loop Guardrails

Status: not started · Size: M · Owner crates: `coco-tool-runtime` (state
machine) + `coco-query` (wiring) + `coco-config` (knobs)

## Problem

Nothing in coco detects a model repeating the same failing tool call.
Verified absent: no tool+args-hash repeat detector anywhere;
`Tool::inputs_equivalent` (`core/tool-runtime/src/traits.rs`) exists
with **zero call sites** — a ready-made seam. The closest mechanisms are
different in kind: the permission `DenialTracker`
(`core/tool-runtime/src/denial_tracking.rs`, consecutive-denial circuit
breaker) and the StructuredOutput retry cap
(`app/query/src/engine_terminal.rs:227-260`). A model stuck on a failing
Edit or a no-progress Read loop burns tokens until the user intervenes.

## Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.5.7 (v0.13.0 "Tenacity"), PR #18227. All in
`agent/tool_guardrails.py` unless noted.

- **Controller**: `ToolCallGuardrailController` (:224), side-effect-free,
  per-turn `reset_for_turn()` (:231) called from
  `agent/turn_context.py:234`.
- **Signature**: `ToolCallSignature.from_call` (:127-141) =
  `(tool_name, sha256(canonical_tool_args(args)))`;
  `canonical_tool_args` (:176) = sorted-key compact JSON. Result
  no-progress detection hashes results the same way (`_result_hash`
  :430).
- **Idempotent vs mutating**: frozensets `IDEMPOTENT_TOOL_NAMES` (:20 —
  read_file, search_files, web_search, …) / `MUTATING_TOOL_NAMES` (:41 —
  terminal, write_file, patch, delegate_task, …); `_is_idempotent`
  (:377) — mutating wins.
- **Thresholds** — `ToolCallGuardrailConfig` (:63-81):

  ```python
  warnings_enabled: bool = True
  hard_stop_enabled: bool = False
  exact_failure_warn_after: int = 2      # identical failing call
  exact_failure_block_after: int = 5
  same_tool_failure_warn_after: int = 3  # same tool, any args
  same_tool_failure_halt_after: int = 8
  no_progress_warn_after: int = 2        # idempotent, same result
  no_progress_block_after: int = 5
  ```

- **Warning = suffix on the tool result, not a separate message** —
  `append_toolguard_guidance` (:394-403):

  ```python
  suffix = (f"\n\n[{label}: "
            f"{decision.code}; count={decision.count}; {decision.message}]")
  return (result or "") + suffix
  ```

  wired via `run_agent.py:5637-5655` from `agent/tool_executor.py:852`.
- **Block → exactly one synthetic tool result** (never an exception):
  `toolguard_synthetic_result` (:383-391) returns
  `json.dumps({"error": decision.message, "guardrail": …})`; checked
  pre-execution at `agent/tool_executor.py:447-449`.
- **Halt ends the turn** with a user-visible explanation:
  `agent/conversation_loop.py:4690-4697` → "I stopped retrying {tool}
  because it hit the tool-call guardrail …" (`run_agent.py:5628-5635`).
- **Config**: `config.yaml` section `tool_loop_guardrails`
  (`agent/agent_init.py:1314-1318`), nested keys
  `warn_after.{exact_failure,same_tool_failure,idempotent_no_progress}`
  / `hard_stop_after.{…}`.
- Failure detection there is string-sniffing (`classify_tool_failure`
  :189, `[error]` heuristics) — coco does NOT need this: tool results
  carry a typed error channel.

## Design

### Types (`core/tool-runtime/src/loop_guardrails.rs`, new module)

```rust
pub struct CallSignature { tool: ToolId, args_hash: [u8; 32] } // sha256 of canonical JSON

pub enum GuardrailVerdict {
    Allow,
    Warn { code: GuardrailCode, count: i64, message: String },
    Block { code: GuardrailCode, count: i64, message: String },
    HaltTurn { code: GuardrailCode, count: i64, message: String },
}

pub enum GuardrailCode { ExactFailureRepeat, SameToolFailures, NoProgressRepeat }

pub enum GuardrailLevel { Off, WarnOnly, Enforce } // enum over bools (repo rule)

pub struct GuardrailThresholds {
    pub exact_failure_warn_after: i64,   // 2
    pub exact_failure_block_after: i64,  // 5
    pub same_tool_failure_warn_after: i64, // 3
    pub same_tool_failure_halt_after: i64, // 8
    pub no_progress_warn_after: i64,     // 2
    pub no_progress_block_after: i64,    // 5
}

pub struct LoopGuardrailState { /* per-turn maps: sig→fail count, tool→fail count, sig→last result hash+count */ }
impl LoopGuardrailState {
    pub fn reset_for_turn(&mut self);
    pub fn before_call(&self, sig: &CallSignature, cfg: &GuardrailConfig) -> GuardrailVerdict;
    pub fn after_call(&mut self, sig: &CallSignature, failed: bool, result_hash: Option<[u8;32]>);
}
```

- Canonical args JSON: serialize `serde_json::Value` with sorted keys,
  compact separators (write a small canonicalizer; `serde_json` maps
  preserve insertion order, so re-collect into `BTreeMap` recursively).
- **Failure signal**: the typed outcome the executor already has
  (`ToolResult` error / `<tool_use_error>` construction in
  `app/query/src/tool_outcome_builder.rs`) — strictly better than
  hermes's string sniffing.
- **Idempotency**: default from the existing concurrency classification
  (`Tool::is_concurrency_safe`, MCP `readOnlyHint` at
  `core/tools/src/tools/mcp_tools.rs:728`) — read-only ⇒ idempotent.
  Add `Tool::is_idempotent()` with that default so tools can override
  independently later. No-progress tracking applies to idempotent tools
  only (mirror hermes).

### Wiring (`app/query`)

- State lives on the turn context; `reset_for_turn` at loop start.
- `before_call` in the tool-call preparation path (alongside the
  existing permission check ordering — after validation, before
  execution): `Block`/`HaltTurn` short-circuit execution and synthesize
  **one** tool result:
  `{"error": "<message>", "guardrail": {code, count}}` (JSON, so MCP and
  builtin tools read it uniformly).
- `after_call` + warning suffix in `tool_outcome_builder.rs`: append
  `\n\n[Tool loop warning: {code}; count={n}; {message}]` (or
  `Tool loop hard stop`) to the result content **before** it enters
  history. Append-only on the newest message ⇒ prompt-cache safe, and
  satisfies anti-lesson 3 (no retroactive mutation, so no freshness
  problem). Marker wording avoids `[SYSTEM:` (anti-lesson 4).
- `HaltTurn` maps to a `ContinueReason` terminal that ends the turn with
  the user-visible sentence (hermes wording): "I stopped retrying
  {tool} because it hit the tool-call guardrail ({code}, {n} attempts).
  Tell me how you'd like to proceed."
- Ordering note: guardrail verdicts are computed per prepared call in
  submission order; for a concurrent batch the counts update as results
  land (same semantics hermes gets from its sequential executor — a
  concurrent batch of identical calls may all pass `before_call`; the
  *next* round gets warned/blocked. Acceptable; document in code).

### Config (`coco-config`)

`tool.loop_guardrail` sub-config on the existing tool section:

```jsonc
{ "level": "warn_only",   // off | warn_only | enforce
  "warn_after":      { "exact_failure": 2, "same_tool_failure": 3, "no_progress": 2 },
  "hard_stop_after": { "exact_failure": 5, "same_tool_failure": 8, "no_progress": 5 } }
```

`level=warn_only` (default) emits warnings and never blocks —
`hard_stop_after` only applies at `enforce` (hermes: warnings default
on, hard stop opt-in). `#[serde(default)]` throughout. Not a
`Feature` — it's a sub-toggle.

## Implementation steps

1. `loop_guardrails.rs` + canonical-JSON hashing + unit tests
   (companion `.test.rs`).
2. `Tool::is_idempotent()` default impl; delete the dead
   `inputs_equivalent` if the new signature path fully supersedes it
   (repo rule: no dead code — verify zero call sites still, then
   remove).
3. Config plumbing (`ToolConfig` → `RuntimeConfig`).
4. Query wiring (before_call short-circuit, after_call + suffix, halt
   ContinueReason).
5. `just quick-check`; `just test-crate coco-tool-runtime`;
   `just test` (shared-crate change).

## Tests

- Same failing call ×2 → warning suffix appears once on the 2nd result;
  ×5 at `enforce` → synthetic blocked result, tool NOT executed.
- Distinct-args failures on one tool → warn at 3; halt at 8 only under
  `enforce`, with the user-visible halt message.
- Idempotent no-progress: same Read call, same result hash ×2 → warn;
  mutating tool with identical results → NOT flagged.
- `warn_only` never blocks; `off` is a no-op; state resets between turns.
- Concurrent batch of identical calls: all execute this round, next
  round warned (documents the ordering semantics).

## Risks / non-goals

- False positives on legitimately repeated calls (e.g. polling
  TaskOutput): mitigated by warn-only default, failure-gated counters
  (success resets nothing to worry about — successes only feed
  no-progress, which requires *identical* results on an idempotent
  tool), and per-turn reset.
- Non-goals: cross-turn memory of repetition; hermes's string-based
  failure classifier; auto-disable of tools.
