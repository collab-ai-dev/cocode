# coco-messages

Message **operations** crate: creation, normalization, predicates,
lookups, history persistence, cost tracking, **and the unified message
mutation pipeline** ([`pipeline::MessagePass`] + [`pipeline::run_message_passes`]).

The Message-family **type definitions** themselves live in
`coco-types` (relocated alongside `ServerNotification` so the wire
enum can carry typed `Message` payloads). This crate re-exports them
at its crate root for backward compat with the established
`coco_messages::Message` import path used across the ops layer.

## Key Types

### Re-exported from `coco-types::messages` (canonical home)

```rust
pub use coco_types::messages::*;
```

Surfaces the full Message family at `coco_messages::*`:

- **Message envelope**: `Message` (8 variants), `UserMessage`,
  `AssistantMessage`, `ToolResultMessage`, `AttachmentMessage`,
  `ProgressMessage`, `TombstoneMessage`, `ToolUseSummaryMessage`,
  `Visibility`, `MessageKind`, `MessageOrigin`, `StopReason`,
  `ApiError`, `PreservedSegment`, `PartialCompactDirection`.
- **System messages**: `SystemMessage` + 16 sub-variants
  (`SystemInformationalMessage`, `SystemApiErrorMessage`,
  `SystemCompactBoundaryMessage`, `SystemMicrocompactBoundaryMessage`,
  `SystemLocalCommandMessage`, `SystemPermissionRetryMessage`,
  `SystemBridgeStatusMessage`, `SystemMemorySavedMessage`,
  `SystemAwaySummaryMessage`, `SystemAgentsKilledMessage`,
  `SystemApiMetricsMessage`, `SystemStopHookSummaryMessage`,
  `SystemTurnDurationMessage`, `SystemScheduledTaskFireMessage`,
  `SystemContextUsageMessage`, `SystemUserInterruptionMessage`),
  `SystemMessageLevel`.
- **Attachment payloads**: `AttachmentBody`, `SilentPayload` + 10
  silent payload structs.
- **Tool result**: `ToolResult<T>` (carries `Vec<Message>`).
- **Hook result**: `HookResult` (embeds `Option<Message>`).
- **Persistence**: `SerializedMessage`, `TranscriptMessage`,
  `TranscriptEntry`.
- **LLM aliases (via `coco-llm-types`)**: `LlmMessage`, `LlmPrompt`,
  `UserContent`, `AssistantContent`, `ToolContent`, `TextContent`,
  `FileContent`, `ReasoningContent`, `ToolCallContent`,
  `ToolResultContent`.

### Owned here (operations only)

- **History**: `MessageHistory`
- **Cost**: `CostTracker`, `calculate_cost_usd`, `format_cost`, `get_model_pricing`
- **Creation**: `create_user_message`, `create_user_message_with_parts`,
  `create_assistant_message`, `create_assistant_error_message`,
  `create_cancellation_message`, `create_compact_boundary_message`,
  `create_error_tool_result`, `create_info_message`,
  `create_meta_message`, `create_permission_denied_message`,
  `create_progress_message`, `create_tool_result_message`,
  `create_user_interruption_system_message`
- **Normalize**: `normalize_messages_for_api(&[Arc<Message>]) -> Vec<LlmMessage>`,
  `to_llm_prompt`, `ensure_user_first`, `merge_consecutive_user_messages`,
  `merge_consecutive_assistant_messages`, `strip_images_from_messages`,
  `strip_signature_blocks`
- **Lookups**: `MessageLookups`, `build_message_lookups`
- **Pipeline**: `MessagePass` trait + `run_message_passes` helper +
  `borrow_refs` (shared by coco-messages and coco-compact for any
  pass-based mutation pipeline). See [Pipeline section](#pipeline-architecture).

## Module Layout

- `creation` — message constructors
- `normalize` — API-shape normalization. Hosts 7 [`MessagePass`] impls
  (`OrphanedThinkingOnly`, `TrailingThinking`, `WhitespaceOnly`,
  `EnsureNonEmptyContent`, `MergeConsecutiveUsers`,
  `MergeAssistantsByRequestId`, `StripExitPlanModeInjectedFields`)
  used by `normalize_messages_for_api` step 8-13a.
- `pipeline` — `MessagePass` trait + `run_message_passes` helper
  ("Arc → owned → mutate → Arc" bridge).
- `predicates` — is_* / has_* predicates
- `lookups` — O(1) index builders
- `wrapping` — message wrapping helpers
- `history` — persistence
- `cost` — token/cost tracking

Note: the legacy `filtering` module was deleted in the pipeline
refactor — production uses `normalize::filter_by_options` directly
(Arc-vec in, Arc-vec out). Type definitions live in
`coco-types::messages` and are re-exported at the crate root.

## Vercel-AI Seam

`coco-messages` does **not** depend on `coco-inference`. DTO content
shapes reach this crate via `coco-types` (which depends on
`coco-llm-types`). Runtime types (`LanguageModel` trait, model runtime,
etc.) are inference's domain and not needed for ops-layer work.

## Architecture

Internal messages embed an `LlmMessage` body directly — no twin
types, no conversion layer. `coco-llm-types` provides the
version-stripped `LlmMessage` alias so SDK upgrades stay scoped to
`common/llm-types/src/lib.rs` + `services/inference/src/lib.rs`.

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

**Pipeline construction** — explicit static dispatch, no `dyn`:

```rust
let refs = borrow_refs(input);
let needs_mutate = Pass1.would_mutate(&refs) || Pass2.would_mutate(&refs);
drop(refs);
run_message_passes(input, needs_mutate, |owned| {
    Pass1.apply(owned);
    Pass2.apply(owned);
})
```

The `||` chain and `.apply()` chain must list passes in the same order.
Adding a pass requires editing both (caught at review, not compile).

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
3. Add the pass to the pipeline's `||` chain AND `.apply()` chain
   at the call site (e.g., `normalize_messages_for_api`).
4. Cover the new trigger condition in `pipeline_invariants` so the
   drift test exercises both fast and slow paths.

See [docs/coco-rs/message-pipeline.md](../../docs/coco-rs/message-pipeline.md)
for the design rationale and migration history.
