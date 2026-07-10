# P1-2 — ToolSearch Deferral Threshold Gate

Status: not started · Size: S · Owner crates: `coco-tool-runtime` (gate)
+ `coco-config` (knob) · Related: `../../tool-search-design.md` (owns the
ToolSearch architecture — this plan adds one gate, does not redefine it)

## Problem

coco's deferral decision is a static per-tool boolean:
`should_defer = tool_search_active && registered.tool.should_defer() &&
!always_load() && !discovered`
(`core/tool-runtime/src/registry.rs:385-388`), where `should_defer()` is
a hardcoded trait method (`core/tool-runtime/src/traits.rs:619-624`) and
the only dynamic gate is `Feature::ToolSearch` on/off. Consequence: a
session with a tiny deferrable set (e.g. two small MCP servers) still
pays the ToolSearch round-trip (search → describe → call) even though
inlining every schema would cost a fraction of a percent of the context
window.

## Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.6.5 (v0.16.0) #34493. All in `tools/tool_search.py`:

- `threshold_pct` default **10.0**, parse/clamp
  `max(0.0, min(100.0, threshold_pct))` (:83-103);
  `CHARS_PER_TOKEN = 4.0` (:55); `estimate_tokens_from_schemas`
  (:217-231) = serialized schema chars ÷ 4.
- **The gate** — `should_activate` (:234-258):

  ```python
  if not context_length or context_length <= 0:
      # Without a known context size, fall back to a fixed 20K-token cutoff
      return deferrable_tokens >= 20_000
  threshold_tokens = int(context_length * (config.threshold_pct / 100.0))
  return deferrable_tokens >= threshold_tokens
  ```

- **Core tools never deferred** (docstring :10-11 "No exceptions.";
  `is_deferrable_tool_name` :163-186) — coco parity: `always_load()` +
  builtin non-deferring tools.
- **Stateless catalog rebuilt every assembly** (:15-19, citing an
  upstream cron regression; re-classified from scratch in
  `assemble_tool_defs` :529-583) — coco parity: registry filtering runs
  per assembly already.

## Design

1. New method on the registry assembly path (the single place
   `should_defer` is consulted, `registry.rs:385`):

   ```rust
   /// Skip deferral entirely when the deferrable schemas are small
   /// relative to the context window.
   fn deferral_worthwhile(deferrable_schema_bytes: i64, context_window_tokens: Option<i64>, threshold_pct: i64) -> bool {
       let deferrable_tokens = deferrable_schema_bytes / 4; // chars≈bytes/4-token heuristic, same as hermes
       match context_window_tokens {
           None => deferrable_tokens >= 20_000, // fixed cutoff, mirror hermes
           Some(w) => deferrable_tokens >= w.saturating_mul(threshold_pct) / 100,
       }
   }
   ```

   - Sum `deferrable_schema_bytes` over exactly the tools that pass the
     current `should_defer` predicate (serialize schemas once — an
     estimator already exists in `services/inference`
     (`estimate_schema_tokens`) for per-model schema filtering; reuse or
     mirror it rather than a third heuristic).
   - Context window from the same source as the offload design:
     `resolved_context_window()` snapshot, `None` handled by the fixed
     cutoff (do NOT silently assume 200k here — the fixed 20k-token
     cutoff is the hermes-tested behavior for unknown windows).
2. When the gate says "not worthwhile": deferral is a no-op for this
   assembly — all tools inline, ToolSearch bridge tools stay registered
   but the deferred set is empty (hermes passes the full array through).
3. Config: `tool_search.threshold_pct: i64`, `#[serde(default)]` → 10,
   clamped to `0..=100`; `0` means "always defer" (gate disabled),
   preserving today's behavior as an escape hatch.
4. Stability note: the verdict can flip when an MCP server
   connects/disconnects mid-session — but the tools array changes at
   that boundary anyway, and coco already handles pool changes through
   the deferred-tools delta reminders, so no additional cache handling
   is needed. Within a stable pool the verdict is deterministic.

## Implementation steps

1. Gate + config plumbing; evaluate once per tools-array assembly.
2. Otel: log the decision (`deferrable_tokens`, `threshold_tokens`,
   verdict) at `debug` for tuning.
3. `just test-crate coco-tool-runtime`.

## Tests

- Small deferrable set (< threshold) → nothing deferred; ToolSearch
  still callable (returns "all tools already loaded" naturally).
- Large set → deferral unchanged from today.
- Unknown window → 20k-token fixed cutoff branch.
- `threshold_pct = 0` → always defers (today's behavior).
- Clamp: 150 → 100; negative → 0.

## Risks / non-goals

- Heuristic divergence from real tokenizers is fine — both sides of the
  comparison use the same chars/4 heuristic.
- Non-goal: per-model thresholds; dynamic schema compression.
