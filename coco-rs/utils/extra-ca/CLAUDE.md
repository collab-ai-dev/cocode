# coco-utils-extra-ca

Process-wide, opt-in extra TLS roots for coco's reqwest clients. Tier-2 leaf
utility; no dependency on config or application crates.

## Contract

- `COCO_EXTRA_CA_BUNDLE` names a PEM bundle whose certificates are added to,
  never substituted for, reqwest's built-in webpki roots.
- Unset or empty means no filesystem access and an unchanged builder.
- The bundle is read once, through a 1 MiB cap, and each certificate is
  validated by rustls before it is cached as DER.
- An unreadable, oversized, malformed, or empty bundle emits diagnostics and
  leaves the normal root set intact. A bad optional enterprise override must
  not make every network feature fail to initialize.
- Production async reqwest clients should start from `client_builder()` or
  `client()`; blocking clients use `with_extra_root_certificates_blocking()`.
  Caller-owned timeout, proxy, redirect, and header policy remains at the
  caller.

The DER cache is intentionally version-neutral so another TLS transport can
consume `extra_root_ders()` without depending on reqwest internals.
