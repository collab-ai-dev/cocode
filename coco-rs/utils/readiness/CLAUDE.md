# coco-utils-readiness

Async one-shot readiness flag. Multiple subscribers hold tokens; any token holder can mark ready (monotonic true), all waiters are woken via `tokio::sync::watch`.

## Key Types
- `Readiness` trait — `is_ready`, `subscribe`, `mark_ready(Token)`, `wait_ready`
- `ReadinessFlag` — default impl (atomic bool + `Mutex<HashSet<Token>>` + `watch::Sender`)
- `Token` — opaque `i32` subscription handle
- `ReadinessError` — `TokenLockFailed` / `FlagAlreadyReady`
