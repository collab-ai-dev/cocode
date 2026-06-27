# coco-utils-rustls-provider

Process-wide `rustls` crypto provider installer. Resolves ambiguity when both `ring` and `aws-lc-rs` features are enabled in the dep graph.

## Key Types
- `ensure_rustls_crypto_provider()` — idempotent install of `rustls::crypto::ring::default_provider()` via `Once`.
