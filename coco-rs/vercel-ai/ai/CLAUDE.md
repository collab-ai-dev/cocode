# vercel-ai

High-level SDK matching `@ai-sdk/ai` v4: `generate_text` / `stream_text` /
`generate_object` / `stream_object` / `embed` / `rerank` / `generate_image` /
`generate_speech` / `generate_video` / `transcribe`, plus middleware, provider
registry, and model handles. Builds on `vercel-ai-provider` types +
`vercel-ai-provider-utils` helpers.

**coco-rs production calls do not route through this crate.**
`services/inference::ModelRuntime` calls provider adapters directly via
`LanguageModelV4::do_generate` / `do_stream`. Treat this crate as the SDK /
spec facade and test/live compatibility surface; agent-loop behavior belongs
in `services/inference` and `app/query`, not in `generate_text` /
`stream_text`.

## SDK Spec

Implements the `@ai-sdk/ai` v4 specification (subset). Skipped upstream
subdirs (`agent/`, `ui/`, `upload-*`), baseline commit, and intentional
deviations: see [`../README.md`](../README.md). Anthropic-specific concerns
(OAuth, policy limits, 529 retry, …) belong in `vercel-ai-anthropic`, not here
— see "Multi-Provider Boundaries" in the workspace `CLAUDE.md`.

## Invariants

- **`StreamProcessor` does no content accumulation.** It is a thin adapter —
  idle-timeout + ttft/stall metrics around a
  `Stream<LanguageModelV4StreamPart>`. Per-stream snapshots embed consumer
  policy (which parts matter, which metadata to preserve), so each consumer
  owns its own accumulator (e.g. `coco_inference::AssistantTurnSnapshot`).
- **Callbacks are NOT bridged into `CoreEvent`.** Callbacks in
  `generate_text/callback.rs` fire at the provider boundary; the agent loop
  (`QueryEngine`) consumes them internally and re-emits `AgentStreamEvent` /
  `ServerNotification`. Trace correlation uses shared `session_id` /
  `turn_id` context. See `docs/internal/event-system-design.md` §1.7.

## Idiom mapping

TS→Rust: `Promise<T>` → `impl Future`, `ReadableStream<T>` →
`Pin<Box<dyn Stream>>`, unions → enums, `Record<string, T>` → `HashMap`,
`AbortSignal` → `CancellationToken`.
