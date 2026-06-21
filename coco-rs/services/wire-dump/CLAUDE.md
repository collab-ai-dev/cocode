# coco-wire-dump

Per-session raw LLM wire-traffic recorder for debugging. Captures +
redacts + classifies a request/response, then hands a redacted
`WireRecord` to a pluggable `WireSink` (default: `FileSink` → files under
`<session_dir>/wire/`).

## Provider-agnostic by design

This crate knows **nothing** about the LLM transport or the `WireTap`
trait — it exposes inherent capture methods (`on_request`,
`on_response_chunk`, `on_response_body`, `finish`). The bridge to the
provider transport's `vercel_ai_provider::WireTap` sink lives in
`app/query` (`wire_tap_adapter::WireTapAdapter`), so this crate depends
on **neither `coco-inference` nor `vercel-ai-*`**.

Deps: `coco-config` (the `WireDumpLevel` enum), `coco-secret-redact`,
`serde_json`, `tokio` (off-thread writes), `tracing`. The only "upward"
dep is `coco-config` (Common) — which is why it sits in `services/` for
now. If `WireDumpLevel` were inlined as a crate-local enum, this would
become a true `utils/` leaf; kept here until a second consumer makes
that move worthwhile.

## Architecture: two seams

```
consumer (app/query WireTapAdapter / MCP / a debug CLI)
   │  on_request / on_response_chunk / on_response_body / finish
   ▼
SessionWireRecorder   ── capture (bounded) + redact + classify ──►  WireRecord (redacted, no I/O)
   │  WireSink::emit(&record, persist_bodies)
   ▼
WireSink   ──►  FileSink (default)  |  custom (tests: in-mem; future: remote/stdout)
```

**Redaction happens before the sink** — no sink, default or custom, can
observe a secret.

## Key Types

- `WireDumpConfig` — session-scoped config + seq counter + `Arc<dyn WireSink>`.
  `new(session_dir, level, max_body_bytes, redact)` → `FileSink`;
  `with_sink(sink, …)` injects a custom sink. `begin(ctx)` mints one
  recorder per call.
- `SessionWireRecorder` — capture/redact/classify core. Inherent
  `on_request` / `on_response_chunk` / `on_response_body` / `finish`.
- `WireRecord` — redacted, ready-to-persist data handed to the sink.
- `WireSink` (trait) + `FileSink` (default). Implement `WireSink` to send
  captures elsewhere; redaction is already applied.
- `WireOutcome` (`Success` / `Failure`) — the typed completion the
  consumer passes to `finish`; authoritative for the persist decision so
  `level=error` never guesses from bytes.
- `WireTurnCtx` — `{ turn_id, provider, model }` identity for one call.

## Lifecycle

- `on_request` resets the response buffers → a retried call captures the
  **final** attempt, not a concatenation.
- `finish(outcome)` writes on `tokio::task::spawn_blocking` (off the async
  runtime). Idempotent with `Drop`.
- `Drop` is the safety net for paths that never call `finish`
  (cancellation, a failed stream *open*): synchronous write, byte
  heuristic for the outcome (non-2xx status / in-band `error` marker).

The engine wiring (`LoopTurnState` field, create-in-`enter_turn`,
`finish`-in-`consume_stream`, the `WireTap` adapter) all stays in
`app/query`. This crate owns only capture + sink.

## On-disk layout (FileSink)

```text
<session_dir>/wire/
  index.jsonl                  # one line per call (always)
  0003-turn-3-openai.req.json  # redacted request body (on failure, or level=all)
  0003-turn-3-openai.resp.txt  # raw response bytes (redacted)
  0003-turn-3-openai.meta.json # status / outcome / redacted headers / sizes
```

`error` persists the body triplet only on failure (every call still gets
an `index.jsonl` line); `all` persists every call; `off` means no
recorder is constructed (zero overhead). `FileSink` `tracing::warn!`s on
any write failure — a debug tool that can't write its dump must say so.

## Conventions

- **No env reads** — verbosity comes from `coco_config::DiagnosticsConfig`
  (`COCO_DIAGNOSTICS_WIRE_DUMP` / `diagnostics.wire_dump`).
- **Infallible API** — diagnostics must never perturb the live request
  path; capture/`finish`/sink swallow their own I/O errors (after logging)
  rather than returning `Result`, so no `coco-error`/snafu dep.
- **Redaction on by default** (`coco-secret-redact`) over bodies and
  header values (where the API key actually lives).
