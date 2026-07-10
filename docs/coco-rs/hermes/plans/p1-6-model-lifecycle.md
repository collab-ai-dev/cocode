# P1-6 — Model Lifecycle Metadata: Retirement Warning + Reasoning Stall-Timeout Floor

Status: not started · Size: M · Owner crates: `coco-model-card` (schema
+ data), `coco-cli` (startup warning), `services/inference` +
`vercel-ai/ai` (stall floor) · Related: merge part (a) with the
model-card item in `../../cc-catchup-roadmap-v2.md` (coco's own running
id resolution) — one schema change, one data pass.

## Part (a) — Retirement metadata + startup warning

### Problem

`ModelCard` has **no** lifecycle field — schema is
`canonical_id, aliases, family, knowledge_cutoff, pricing,
vendor_context_window, display_name`
(`common/model-card/src/schema.rs:2-18`); grep for
`deprecat|retire|sunset|eol` across `common/model-card` finds nothing.
When a vendor retires a configured model, the user's first symptom is a
silent 404 mid-session.

### Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.5.28 #29277. Hermes's mechanism is vendor-specific
(xAI), which is itself instructive — retirement facts are curated per
vendor announcement, not scraped:

- `hermes_cli/xai_retirement.py:15-16` — `MIGRATION_GUIDE_URL`,
  `RETIREMENT_DATE = "May 15, 2026"`; `_RETIRED_MODELS` mapping
  (:23-32) with per-model `replacement` **and** parameter migration
  (e.g. `"grok-4-fast-non-reasoning": {"replacement": "grok-4.3",
  "reasoning_effort": "none"}` — non-reasoning variants migrate with
  reasoning disabled since the replacement reasons by default,
  :20-22).
- Detection walks every config slot that can hold a model id:
  `find_retired_xai_refs(config)` (:62-120) — principal, auxiliary
  roles, delegation, tts, plugins.
- **Startup warning: one-shot, non-blocking, never fails startup** —
  `hermes_cli/main.py:2289-2310`: `⚠ xAI retires N model(s) in your
  config on {date}` + "Run 'hermes doctor' for details."
- Guided migration: `hermes_cli/migrate.py` `cmd_migrate_xai` (:26-70) —
  dry-run by default, `--apply` rewrites config in place with a
  timestamped backup (`apply_migration`, `xai_retirement.py:171+`).
- Generic-catalog hint only: `agent/models_dev.py:81`
  (`status: str  # "alpha", "beta", "deprecated", or ""`).

### Design

1. Schema (`common/model-card`):

   ```rust
   #[derive(Default)]
   pub struct ModelLifecycle {
       pub status: LifecycleStatus,            // Active (default) | Deprecated | Retired
       pub retires_on: Option<NaiveDate>,      // vendor-announced date only
       pub replacement: Option<String>,        // canonical model id
       pub migration_note: Option<String>,     // e.g. "reasoning on by default in replacement"
   }
   ```

   `#[serde(default)]` on the `ModelCard` field; **data comes only from
   vendor announcements** (model-card rule: facts are curated, never
   guessed — same constraint as pricing).
2. Startup check (`app/cli` bootstrap, after `RuntimeConfig` is built):
   resolve every `ModelRole` to its card via exact-id lookup; for
   `Deprecated` with a future `retires_on` → one-shot `warn` notice
   (TUI notice line / stderr in headless); for `Retired` → prominent
   warning naming the `replacement`. **Never block or fail startup**
   (hermes's explicit property). No auto-rewrite of config in v1 —
   surface the fact + the replacement; a `/model`-picker badge is the
   natural second step (the picker already renders card metadata).
3. Walk the same slots hermes does: all `ModelRoles` entries + any
   per-command model overrides — i.e. everything reachable from
   `RuntimeConfig`, not just `Main`.

## Part (b) — Per-model reasoning floor for stream stall detection

### Problem

coco has the stall machinery but trades it away globally:
`StreamProcessor` supports `idle_timeout` (default 60 s) and
`stall_threshold` (30 s) (`vercel-ai/ai/src/stream/processor.rs:21-22,
94-107`), but the inference default **disables** idle timeout precisely
to avoid killing slow reasoning streams
(`services/inference/src/stream.rs:604-616`). Result: a genuinely hung
stream on a non-reasoning model hangs the turn until the outer request
timeout.

### Hermes evidence (hermes-agent @ `a7f65e3bc`)

Release v2026.7.1 #52845 ("Fixes #52217"). Module
`agent/reasoning_timeouts.py` (225 lines):

- Two default detectors: stream stale timeout default **180 s**,
  non-stream **90 s** (docstring :7-10).
- **Floor table** `_REASONING_STALE_TIMEOUT_FLOORS` (:62-115): e.g.
  `deepseek-r1`/`deepseek-v4-*` 600, `o1/o3/o3-pro` 600,
  `o3-mini/o4-mini` 300, `qwq-32b` 300, `claude-opus-4` 240,
  `claude-sonnet-4.5/4.6` 180, `grok-4-fast-reasoning` 300.
- Resolver (:172-224): strips aggregator prefix, start-of-slug anchored
  regex, longest-slug-wins. **"This is a FLOOR — callers must apply it
  as `max(default, floor)`"** (:187).
- Consumers apply exactly that:
  `agent/chat_completion_helpers.py:2908-2911`
  (`_stream_stale_timeout = max(_stream_stale_timeout, _reasoning_floor)`)
  and `run_agent.py:1233-1273` (non-stream).

### Design

1. Add `stall_timeout_floor_secs: Option<i64>` to `ModelCard`
   (`#[serde(default)]`). Curate values for the reasoning families coco
   ships cards for (start from hermes's table for overlapping models;
   the exact numbers are operational tuning, not vendor facts, so they
   may live in card data with a comment citing hermes's table as the
   seed).
2. Re-enable idle detection by default in `services/inference`
   (`stream.rs:604` site): effective idle timeout =
   `max(configured_or_default, card_floor.unwrap_or(0))`, where the
   default returns to the `StreamProcessor` 60 s (or a conservative
   120 s — decide with telemetry). Unknown model / no card → keep
   **disabled** (today's safe behavior) so the change is strictly
   card-gated and can't kill an unlisted slow reasoner.
3. Config override precedence unchanged: explicit user config wins,
   then the card floor lifts it, mirroring hermes's env>config>floor
   ordering (`run_agent.py:1233-1273`).
4. Otel: log resolved timeout + source (`config`/`floor`/`disabled`) at
   stream open, so misfires are diagnosable.

## Implementation steps

1. Schema fields + serde tests (`coco-model-card`).
2. Data pass over bundled cards (retirement facts + floors — curated,
   cite sources in the data file comments).
3. Startup lifecycle check + notice (`coco-cli`).
4. Stall-floor resolution in `services/inference` + re-enable default
   (card-gated).
5. `just test-crate coco-model-card` + `coco-inference`; full
   `just test` (shared crates).

## Tests

- Card with `Retired` + replacement → startup notice contains both;
  startup exit code unaffected.
- All-Active config → zero notices.
- Floor resolution: card floor 600 + default 120 → 600; user config
  900 + floor 600 → 900; no card → idle detection disabled (parity with
  today).
- Simulated stalled stream on a floored model aborts at the floor, on a
  cardless model does not abort.

## Risks / non-goals

- Wrong floor value kills a legitimate slow stream → card-gated rollout
  (unknown models keep today's disabled behavior) + otel visibility.
- Non-goals: auto config rewrite (`hermes migrate xai --apply` analog) —
  v2 once the warning proves out; live catalog fetch (model-card is
  bundled-data by design); per-provider (vs per-model) floors.
