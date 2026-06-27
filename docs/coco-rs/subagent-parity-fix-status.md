# Subagent Parity Fix — Status

Date: 2026-05-16 (round 2)
Companion: `subagent-parity-fix-plan.md`

This document is the second-round status after the adversarial review.
The first round landed 6 surgical fixes inside the existing god-function;
the second round addressed the architectural gaps the review surfaced,
plus the new `coco-model-card` crate.

---

## Honest summary

| Status | Items |
|---|---|
| ✅ Landed in round 1 | PR 1.1 env block, P0-1 fork-model strip, Missed-2 parent_runtime wiring, PR 2.1 system-prompt reuse, P1-6 plan-mode swap, stale-doc cleanup |
| ✅ Landed in round 2 | `coco-model-card` crate, G1 test fixture, G2/G3/G4 fork pinning at type level, G6 AGENT_NOTES ordering, G7 add-dir dedup, G8 ToolAllowList enum, G9 insta snapshot, G10 source citation sibling file, G11 home_dir fallback, G12 Display impl |
| ✓ Verified false alarms | PR 1.2 wildcard memory (semantically equivalent), PR 4.3 walkdir+mcp inline, G5 skill listing (subagent inherits SkillsSource via wire_engine) |
| ⏸ Still deferred | PR 1.3 full AgentSpawnRequest decomp, PR 2.2 SpawnPipeline, PR 3.2 cancel propagation, PR 4.1 sync events, PR 4.2 narrowed_by enforcement, PR 4.4 dynamic schema, PR 4.5 fork-decision collapse, PR 5.1 verbatim prompts, PR 5.2 Resume e2e |

---

## Round 2 fixes (this commit)

### ✅ New crate `coco-model-card`

`common/model-card/` carries vendor-defined model facts (knowledge cutoff,
pricing, vendor context window, deprecation). Separate from
`coco-config::ModelInfo` because ownership and update cadence differ:
`ModelInfo` is user-configurable; `ModelCard` is vendor-published facts.

- `lookup(model_id) -> Option<&'static ModelCard>` — exact match against
  canonical id + per-card alias slice. **No substring matching, no case
  folding.** Unknown ids return `None` — env block omits the line.
- `knowledge_cutoff(model_id) -> Option<&'static str>` — convenience for
  the env-block call site.
- `pricing(model_id) -> Option<&'static Pricing>` — for future cost
  telemetry.
- 6 Claude models + GPT-5-4 placeholder seeded.
- 8 unit tests including `no_substring_matching` (the bug class the
  crate exists to prevent).

`core/context/src/environment.rs::knowledge_cutoff_for_model` becomes a
thin shim delegating to the crate. The previous substring-matching helper
that returned wrong cutoffs for `claude-haiku-4-5` and fallback
unknowns is gone.

### ✅ G1 — `tool_context.test.rs` missing field

Added `parent_runtime_snapshot: None` to the test fixture at
`app/query/src/tool_context.test.rs:54`. Build no longer fails on
`cargo check --tests`.

### ✅ G2 + G3 + G4 — Fork pinning at the type level

`SpawnMode::Fork` now carries `parent_snapshot: Arc<SubagentRuntimeSnapshot>`
as a **non-optional field**. The previously-optional
`AgentSpawnRequest.parent_runtime_snapshot` is gone — there is no longer
a way to construct `SpawnMode::Fork` without a snapshot.

- `AgentTool::execute` fails loud (`ExecutionFailed`) when fork is
  requested and `ctx.parent_runtime_snapshot` is `None`. No silent
  fallback to live runtime.
- `coordinator::spawn.rs` reads the snapshot directly from the matched
  variant — used for **both** env-block rendering AND the actual
  `AgentQueryConfig.model` for the API call. Cache parity now survives
  hot-reload on the actual model resolution, not just the displayed one.
- `SpawnMode::Resume` no longer pins to a snapshot — it reads live
  `RuntimeConfig` (G4). Pinning Resume to a snapshot captured at engine
  bootstrap was meaningless for resumes that cross processes.
- `SpawnMode` drops `Serialize`/`Deserialize` derives. The wire form is
  `#[serde(skip)]` on `AgentSpawnRequest.spawn_mode`. In-process spawn
  carries the runtime form; IPC rebuilds it on the receiver side.
- The `parent_runtime_snapshot` field on `AgentQueryConfig` is removed
  (dead — no consumer was reading it).

### ✅ G6 — AGENT_NOTES ordering

`build_system_prompt` rename: `custom_append` → `notes_after_env`. The
slot moves to immediately after the env block (before skill listing,
memory). For the subagent this matches TS `enhanceSystemPromptWithEnvDetails`
where `notes` come bundled with the env block, not after memory.

Snapshot in `core/context/src/snapshots/coco_context__prompt__tests__snapshot_subagent_full_prompt.snap`
locks in the order: identity → env → AGENT_NOTES → skills → memory.

### ✅ G7 — `--add-dir` resolution dedup

`headless::resolve_additional_dirs_display(cli, cwd) -> Vec<String>` is
the single source of truth. Both `headless::compose_system_prompt` and
`session_bootstrap::build_engine_resources` now call it. Previously
there were two divergent implementations (one returning `Vec<PathBuf>`,
one inline returning `Vec<String>`).

### ✅ G8 — `ToolAllowList` enum

`AgentDefinition.allowed_tools` is now
`ToolAllowList { Wildcard, Explicit(Vec<String>) }` instead of
`Vec<String>`. The enum makes three previously-ambiguous states
representable:
- `Wildcard` — `tools:` absent OR `tools: ['*']` (mirrors TS
  `tools: undefined`)
- `Explicit([..])` — finite list, agent sees only those
- `Explicit([])` — explicit empty (degenerate but legal)

`from_frontmatter(items)` collapses empty Vec and `['*']` to `Wildcard`
at the parser boundary. `inject_memory_tools` uses `as_explicit_mut()` —
wildcard agents are skipped at the type level rather than via
`is_empty()` heuristic. `format_tools_description` exhaustively matches
on the enum.

### ✅ G9 — `insta` snapshot test for `build_system_prompt`

Two snapshots:
- `snapshot_subagent_full_prompt.snap` — identity + env + AGENT_NOTES +
  skill listing + memory + additional dirs.
- `snapshot_main_agent_prompt.snap` — identity + env only (no
  AGENT_NOTES, matching TS main-agent path).

Plus a regression test `notes_after_env_renders_before_memory` that
asserts the ordering invariant without relying on the snapshot.

### ✅ G10 — Source citation sibling file

`core/context/src/agent_notes.SOURCE.md` documents the provenance of
`agent_notes.md` without polluting the model's view. The data file
itself is `include_str!`'d into the system prompt, so a header comment
would leak; the sibling file is the right shape.

### ✅ G11 — `home_dir` fallback

`coordinator::spawn.rs::713` no longer silently falls back to
`/tmp`. Pattern match emits a `tracing::warn!` and skips per-agent
memory injection. Better an empty memory section than fabricated paths.

### ✅ G12 — Consolidate `Platform`/`ShellKind` display methods

`Platform::display_name` replaced by `impl std::fmt::Display` (idiomatic
Rust). `ts_name()` stays for the wire format. The single consumer
(`get_os_version` fallback) uses `Platform::current().to_string()`.

---

## Verified false alarms (round 2)

### G5 — Subagent skill listing was a non-issue

The first-round adversarial review flagged `spawn.rs:762` passing
`skill_listing = None` to `build_system_prompt`. Verification on
deeper inspection shows:

- The main agent (`headless.rs:286`) also passes `None` for this slot.
- Skill listing for both main and subagent flows via
  `coco-system-reminder::generators::skill_listing` per-turn, not as
  a baked-in system-prompt section.
- The subagent's engine goes through `session_runtime::build_engine_from_config`
  → `wire_engine` → `with_reminder_sources(sources)` (`session_runtime.rs:1900`),
  which installs the same `SkillsSource` as the main engine.

The subagent inherits the per-turn skill listing pipeline correctly.
No fix needed.

---

## Still deferred (with current rationale)

| Item | Why still deferred |
|---|---|
| PR 1.3 `AgentSpawnRequest` decomposition into nested concern structs | Type-level fix for the biggest concrete bug (fork pinning) landed via G2/G3/G4 inside the existing flat struct. Splitting into `AgentInheritance` / `AgentSafetyConfig` / etc. is now cosmetic — does not unblock additional fixes. |
| PR 2.2 `SpawnPipeline` of `SpawnStep`s | User asked to ignore LoC concerns. The remaining concerns it would address (per-step cleanup ordering, testability) are now stable enough that the refactor is a follow-up cleanup, not a correctness gate. |
| PR 3.2 cancel propagation | Still real. Sync subagents do not honor parent cancel. Worth doing — separate PR. |
| PR 4.1 sync subagent event piping | Still real. UI gets no nested tool-call events. |
| PR 4.2 `ToolRegistry::narrowed_by(plan)` | Cosmetic — `agent_adapter` already consumes the plan correctly. Visibility tweak. |
| PR 4.4 dynamic `subagent_type` schema | Still real. Unknown types degrade to `general-purpose` silently. |
| PR 4.5 collapse fork decisions | Subsumed: with G2/G3/G4 landing, `SpawnMode::Fork` is constructable only at the AgentTool boundary in practice. `is_fork_subagent_active` gate is still a separate function but they no longer drift. |
| PR 5.1 verbatim builtin prompts | Mechanical. |
| PR 5.2 Resume e2e + transcript symlink | Test infra. |

---

## Verification

```
just quick-check    # passes (fmt + seam guards + clippy zero warnings)
cargo nextest run -p coco-model-card     # 8/8 pass
cargo nextest run -p coco-context        # 252/252 pass
cargo nextest run -p coco-types          # passes (ToolAllowList tests added)
```

The full workspace `cargo nextest run --workspace` is in flight at the
time of this writing — see git log for the final commit's actual test
counts.
