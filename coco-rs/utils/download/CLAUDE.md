# coco-utils-download

Streaming file downloader. Tier-2 leaf utility (`thiserror`, no `coco-error`).

The one reusable "fetch a large artifact to disk, safely" primitive. Before it,
every HTTP fetch in the repo (`plugins/fetch`, retrieval providers) buffered the
whole body in memory with no checksum and no atomic install. Use this for any
sizeable download (Whisper weights, future model artifacts).

## API

- `download_file(req: DownloadRequest, progress: Option<Sender<DownloadProgress>>, cancel: CancellationToken) -> Result<(), DownloadError>`

## Guarantees

- **Streamed to disk**, never buffered whole — safe for multi-GB files.
- **Atomic install**: bytes land in a sibling `<dest>.part`, `fsync`ed, then
  `rename`d onto `dest` only after verification. A killed/corrupt download never
  leaves a half-file at the real path (the cache-poisoning failure mode of a
  naive downloader). The `*.part` is removed on any error.
- **Verified**: optional pinned `expected_sha256` (lowercase hex) and optional
  `expected_size` vs `Content-Length` pre-check.
- **Cancellable**: the `CancellationToken` aborts mid-stream (via `select!`) and
  cleans up.
- **Redirect-following**: reqwest default policy — HuggingFace `resolve/` → CDN
  redirects work.

## Notes

- `progress` is **lossy** (`try_send`, throttled to ~512 KiB strides): a slow
  consumer never backpressures the transfer.
- No total-transfer timeout (only a connect timeout); a legitimately slow large
  download must not be aborted — cancellation is the escape hatch.
- `reqwest` here carries the `stream` feature (cargo-unions with the workspace
  `rustls-tls` default) for `Response::bytes_stream()`.
