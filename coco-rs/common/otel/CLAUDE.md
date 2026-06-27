# coco-otel

OpenTelemetry tracing and metrics: OTLP gRPC/HTTP export, base events, metrics client with RAII timers.

## Key Types

- `OtelManager` — top-level handle with metadata, session span, optional `MetricsClient`
- `OtelEventMetadata` — conversation_id, provider, model, auth_mode, app_version, terminal_type
- `OtelProvider` — composed logger / tracer / metrics exporters
- `MetricsClient`, `MetricsConfig`, `MetricsError`, `Timer` (RAII duration recording)
- `ToolDecisionSource` (Config / User)
- `subscriber::{init_subscriber, init_for_tests, SubscriberOpts, SubscriberHandle, Mode, Format}`
  — global `tracing` registry bootstrap. Binaries call exactly once from `main()`.
- Modules: `config` (`OtelSettings`, `OtelExporter`, `OtelTlsConfig`), `otel_provider`, `events`, `traces`, `metrics`, `subscriber`, `otlp` (exporter builders)

## Current Scope

- **Shipped**: export pipeline (OTLP gRPC/HTTP, TLS, Statsig, InMemory) + 7 base events (conversation, prompt, tool_decision, tool_result, api_request, sse_event, completion).
- **Not yet shipped**: higher-layer span hierarchy, business metrics, custom exporters (BigQuery / Perfetto / first-party event logger), operational controls (sampling / killswitch / PII safety / opt-out). See `docs/coco-rs/crate-coco-otel.md` for the full roadmap.

## Conventions

- Uses `thiserror` (not snafu).
- `Timer` auto-records duration on Drop (safer than manual start / end).
- Tag keys / values validated via `validate_tag_key` / `validate_tag_value` before emission.
- `metrics_use_metadata_tags` flag controls whether provider / model / auth_mode are auto-tagged on every metric.

## Subscriber bootstrap

The `subscriber` module owns the single registered global `tracing`
subscriber for the binary. **Without it, every `tracing::*` call is a
no-op.** Library and test code MUST NOT call `init_subscriber` —
binaries call it exactly once from `main()` after parsing CLI args.

### Sink defaults per mode

| Mode      | File sink (rotating daily)         | Stderr sink |
|-----------|------------------------------------|-------------|
| `Tui`     | on (`<config_home>/logs/coco.log`) | off (opt-in via `also_stderr`) |
| `Sdk`     | on                                 | off (stdout owns NDJSON; opt-in via `also_stderr`) |
| `Headless`| on                                 | on |
| `Skip`    | —                                  | —  (no subscriber installed) |

Rationale: TUI owns the screen via ratatui; SDK owns stdout via the
NDJSON RPC channel — logs would corrupt either.

### Filter resolution priority

`--log-level` > `COCO_LOG` > `RUST_LOG` > `settings.log.level` >
`subscriber::DEFAULT_FILTER` (`coco=debug,info`). A bare level
(e.g. `--log-level=debug` or `"log": { "level": "debug" }`) expands
to `coco=debug,debug` so coco crates stay verbose without flooding
third-party output. Full `EnvFilter` directives pass through verbatim.

### Layout toggles: `--log-location` (`COCO_LOG_LOCATION`)

`SubscriberOpts.location` + `.thread_names` add `filename:line` and the
thread name to each event (~30–80 bytes per line).
Tri-state resolution in `app/cli/src/tracing_init.rs`:

```
explicit --log-location > COCO_LOG_LOCATION > settings.log.location > auto-rule
```

Auto-rule: when the resolved static filter enables DEBUG or TRACE for
any `coco*` target, the toggle defaults to on. This includes both bare
levels and full directives such as `coco=debug,info`. Thread name
follows `location` byte-for-byte; there's intentionally no separate
flag.

Errors won't carry the **caller** site though — `file!()/line!()`
points at the `tracing::*` macro call, not the error origin. For
that, see `#[stack_trace_debug]` in `common/stack-trace-macro`.

## Logging conventions (workspace-wide)

These apply to every crate that emits `tracing::*` events.

### Levels

- `error!` — user-visible failure that aborts a turn or session.
- `warn!`  — recoverable / unexpected (retry engaged, hook denied, fallback triggered).
- `info!`  — lifecycle milestones: session start, turn start/end, model switch, compaction, tool batch.
- `debug!` — per-operation detail: classifier stage, hook match, queue drain item.
- `trace!` — per-chunk / per-token / per-SSE event. Off by default — opt-in via filter (`coco_inference::stream=trace`).

### Standard field names

Always use these spellings — consistency lets ops pivot on a field
across crates. `%` for `Display`, `?` for `Debug`. Never embed
JSON-stringified blobs in the message text.

```
session_id, turn_id, message_id, tool_call_id, tool_name, batch_type,
request_id, provider, model_id, duration_ms, tokens_in, tokens_out,
cache_hit, retry_count, attempt, permission_decision, hook_event, mcp_server
```

### `#[instrument]` policy

Adopt selectively. Use `#[tracing::instrument(skip_all, fields(...))]`
**only** on the seven canonical span anchors:
`session`, `turn`, `api_call`, `tool_call`, `compaction`, `hook_event`,
`mcp_lifecycle`. Sprinkling on small helpers creates span overhead and
noise. Inside instrumented fns, prefer flat `info!`/`debug!` events.

### Span hierarchy

```
session > turn > api_call > sse_chunk
session > turn > tool_batch > tool_call > sandbox_exec
```

Hooks / permissions / compaction stay as flat events under the
current span unless they own substantial work worth a child span.

### Secret safety

Any HTTP body, header, or env-var dump goes through
`coco_secret_redact::redact_secrets` (`utils/secret-redact/src/lib.rs:122`)
before logging — and only at `trace!`. Never log a raw `Authorization`
or `x-api-key` value at any level.
