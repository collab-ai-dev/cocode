# coco-error

Unified error handling: `StatusCode` classification, `ErrorExt` trait, snafu + virtual stack traces. Rust-only infrastructure — no TS counterpart.

## Key Types

- `StatusCode` — 5-digit `XX_YYY` classification (categories: General 00-05, Config 10, Provider 11, Resource 12). Full list in `common/error/README.md`.
- `ErrorExt` trait — `status_code()`, `is_retryable()`, `retry_after()`, `output_msg()`. Future: `telemetry_msg()` for PII-safe logging.
- `StackError`, `BoxedError`, `BoxedErrorSource`, `PlainError`, `boxed` / `boxed_err` helpers.
- Re-exports `snafu`, `snafu::Location`, and `#[stack_trace_debug]` proc macro.

## Error Code Reference

See [common/error/README.md](README.md) for the full `StatusCode` catalog.
