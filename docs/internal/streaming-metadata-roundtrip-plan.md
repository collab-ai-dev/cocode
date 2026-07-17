# Plan v6: AssistantTurnSnapshot in coco-inference (zero changes to vercel-ai/ai)

## Context

**Bug.** Gemini-3 attaches `thoughtSignature` to every content part — Text, ToolCall, File, Reasoning, ReasoningFile. coco-rs's streaming path preserves it **only on Reasoning** (via `StreamEvent::ReasoningEnd { provider_metadata }`). On every other variant it is silently discarded at two seams:

1. `services/inference/src/stream.rs:181-217` — `stream_event_from_part` strips `provider_metadata` via `..` patterns; `LanguageModelV4StreamPart::ToolCall(tc)` falls to `_ => None`.
2. `app/query/src/engine.rs:1521,1544` — `ToolCallPart.provider_metadata` and `TextPart.provider_metadata` hardcoded to `None`.

**Intended outcome.** A coco-rs-owned `AssistantTurnSnapshot` (defined inside `coco-inference`) becomes the single source of truth for end-of-turn assistant reconstruction. It rides on `StreamEvent::Finish` as `Arc<AssistantTurnSnapshot>` (one clone per turn). Mid-stream tool execution keeps its existing local `tool_buffers`. `vercel-ai/ai` is not modified.

## Architecture choice — Path B (coco-inference-owned)

The plan went through 5 prior iterations (v1 → v5) that all modified `vercel-ai/ai/src/stream/snapshot.rs`. v6 pivots to **Path B**: leave `vercel-ai/ai` entirely untouched and own the snapshot in `coco-inference`. This pivot was triggered by the user's question "if not using vercel-ai/ai/stream/snapshot, would wrapping it in coco-inference be simpler?".

### Why Path B over Path A

**Verified by Phase-1 Explore agent ("Path B feasibility audit"):**

1. **Raw parts arrive with full fidelity at the coco-inference layer.** `processor.next().await` returns `(LanguageModelV4StreamPart, &StreamSnapshot)` — `part.provider_metadata` is intact. coco-inference already ignores the `&StreamSnapshot` half (`stream.rs:95` reads `(part, _)`).
2. **`StreamProcessor` is consumed for two things only:** `.next()` (part stream iterator) and `.metrics()` (stall/health). Both keep working. `.snapshot()` is never called by coco-inference (zero grep matches).
3. **`ProcessorState::update` is pure-functional pattern match** — re-implementable in coco-inference with no hidden dependencies.
4. **Re-export seam is explicitly designed for this:** `services/inference/CLAUDE.md`: "Thin multi-provider LLM client wrapper over `vercel-ai`. Generic retry, usage aggregation, cache-break detection — and nothing Anthropic-specific…"
5. **No required lifecycle hooks** — `StreamProcessor` has no `finalize()`; `.next()` is a pure iterator. Path B treats it that way.
6. **Dual accumulation is safe** — `ProcessorState`'s mutation is private; coco-inference accumulates after the part is yielded. No data races.

### Eliminated findings (vs v5)

| v5 finding | Why it disappears in v6 |
|---|---|
| **A6** `snapshot.text` must stay incrementally maintained | vercel-ai/ai `StreamSnapshot` unchanged; coco-inference defines its own type, no inheritance of that contract. |
| **B2** Existing test invariants (`processor.test.rs:395` etc.) | vercel-ai/ai tests untouched; nothing to regress. |
| **B3** `completed_tool_calls() -> Vec` signature break | vercel-ai/ai helpers untouched. |
| **D1** `ReasoningOutput.signature` orphan in SDK convert path | vercel-ai/ai/generate_text not modified; D1 is a pre-existing independent bug, separate PR. |
| Backwards-compat shims (`pub text: String` cached field, derived getters) | None needed. |
| `vercel-ai/ai/tests/live/` regression sweep | Zero diff to that directory. |
| `vercel-ai/ai/CLAUDE.md` doc updates | None. |
| `ReasoningSnapshot.signature` field removal sweep | Not removing it. |

### Preserved findings from prior reviews

| Finding | v6 handling |
|---|---|
| Bug (Gemini metadata on ToolCall/Text) | Fixed — accumulator in coco-inference captures provider_metadata on every part. |
| P0-1 mid-stream tool execution | `tool_buffers` + `tool_order` preserved in engine.rs; reconstruction path is the only thing switched. |
| P0-2 watch O(N²) | Snapshot rides on `StreamEvent::Finish` as `Arc<AssistantTurnSnapshot>` — one allocation per turn. |
| P1-3 `synthetic_stream_from_content` patch | Still needed — same scope, same change (propagate metadata + emit ToolCall close + cover Source/ReasoningFile/ToolApprovalRequest/Custom). Lives in `coco-inference/src/stream.rs:254`, not in vercel-ai. |
| P1-5 `is_input_complete \|\| is_complete` filter | Applied in coco-inference's accumulator. |
| P2-6 emission order + 9 variants | `TurnPart` enum covers 8 relevant variants (excluding `ToolResult` which providers don't emit as stream parts). |
| P2-7 multiple reasoning segments | Multiple `TurnPart::Reasoning` entries supported. |
| A1 `Arc` clone cost | `Arc<AssistantTurnSnapshot>` on Finish. |
| A2 `ToolApprovalRequest` | `TurnPart::ToolApprovalRequest` variant. |
| A3 input_json divergence | Same contract: tool_buffers and snapshot consume same delta stream; `ToolCall(tc)` close canonical. |
| A4 None-overwrites-Some | Same rule: prefer close's metadata only when `Some`. |
| A5 cancellation | Same: `engine.rs:1379` skips reconstruction on cancel. |
| A8 multi-text outbound | Same test, but now correctly placed in provider crates (B4 fix). |
| B1 `response_text` cannot be deleted | Same: keep `response_text` local for Stop hook / log / QueryResult. |
| B4 provider-serializer tests in wrong crate | Same fix: tests in `vercel-ai/anthropic` and `vercel-ai/google`, not in `app/query`. |
| B5 File data type mapping | Same rule: stream-source emits `LanguageModelV4FileData::Data { data }`. |
| Cache-OK verdict from cache-correctness audit | Still applies — cache hashes don't see metadata. |
| C1-C4 downstream text-extraction consumers | Same fixes — these don't depend on which layer owns the snapshot. |
| D3 session persistence round-trip | Same verification. |
| D4 fork cache parity | Same regression test. |
| D5 v4 internal contradiction (legacy ToolCallSnapshot missing fields) | Disappears — no legacy slots in v6's `AssistantTurnSnapshot`. |

## Architecture diagram

```
provider
  │ LanguageModelV4StreamPart (carries provider_metadata natively)
  ▼
vercel-ai StreamProcessor + ProcessorState
  │ UNCHANGED ↘
  │           processor.snapshot() never read by coco-rs
  │ processor.next() yields (part, _)
  ▼
coco-inference::process_stream_with_config
  │
  │ ┌────────────────────────────────────────────┐
  │ │ NEW: AssistantTurnSnapshotState            │
  │ │ ── per-part accumulator                    │
  │ │ ── HashMap<id, idx> for text/reasoning/tool│
  │ │ ── pushes TurnPart into ordered Vec        │
  │ │ ── captures provider_metadata on every part│
  │ │ ── is_input_complete / is_complete tracking│
  │ │ ── idempotent on duplicate *Start          │
  │ └────────────────────────────────────────────┘
  │
  │ EXISTING: stream_event_from_part → StreamEvent → mpsc (UI flow unchanged)
  │
  │ At Part::Finish:
  │   wrap state.snapshot in Arc, attach to
  │   StreamEvent::Finish { ..., snapshot: Arc<AssistantTurnSnapshot> }
  ▼
coco-query::engine
  │ UI path: existing StreamEvent → ServerNotification fan-out
  │ Mid-stream tool exec: existing tool_buffers + handle.feed_plan
  │ History reconstruction: at Finish, walk snapshot.parts in order
  │   → assistant_content_from_snapshot(&snapshot) -> Vec<AssistantContentPart>
  ▼
Message::Assistant with full provider_metadata fidelity + emission order intact
```

## Critical files (4 layers)

### Layer 1 — coco-inference (new types + accumulator)

| File | Change |
|---|---|
| `coco-rs/services/inference/src/stream.rs` | **Add types**: `pub enum TurnPart { Text(TextSegment), Reasoning(ReasoningSegment), ToolCall(ToolCallSegment), File(FileSegment), ReasoningFile(ReasoningFileSegment), Source(SourceSegment), Custom(CustomSegment), ToolApprovalRequest(ToolApprovalRequestSegment) }`. Each segment carries `id`, payload-specific fields, and `provider_metadata: Option<ProviderMetadata>`. `ToolCallSegment` additionally: `tool_name`, `input_json: String`, `provider_executed: Option<bool>`, `dynamic: Option<bool>`, `is_input_complete: bool`, `is_complete: bool`. `pub struct AssistantTurnSnapshot { pub parts: Vec<TurnPart> }`. **Add accumulator**: `struct AssistantTurnSnapshotState { snapshot: AssistantTurnSnapshot, active_text: HashMap<String, usize>, active_reasoning: HashMap<String, usize>, active_tool: HashMap<String, usize> }`. `impl AssistantTurnSnapshotState { fn update(&mut self, part: &LanguageModelV4StreamPart) }` — pattern match on every `LanguageModelV4StreamPart` variant; captures `provider_metadata` per part; idempotent on duplicate `*Start`; first-wins metadata merge on deltas; prefer `ToolCall(tc)`-close `provider_metadata` and `input` only when `Some` (A4); `LanguageModelV4FileData::Data { data }` wrap for File/ReasoningFile (B5). **Modify `process_stream_with_config`**: instantiate `AssistantTurnSnapshotState` once per call; call `state.update(&part)` immediately after `processor.next()` yields a part and BEFORE `stream_event_from_part` (line 97). At `Part::Finish`, wrap `state.snapshot` in `Arc::new(...)`. **Modify `StreamEvent::Finish`**: add `pub snapshot: Arc<AssistantTurnSnapshot>` field. **Patch `synthetic_stream_from_content` (P1-3)**: propagate `provider_metadata` from source `AssistantContentPart` to every emitted part; emit `ToolCall(tc)` close after `ToolInputEnd`; add explicit emission arms for `Source`, `ReasoningFile` (with `LanguageModelV4FileData::Data` wrap), `ToolApprovalRequest`; `Custom` skipped with trace log. |
| `coco-rs/services/inference/src/lib.rs` | Re-export: `AssistantTurnSnapshot`, `TurnPart`, all `*Segment` types. |
| `coco-rs/services/inference/src/stream.test.rs` | New tests: per-segment metadata accumulation for each variant; multiple reasoning segments with distinct signatures; emission order preservation through text↔tool↔text interleaving; `is_input_complete && !is_complete` from omitted `ToolCall(tc)` close still appears in `parts`; `ToolCall(tc).provider_metadata == None` does not overwrite earlier `Some`; duplicate `*Start` idempotency; `synthetic_stream_from_content` roundtrip preserves metadata; File data wrap. |
| `coco-rs/services/inference/CLAUDE.md` | Document `AssistantTurnSnapshot` as the canonical end-of-turn history source; explain the dual relationship to vercel-ai's untouched `StreamSnapshot`. |

### Layer 2 — coco-query (the consumer)

| File | Change |
|---|---|
| `coco-rs/app/query/src/engine.rs` | **KEEP** `tool_buffers`, `tool_order`, mid-stream `ToolCallEnd → prepare_one_pending_tool_call → handle.feed_plan` (lines 1213-1334). **KEEP** `response_text` (Stop hook input at 1691,1694; log at 1739; `QueryResult.response_text` at 1765 — B1). **KEEP** `reasoning_text` if any downstream reader (sweep — preliminary scan: reconstruction-only, safe to delete; verify). **DELETE** `reasoning_provider_metadata` (replaced by snapshot read). **REPLACE** end-of-turn reconstruction at lines 1466-1567: extract `event.snapshot: Arc<AssistantTurnSnapshot>` at `StreamEvent::Finish`; new private helper `fn assistant_content_from_snapshot(snap: &AssistantTurnSnapshot) -> Vec<AssistantContentPart>` walks `snap.parts` in order, mapping each `TurnPart` variant to the corresponding `AssistantContentPart` with `provider_metadata` preserved. Tool-call filter: `is_input_complete \|\| is_complete`; prefer `ToolCall(tc)`-close data when both Some (A4). Use existing `parse_tool_input` repair logic; reject malformed JSON with `tracing::warn!` + skip. Helper ~60 lines, exhaustive match. |
| `coco-rs/app/query/src/single_turn.rs`, `agent_query.rs`, `forked_agent.rs` | No change. |

### Layer 3 — downstream text-extraction consumers

| File | Change |
|---|---|
| `coco-rs/core/messages/src/history.rs:50-68` (C1) | `last_assistant_text()` — change `.join("")` to insert `\n` between text parts and `[tool: <name>]` placeholder lines for non-text parts. Document new contract. |
| `coco-rs/services/compact/src/tokens.rs:181-193` (C2) | `extract_llm_message_text()` for Assistant content: insert `[tool: <name>]` placeholders in emission order. |
| `coco-rs/app/query/src/engine_compaction.rs:655-664` (C2 mirror) | Same fix in the inline summary-input extraction. |
| `coco-rs/app/query/src/tool_call_preparer.rs:502-511` (C3) | Permission classifier text input: insert tool-call boundary placeholders. |
| `coco-rs/app/query/src/hook_llm.rs:196-206` (C4) | Add `debug_assert!` documenting single-text expectation on hook LLM responses. |

### Layer 4 — tests

| File | Purpose |
|---|---|
| `coco-rs/app/query/tests/streaming_metadata.rs` | **New unit test file.** All scenarios use `ScriptedMock` + patched `synthetic_stream_from_content`. **Scenarios:** (1) `Reasoning(signature=S1)` + `ToolCall(thoughtSignature=T1)` round-trip; (2) `Text(thoughtSignature=Tx)` survives on `TextPart`; (3) interleaved `[Text(M1), ToolCall, Text(M2)]` preserves order + per-part metadata (P2-6 regression); (4) two `Reasoning` segments with distinct signatures both survive (P2-7); (5) `ToolInputStart/Delta/End` without `ToolCall(tc)` close still produces a ToolCallPart in history (P1-5); (6) `ToolCall(tc).provider_metadata = None` doesn't overwrite earlier `Some` (A4); (7) mid-stream tool execution — `Tool::execute` invoked before `Finish` (P0-1). 7 scenarios; all fail on `main`. |
| `coco-rs/vercel-ai/anthropic/src/messages/convert_to_anthropic_messages.test.rs` | A8: feed `[Text(A), ToolCall, Text(B)]` to converter; assert two text blocks in wire body. |
| `coco-rs/vercel-ai/google/src/convert_to_google_generative_ai_messages.test.rs` | Same A8 for Google. |
| `coco-rs/app/session/tests/...` (D3) | Round-trip test: serialize `Message::Assistant` with `Reasoning.provider_metadata = Some({...signature...})` to JSONL; deserialize; assert metadata equal byte-for-byte. |
| `coco-rs/app/query/tests/fork_cache_parity.rs` (D4) | Multi-text assistant message in parent turn 1; spawn fork; assert fork's outbound `messages[]` shape byte-identical to parent's next-turn equivalent. |
| `coco-rs/tests/live/tests/sdk/suite/streaming_metadata.rs` | Live: Scenario A (Gemini-3, `max_tokens: 2048`, force `get_weather` tool call, assert `thoughtSignature` non-empty in history, turn 2 round-trips without 4xx); Scenario B (Anthropic / deepseek-anthropic with `thinking_level: Medium`). |
| `coco-rs/tests/live/tests/sdk_google.rs`, `sdk_deepseek.rs` | Wire-up entries for new live suite. |
| `coco-rs/services/inference/src/stream.test.rs` | C2 placeholder-insertion regression test in `extract_llm_message_text`. |

## Re-used existing infrastructure

- `LanguageModelV4ToolCall` carries `provider_metadata`/`provider_executed`/`dynamic` natively — `vercel-ai/provider/src/language_model/v4/tool_call.rs:13-31`.
- Google emits `thoughtSignature` on every relevant part — `vercel-ai/google/src/google_generative_ai_language_model.rs:1282,1320,1328,1352,1357,1361,1365`. **No change.**
- Anthropic emits `signature` on `ReasoningEnd` — `vercel-ai/anthropic/src/messages/anthropic_messages_language_model.rs:2265`. **No change.**
- Outbound serializers already read `provider_metadata` per part — `convert_to_google_generative_ai_messages.rs:233+`, `convert_to_anthropic_messages.rs:861-874`. **No change.**
- `coco-messages` preserves `provider_metadata` through history. **No change.**
- `app/query/tests/mock_harness.rs::ScriptedMock` — usable once `synthetic_stream_from_content` is patched.
- `prepare_one_pending_tool_call` + `handle.feed_plan` — unchanged.
- `parse_tool_input` + JSON-repair — reused in `assistant_content_from_snapshot`.
- `StreamProcessor` + `ProcessorState` (vercel-ai/ai) — UNCHANGED. coco-inference uses `.next()` and `.metrics()` only.

## Backwards compatibility

1. **`StreamEvent::Finish` gains `snapshot: Arc<AssistantTurnSnapshot>` field.** Four match sites in coco-rs need `snapshot: _` ignore pattern. `StreamEvent` is internal-only (no external wire). Grep-and-fix in one pass.
2. **No changes to vercel-ai/ai public types or behavior** — all upstream Vercel-ai consumers unaffected.
3. **No backwards-compat shims, no field renames, no signature changes** outside coco-rs's internal seam.
4. **Engine-level locals** (`response_text`, `reasoning_text`, `tool_buffers`, `tool_order`) preserved — only `reasoning_provider_metadata` deleted (replaced by snapshot).

## Phasing

Single PR, full v6. Sequencing inside the PR (each commit independently buildable + green):

1. Commit 1 — Layer 1 (types + accumulator + `synthetic_stream_from_content` patch + stream.test.rs unit tests).
2. Commit 2 — Layer 2 (engine.rs reconstruction switch).
3. Commit 3 — Layer 3 (downstream consumer fixes C1-C4).
4. Commit 4 — Layer 4 unit + session-roundtrip + fork-parity tests.
5. Commit 5 — Layer 4 live tests + sdk_google.rs / sdk_deepseek.rs wire-up.

## Verification

### Build / lint gates

```bash
cd coco-rs
just pre-commit                            # fmt + clippy + nextest workspace-wide
just test-crate coco-tests-live            # smoke-compile live suite
```

### Unit-level regression (must fail on main, pass on fix)

```bash
cargo test -p coco-query --test streaming_metadata       # all 7 scenarios
cargo test -p coco-inference --lib stream::tests          # accumulator + synthetic_stream patch
cargo test -p vercel-ai-anthropic --lib convert_to_anthropic_messages::tests
cargo test -p vercel-ai-google --lib convert_to_google_generative_ai_messages::tests
```

### Session persistence + fork parity

```bash
cargo test -p coco-session
cargo test -p coco-query --test fork_cache_parity
```

### Live end-to-end

```bash
for i in 1 2 3 4 5; do
  cargo test -p coco-tests-live --test streaming_metadata test_gemini_signature_roundtrip -- --nocapture
done
cargo test -p coco-tests-live --test streaming_metadata test_anthropic_signature_roundtrip -- --nocapture
```

### Wire-level reproducer

```bash
COCO_LOG=vercel_ai_google=trace,coco_inference=debug \
  cargo test -p coco-tests-live --test sdk_google test_streaming_tools_google \
  -- --nocapture 2>&1 | grep -A2 thoughtSignature
```

Before: absent from turn-2 outbound body. After: present on `functionCall` part.

### Mid-stream execution latency (P0-1 regression guard)

```bash
cargo test -p coco-query --test streaming_metadata test_mid_stream_tool_execution -- --nocapture
```

Asserts `Tool::execute` invoked before `StreamEvent::Finish`. Catches accidental deletion of `tool_buffers`.

## Out of scope (explicit)

- **`vercel-ai/ai/src/stream/snapshot.rs` cleanups**: dead `signature: Option<String>`, missing `ToolCallSnapshot.provider_metadata`, `StreamSnapshot` being unused in production — all real, all separate PR ("vercel-ai/ai snapshot maintenance").
- **`vercel-ai/ai/src/generate_text/reasoning_output.rs:25` signature wiring** (D1): independent SDK-level bug; separate PR.
- **Non-streaming `client.query()` callers**: already preserve metadata via `do_generate`.
- **`max_tokens` default bumps** in streaming tests: separate concern.
- **`AISdkError` struct → enum migration**: separate PR.
- **Plan-mode attachment metadata preservation** (D6): pre-LLM input path, different concern.
- **Cross-provider thinking-continuity stress matrix**: follow-up.

## Plan history

- **v1** (initial): collapse to single `StreamSnapshot` accumulator via watch channel.
- **v2** (third-party review): keep `tool_buffers`; fix P0-1, P1-3, P1-5; `Vec<SnapshotPart>` for order + multi-reasoning.
- **v3** (attacker review): `Arc<StreamSnapshot>` instead of per-part clone; add `ToolApprovalRequest`; address API-shape risks.
- **v4** (downstream-caller / API-shape review): keep `response_text`; keep `snapshot.text` incremental; move A8 tests to provider crates; `File` data wrap rule.
- **v5** (cache + downstream consumer + SDK convert review): cache-OK verdict; C1-C4 downstream consumer fixes; D1-D5 deeper bugs surfaced.
- **v6** (Path-B pivot): leave vercel-ai/ai untouched; coco-inference owns the snapshot. Eliminates 4 prior findings (A6, B2, B3, D1) and ~300 lines of compat shims. Net: simpler, lower-risk, better-aligned with coco-rs seam architecture.
- **v7** (post-v6 cleanup): the "no external SDK consumer" premise was verified — `StreamSnapshot` / `ProcessorState` / `ToolCallSnapshot` / `ReasoningSnapshot` / `FileSnapshot` / `SourceSnapshot` had **0 production callers in coco-rs** after v6 landed. Leaving them in place created dual accumulation: every `LanguageModelV4StreamPart` was walked twice (once by `ProcessorState`, once by `AssistantTurnSnapshotState`), and the upstream snapshot was silently discarded by `Ok((part, _))`. v7 deletes the entire vendor-side accumulator: `vercel-ai/ai/src/stream/snapshot.rs` and `processor_state.rs` are gone, `StreamProcessor` is reduced to an idle-timeout + metrics adapter over `Stream<LanguageModelV4StreamPart>`, and `.next()` now returns just `Option<Result<LanguageModelV4StreamPart, AISdkError>>`. Net change: −284 LOC of dead code, single source of truth (coco-inference) for assistant-turn state, vendor layer's responsibility shrinks to its actual job (protocol shape + timeout + metrics). Plan rationale corrected: the right reason for upper-layer ownership is **accumulation is consumer policy, not vendor protocol** — different consumers want different accumulators, baking one into the SDK privileges one consumer's view.
