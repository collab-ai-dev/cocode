# coco-async-utils

Async cancellation helpers built on `tokio_util::CancellationToken`.

## Key Types

| Type | Purpose |
|------|---------|
| `OrCancelExt` | Extension trait: `future.or_cancel(&token)` races a future against cancellation |
| `CancelErr::Cancelled` | Returned when the token fires before the future completes |
