# Message Pipeline Architecture

> Owner: `coco-messages::pipeline` (trait + helper) + `coco-messages::normalize::passes` + `coco-compact::compact::compact_passes`
> TS source: `utils/messages.ts::normalizeMessagesForAPI` (steps 8-13a) and `services/compact/compact.ts::stripImagesFromMessages` / `stripReinjectedAttachments`
> Status: implemented. Verified by `coco-messages::pipeline_invariants` (15 tests) and 671 integration tests across `coco-messages`, `coco-compact`, `coco-query`.

## 1. Purpose

This doc owns the design of coco-rs's **unified message mutation
pipeline** — the canonical bridge between the in-memory
`Vec<Arc<Message>>` form and the TS-parity in-place mutating
algorithms (filter / merge / strip) that need `&mut Vec<Message>`.

The pipeline collapses **three** previous scattered patterns into one
shared abstraction:

1. `coco-compact::compact_conversation` upfront entry materialize
2. `coco-compact::partial_compact_conversation` upfront entry materialize
3. `coco-compact::compact_session_memory` upfront entry materialize
4. `coco-messages::normalize_messages_for_api` step 8-12 owned-Vec materialize
5. The `ArcInput` sealed-trait band-aid that previously sat at the
   compact entry to bridge `&[Arc<Message>]` and `&[Message]` callers
6. The `_arc` function-name duplicates
   (`strip_images_from_messages_arc`, `strip_reinjected_attachments_arc`)
7. The `needs_message_level_passes` centralized predicate that
   duplicated the trigger condition of each of the 7 normalize passes

All of the above are now gone. One trait + one helper + a single
canonical call shape replace them.

## 2. The Rule (one sentence)

> **`Vec<Arc<Message>>` is the canonical in-memory container for
> messages across coco-rs. Every public mutation / transformation API
> takes `&[Arc<Message>]` and returns `Vec<Arc<Message>>` (or
> `Vec<LlmMessage>` at the wire seam). In-place TS-parity algorithms
> stay in private `pub(crate) fn (&mut Vec<Message>)` form and are
> driven by `MessagePass` impls via `run_message_passes`.**

Three exceptions, and only three:

1. **Constructor boundary** — `create_*_message() -> Message` returns
   owned; callers wrap in `Arc::new`. (No reason to allocate an Arc
   inside the constructor when the caller may immediately push into
   history's `push_arc`.)
2. **Wire seam** — `Vec<LlmMessage>` to the provider, `Vec<Message>`
   to JSONL persistence. Materialized once at the seam.
3. **History internal** — `MessageHistory::with_owned_messages` for
   compaction passes that need the index rebuild. One escape hatch.

Read-only utilities (`tokens::estimate_tokens`,
`group_messages_by_api_round`, `extract_discovered_tool_names`) keep
`<M: Borrow<Message>>` generic so they accept both `&[Arc<Message>]`
and the `&[&Message]` slices that internal iterators produce.

## 3. The trait

```rust
// coco-messages::pipeline

pub trait MessagePass {
    /// Cheap predicate: would `apply` mutate this slice?
    ///
    /// Receives borrowed `&Message` refs so the check is allocation-free.
    /// MUST be referentially transparent (same input ⇒ same output)
    /// and strictly cheaper than `apply` (single walk, no clone).
    ///
    /// Contract: **if this returns `false`, `apply` MUST be a no-op**.
    fn would_mutate(&self, messages: &[&Message]) -> bool;

    /// In-place mutation. Only invoked when the pipeline's combined
    /// `needs_mutate` (OR of every pass's `would_mutate`) is `true`.
    fn apply(&self, messages: &mut Vec<Message>);
}
```

**Why a trait** rather than a function pointer or closure pair:
- Each pass bundles `would_mutate` + `apply` in one `impl` block. Editing
  one without the other is physically adjacent — cannot drift.
- Reviewing a new pass = reading one `impl`, not three places.
- Tests can exercise `would_mutate` and `apply` independently per pass.

**Why static dispatch** (unit structs, not `dyn`):
- Each pass is a zero-size unit struct. Compiler monomorphizes;
  no vtable, no indirect call.
- The pipeline's static `||` chain is fully visible at the call site —
  `Pass1.would_mutate(refs) || Pass2.would_mutate(refs) || ...`.
- For `normalize_messages_for_api` (per-LLM-call, hot path), this
  avoids ~14 indirect calls per turn that a `dyn`-based pipeline would
  incur.

## 4. The helper

```rust
pub fn run_message_passes(
    input: &[Arc<Message>],
    needs_mutate: bool,
    apply_all: impl FnOnce(&mut Vec<Message>),
) -> Vec<Arc<Message>> {
    if !needs_mutate {
        return input.to_vec();   // fast path: N×Arc::clone, zero Message::clone
    }
    let mut owned: Vec<Message> = input.iter().map(|a| (**a).clone()).collect();
    apply_all(&mut owned);
    owned.into_iter().map(Arc::new).collect()
}

pub fn borrow_refs(input: &[Arc<Message>]) -> Vec<&Message> {
    input.iter().map(Arc::as_ref).collect()
}
```

**Fast path** (no pass would mutate) — `input.to_vec()` does N atomic
refcount bumps, zero `Message::clone`. Matches TS's `[...messages]`
shallow array copy (V8 pointers).

**Slow path** — materialize one `Vec<Message>` (N deep clones),
invoke `apply_all` (which runs each pass's `apply` in caller order),
re-wrap into a fresh `Vec<Arc<Message>>`. Same cost as the pre-refactor
hand-written entry materialize, but **only fires when at least one
pass would actually mutate**.

`needs_mutate` is computed by the caller as an explicit `||` chain
over each pass's `would_mutate` — this keeps both the predicate
expression and the `apply` sequence visible side-by-side at the call
site.

## 5. The canonical call shape

```rust
fn run_some_pipeline(input: &[Arc<Message>]) -> Vec<Arc<Message>> {
    let refs = borrow_refs(input);
    let needs_mutate = Pass1.would_mutate(&refs)
        || Pass2.would_mutate(&refs)
        || Pass3.would_mutate(&refs);
    drop(refs);

    run_message_passes(input, needs_mutate, |owned| {
        Pass1.apply(owned);
        Pass2.apply(owned);
        Pass3.apply(owned);
    })
}
```

The `||` chain and the `.apply()` chain are **structurally duplicated**
— both must list the same passes in the same order. Reviewers verify
at PR time; the `pipeline_invariants` test suite catches any pass
whose `would_mutate` becomes incorrect.

### Currently shipped pipelines

| Pipeline | Caller | Passes (in order) |
|---|---|---|
| `normalize_messages_for_api` step 8-13a | `coco-messages::normalize` | `OrphanedThinkingOnly`, `TrailingThinking`, `WhitespaceOnly`, `EnsureNonEmptyContent`, `MergeConsecutiveUsers`, `MergeAssistantsByRequestId`, `StripExitPlanModeInjectedFields` |
| `run_compact_strip_pipeline` | `coco-compact::compact` (used by `compact_conversation`, `partial_compact_conversation`, `compact_session_memory`) | `StripImages`, `StripReinjectedAttachments` |

## 6. Pass catalog

### `coco-messages::normalize::passes` (7 passes, TS messages.ts:2255-2343)

| Pass | TS source | `would_mutate` trigger |
|---|---|---|
| `OrphanedThinkingOnly` | `filterOrphanedThinkingOnlyMessages` | Any assistant with content that is all-`Reasoning` / `ReasoningFile` (over-conservative: sibling-id keep rule enforced inside `apply`) |
| `TrailingThinking` | `filterTrailingThinkingFromLastAssistant` | Last assistant has a trailing `Reasoning` part |
| `WhitespaceOnly` | `filterWhitespaceOnlyAssistantMessages` | Any assistant whose content is non-empty + all whitespace `Text` |
| `EnsureNonEmptyContent` | `ensureNonEmptyAssistantContent` | Any non-final assistant has empty content |
| `MergeConsecutiveUsers` | `mergeUserMessages` | Any pair of consecutive `User` messages |
| `MergeAssistantsByRequestId` | `messages.ts:2257-2261` | Any pair of consecutive `Assistant` messages with matching non-`None` `request_id` |
| `StripExitPlanModeInjectedFields` | `normalizeToolInputForAPI` (`utils/api.ts`) | Any `ExitPlanMode` tool_call carries `plan` / `planFilePath` injected fields |

### `coco-compact::compact::compact_passes` (2 passes)

| Pass | TS source | `would_mutate` trigger |
|---|---|---|
| `StripImages` | `compact.ts::stripImagesFromMessages` | Any `User` or `ToolResult` carries `FileData` content (image / document) |
| `StripReinjectedAttachments` | `compact.ts:211-223` | Any attachment whose `AttachmentKind::survives_compaction()` returns `false` |

## 7. The trait contract — drift detection

The pipeline's correctness hinges on a single invariant:

> **If `would_mutate` returns `false`, `apply` is a no-op.**

A pass that violates this — for example, `would_mutate` returning
`false` when its `apply` body would in fact have mutated — silently
skips the pipeline's slow path. The user-visible bug is a missing
normalization (consecutive users sent to the API unmerged, trailing
thinking sent to Anthropic and rejected as a 400, etc.). Existing
integration tests catch SOME of these, but only if the test happens
to exercise the specific drift scenario.

`pipeline_invariants` (`core/messages/src/normalize.test.rs`)
addresses this with **two test patterns per pass**:

```rust
#[test]
fn pass_X_clean_is_no_op() {
    assert_clean(Pass::X, /* input where Pass::X's trigger does NOT hold */);
    // assert_clean verifies:
    //   1. would_mutate returns false
    //   2. apply leaves the Vec byte-identical (compared via serde_json)
}

#[test]
fn pass_X_dirty_triggers() {
    assert_dirty(Pass::X, /* input crafted to hit Pass::X's trigger */);
    // assert_dirty verifies: would_mutate returns true
}
```

Adding a new pass requires adding a `clean` + `dirty` test pair — the
test module is structured to make this a one-template-per-pass
addition.

**Note on `Message` equality**: `coco_types::Message` does not impl
`PartialEq` (`AssistantContent` / `LlmMessage` carry `serde_json::Value`
provider blobs that compare as opaque). `assert_clean` compares via
canonical JSON serialization (`serde_json::to_value`) — for our
"identity preserved" check, this is equivalent and zero-friction.

## 8. Adding a pass — checklist

1. **Decide the host crate** — normalize passes live in
   `core/messages/src/normalize.rs::passes`; compact passes live in
   `services/compact/src/compact.rs::compact_passes`.
2. **Add a unit struct** + `impl MessagePass for X`:
   - `would_mutate` should be a single walk, no allocation
   - `apply` delegates to a `pub(crate) fn` algorithm body (so the
     TS-parity algorithm form stays unchanged)
3. **Add the pass to the pipeline call site** — both the `||` chain
   AND the `.apply()` chain, in TS-aligned order
4. **Add a `pass_X_clean_is_no_op` test** + `pass_X_dirty_triggers`
   test in `pipeline_invariants`
5. **Update this doc** if the pipeline's pass count changes

## 9. Design rationale (FAQ)

### Why static dispatch instead of `&[&dyn MessagePass]`?

For the normalize pipeline at `~7 passes × ~1 LLM call/turn`, dyn
dispatch's vtable cost is ~10 ns per turn — totally negligible vs
the LLM call's ~500 ms-2 s. But the `&[&dyn MessagePass]` slice
would make the `apply` order implicit (defined by slice contents) and
the `would_mutate` reduce a separate iteration. The static form keeps
both sequences visible side-by-side in source.

### Why not put `apply` in a macro that emits both chains?

Considered. Macro would eliminate the "two-chain" drift risk
mechanically. Rejected because:
- The macro syntax would obscure the explicit `Pass1.apply(); Pass2.apply();` shape
- We have a tested invariant (`pipeline_invariants`) that catches drift
- Macro complexity outweighs the human-error rate at code review

If a third pipeline gets added and the duplication burden grows, the
macro option can be reconsidered. For 2 pipelines, manual is cleaner.

### Why not make every pass functional (build new Arc-vec, don't mutate)?

Considered. A "pure" pipeline where each pass returns
`Vec<Arc<Message>>` would let the implementation Arc-share unchanged
messages and only allocate new Arcs for touched ones — strictly more
efficient than today's "materialize all, mutate in place" form.

Rejected because:
- The 9 algorithm bodies are TS-parity ports of in-place reducers in
  `utils/messages.ts`. Rewriting them to functional form (return new
  Vec) drifts away from the TS reference; future TS-side changes
  become harder to mirror.
- The materialize cost on the slow path is bounded by the existing
  TS-parity behavior — TS pays equivalent JS-engine cost. We're not
  worse than TS here.
- The fast path already covers the common case (clean turn → zero
  Message clones). The slow path's N×Message::clone is paid only when
  mutation actually happens.

If a future pass demonstrably becomes a hotspot and 100% Arc-share
matters, that specific pass can be implemented as a functional rebuild
(returning a new Vec inside `apply` — the trait doesn't preclude this;
it just gives access to `&mut Vec<Message>`).

### Why two crates (coco-messages + coco-compact) share the same trait?

The trait surface is tiny (2 methods, no associated types, no generic
type parameters). Making it `pub` in `coco-messages` and reused by
`coco-compact` avoids duplicating the trait + helper. The downside —
a public trait — is mitigated by `pub trait MessagePass` being clearly
internal-flavored (no doc-link guarantee, no SemVer commitment beyond
the workspace).

### Why aren't `micro_compact` / `api_microcompact` MessagePass impls?

`micro_compact` runs in a different sequencing context (called by
`teammate_engine.rs` *before* `compact_conversation`, not as part of
the same Arc-vec → Arc-vec transformation). Its caller still uses
`&mut [Message]` directly. Including it in the pipeline would force
an unnecessary materialize round-trip. Left as-is, intentionally.

`api_microcompact` (reactive PTL recovery) mutates history's storage
via `with_owned_messages` and doesn't have a clean Arc-vec → Arc-vec
shape either. Same reasoning.

## 10. Performance characteristics

| Path | `Message::clone` count per call |
|---|---|
| normalize fast path (clean turn) | **0** |
| normalize slow path (any pass triggers) | N (one materialize) |
| compact_conversation skip path (rounds ≤ keep_recent) | **0** |
| compact_conversation full path, no images | N (`recent_rounds[..]` → `working_messages[prefix_len..]` is Arc-share now) — wait, let's double-check: see § Per-path details |
| compact_conversation full path, with images | N + M_image |
| partial_compact_conversation | N + M_image |
| compact_session_memory | M_keep_filtered (only filters, Arc-share) |
| `peel_head_for_ptl_retry` | **0** (Arc-share survivors) |
| `truncate_head_for_ptl_retry` | M_marker (only the PTL marker prepend) |

### Per-path details

**normalize_messages_for_api**:
- Per LLM call (every turn): yes
- Hot-path savings: skip materialize when no pass would mutate
- TS parity: matches `normalizeMessagesForAPI` filter-chain output

**compact_conversation** full path:
- Per compact (~every 50 turns): yes
- `messages_to_keep` + `messages_to_summarize` are built by indexing
  back into `working_messages` (Arc-vec) via prefix-sum on round
  lengths — **zero `Message::clone`** for the split, both halves share
  Arcs with the post-strip Arc-vec
- The `Message::clone` budget is concentrated inside the pipeline's
  slow-path materialize and the per-image rewrite

**`peel_head_for_ptl_retry`** (reactive recovery):
- Per PTL hit: yes
- All survivors are Arc-shared from the input (`messages[prefix_len..]`)
- Zero `Message::clone`, zero pipeline overhead

## 11. Migration history

The pipeline architecture replaced a multi-stage refactor lineage:

| Round | What landed |
|---|---|
| Round 1 (variant-C in compact) | Local optimization: `compact_conversation` skip path Arc-share + `engine_finalize_turn.rs:268` synthetic `Vec::new()` |
| Round 2 (P0–P3 applied piecemeal) | normalize fast/slow path predicate + 4 `_arc` function variants + `CompactResult.messages_to_keep: Vec<Arc<Message>>` + `ArcInput` sealed trait for the compact entry |
| Round 3 (this design) | Unification: `MessagePass` trait + `run_message_passes` helper + 9 pass impls + 3 compact entry points migrated to one canonical call shape + `ArcInput` / `_arc` duplicates / centralized predicate all deleted |

The pre-Round-3 state had **5 distinct API shapes** (`&[Message]`,
`&[Arc<Message>]`, `<M: Borrow<Message>>`, `<M: ArcInput + Borrow<Message>>`,
`&mut Vec<Message>`) and **3 hand-written entry materializes**. Round
3 collapsed those to 3 shapes (read = `&[Arc<Message>]`, transform =
`&[Arc<Message>] → Vec<Arc<Message>>`, read-only utility = `<M: Borrow<Message>>`)
and 1 canonical bridge.

## 12. TS parity

TS `utils/messages.ts:2255-2343` is the spec. Each Rust pass impl's
`apply` body is a direct port of the corresponding TS in-place reducer
— byte-for-byte aligned with the TS algorithm (filter / merge / strip
patterns). The Rust pipeline's `run_message_passes` wrapper is the
Arc-vs-Owned bridge — TS doesn't need this because V8 refcounts
opaquely.

| TS pattern | Rust equivalent |
|---|---|
| `arr.filter(p).map(f)` | `run_message_passes(input, p_any, \|owned\| f_each(owned))` |
| `[...arr]` shallow spread | `input.to_vec()` (Arc-vec) |
| `{ ...message, content: filtered }` shallow object spread | `Arc::new(modified_message)` in slow path |
| In-place `messages[i].field = ...` (TS array mutability) | `apply(&mut Vec<Message>)` with TS-byte-aligned reducer body |

The fast path's behavior (input passes through unchanged when no
mutation would have happened) matches TS's natural behavior — when
TS `filter()` matches nothing or `map()` produces identical objects,
the resulting array is observationally equivalent to the input.

## 13. References

- Implementation: [`coco-rs/core/messages/src/pipeline.rs`](../../coco-rs/core/messages/src/pipeline.rs)
- Pass impls: [`coco-rs/core/messages/src/normalize.rs`](../../coco-rs/core/messages/src/normalize.rs) (`passes` module), [`coco-rs/services/compact/src/compact.rs`](../../coco-rs/services/compact/src/compact.rs) (`compact_passes` module)
- Tests: [`coco-rs/core/messages/src/normalize.test.rs`](../../coco-rs/core/messages/src/normalize.test.rs) (`pipeline_invariants` module — 15 tests)
- Per-crate docs: [`coco-rs/core/messages/CLAUDE.md`](../../coco-rs/core/messages/CLAUDE.md), [`coco-rs/services/compact/CLAUDE.md`](../../coco-rs/services/compact/CLAUDE.md)
- TS source: `claude-code/src/utils/messages.ts:2255-2343` (normalize), `claude-code/src/services/compact/compact.ts:144-223` (strip)
