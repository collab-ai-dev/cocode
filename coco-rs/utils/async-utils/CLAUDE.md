# coco-async-utils

Async cancellation helpers built on `tokio_util::CancellationToken`, plus a
workspace-shared injectable clock for deterministic time in tests.

## Key Types

| Type | Purpose |
|------|---------|
| `OrCancelExt` | Extension trait: `future.or_cancel(&token)` races a future against cancellation |
| `CancelErr::Cancelled` | Returned when the token fires before the future completes |
| `Clock` | Read-only clock trait (`now() -> Instant`, `now_unix_millis() -> i64`). Production code takes `&dyn Clock` / `Arc<dyn Clock>` and reads time through it |
| `SystemClock` | Production impl reading the OS clock |
| `TestClock` | Deterministic clock (pin + `advance_millis`), gated behind the `testing` feature; downstream crates enable it in `[dev-dependencies]` |

## Clock seam

The workspace-shared counterpart to `coco-tui-ui`'s TUI-scoped clock (that crate
is a leaf UI primitive others must not depend on). Adopt it where wall-clock
time makes a unit test flaky: thread `&dyn Clock` to the time-reading fn,
default production callers to `&SystemClock`, inject `TestClock` in tests. First
adopter: `services/rmcp-client/src/oauth.rs` token-expiry math.
