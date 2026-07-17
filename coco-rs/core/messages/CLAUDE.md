# coco-messages

Message **operations** crate: creation, normalization, predicates,
lookups, history persistence, cost tracking, **and the unified message
mutation pipeline** ([`pipeline::MessagePass`] + [`pipeline::run_message_passes`]).

## Type definitions live in `coco-types`

The Message family — `Message` (**7 variants**: `User` / `Assistant` /
`System` / `Attachment` / `ToolResult` / `Progress` / `Tombstone`), the
`SystemMessage` sub-variants, attachment payloads, persistence types, LLM
aliases — is defined in `coco-types::messages` and re-exported at this
crate's root (`pub use coco_types::messages::*;`). See
`common/types/CLAUDE.md` for the family. Note: `ToolUseSummary` is an SDK
event side-channel, **not** a `Message` variant.

## Owned here (operations) + Module Layout

- `history` — `MessageHistory` persistence
- `cost` — `CostTracker`, `calculate_cost_usd`, `get_model_pricing`
- `creation` — the `create_*` constructor family (user / assistant / tool-result / system / meta / progress / compact-boundary / …)
- `normalize` — `normalize_messages_for_api(&[Arc<Message>]) -> Vec<LlmMessage>`, `to_llm_prompt`, `filter_by_options`; hosts the 7 [`MessagePass`] impls used by steps 8-13a
- `pipeline` — `MessagePass` trait + `run_message_passes` + `borrow_refs` ("Arc → owned → mutate → Arc" bridge; shared with coco-compact)
- `predicates` / `lookups` — `is_*`/`has_*` predicates; `MessageLookups` O(1) index builders
- `wrapping` — system-reminder wrapping helpers
- `changed_file` — model-visible reminder for files changed since the model last saw them
- `command_tags` — slash-command echo/result transcript messages
- `content_kind` — per-content-kind token-estimation density (single source of truth for the density magic numbers)
- `token_estimation` — token estimation over `Message` content parts (lives here, not in `core/context`)
- `resume` — resume-time history normalization

Note: the legacy `filtering` module was deleted in the pipeline refactor —
production uses `normalize::filter_by_options` directly (Arc-vec in, Arc-vec out).

## Vercel-AI Seam

`coco-messages` does **not** depend on `coco-inference`. DTO content shapes
reach this crate via `coco-types` (which depends on `coco-llm-types`).
Internal messages embed an `LlmMessage` body directly — no twin types, no
conversion layer. `coco-llm-types` provides the version-stripped `LlmMessage`
alias so SDK upgrades stay scoped to `common/llm-types/src/lib.rs` +
`services/inference/src/lib.rs`.

## Pipeline Architecture

The `pipeline` module hosts the **single canonical bridge** between
the in-memory `Vec<Arc<Message>>` form and the in-place mutating
algorithms that need `&mut Vec<Message>`. Used by both
`normalize_messages_for_api` (steps 8-13a) and the compact crate
(`StripImages` / `StripReinjectedAttachments` passes).

```rust
pub trait MessagePass {
    fn would_mutate(&self, messages: &[&Message]) -> bool;
    fn apply(&self, messages: &mut Vec<Message>);
}

pub fn run_message_passes(
    input: &[Arc<Message>],
    needs_mutate: bool,
    apply_all: impl FnOnce(&mut Vec<Message>),
) -> Vec<Arc<Message>>;
```

**Contract** — implementers MUST satisfy:
- If `would_mutate` returns `false`, `apply` is a no-op.
- `would_mutate` is referentially transparent (same input ⇒ same output)
  and strictly cheaper than `apply` (single walk, no allocation).
- Over-conservative `would_mutate` (false positive) is acceptable
  (slow path runs unnecessarily but correctness preserved). Under-
  reporting (false negative) IS a bug — silently skips mutation.

**Pipeline construction** — explicit static dispatch, no `dyn`, with one
pass-order declaration in `normalize.rs`:

```rust
declare_normalize_passes!(
    OrphanedThinkingOnly, TrailingThinking, WhitespaceOnly,
    EnsureNonEmptyContent, MergeConsecutiveUsers,
    MergeAssistantsByRequestId, StripExitPlanModeInjectedFields,
);
```

The macro generates `NORMALIZE_PASS_ORDER`,
`normalize_passes_would_mutate(&refs)`, and
`apply_normalize_passes(&mut owned)`, so adding or reordering a
normalize pass requires editing one list.

**Fast path** (no pass would mutate) → `input.to_vec()` (N×Arc::clone,
zero Message::clone). **Slow path** → materialize once, run all
passes in order, re-wrap.

### Drift-detection invariant (tested)

`normalize.test.rs::pipeline_invariants` verifies the trait contract
for each of the 7 normalize passes: for every "clean" input (where
`would_mutate` returns `false`), running `apply` produces an
unchanged `Vec<Message>`. This catches the silent-divergence failure
mode (false-negative predicate → mutation silently skipped).

### Adding a pass

1. Add a unit struct to the relevant `passes` module
   (`normalize::passes` or `compact::compact_passes`).
2. `impl MessagePass for X` with `would_mutate` (cheap scan) and
   `apply` (delegates to the existing `pub(crate) fn` algorithm).
3. Add the pass once to `declare_normalize_passes!(...)` in `normalize.rs`.
4. Cover the new trigger condition in `pipeline_invariants` so the
   drift test exercises both fast and slow paths.

See [docs/internal/message-pipeline.md](../../../docs/internal/message-pipeline.md)
for the design rationale and migration history.
