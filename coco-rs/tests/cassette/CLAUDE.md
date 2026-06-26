# coco-cassette

VCR-style HTTP record/replay for deterministic provider tests. Test-support
crate (under `tests/`), ported from opencode's `@opencode-ai/http-recorder`.

## Why

Provider tests across `vercel-ai/*` use hand-written `wiremock` responses that
match requests on method+path only (0 body/header matchers workspace-wide), so
the outbound wire shape is unverified and the canned responses drift from real
provider output. A cassette records a real request/response pair once and
replays it deterministically — exercising the genuine `reqwest` + SSE codec
path against true provider bytes, no network, no live key.

## Key Types

- `Cassette` / `Interaction` / `RecordedRequest` / `RecordedResponse` — the JSON
  on-disk format (`version` + ordered `interactions`).
- `CassetteBuilder` — record half. Fed like a `WireTap`
  (`on_request` → `on_response_chunk*` → `finish_stream`, or `on_response_body`
  for non-streaming). Redacts via `coco-secret-redact` on arrival.
- `CassettePlayer` — replay half. A `wiremock` server serving recorded
  responses in order; matches each request body (canonicalized JSON) and
  `verify()`s every interaction was consumed.
- `CassetteError::UnsafeCassette` — `Cassette::save` re-scans the serialized
  file and refuses to write if a credential survived redaction.

## Design notes

- **No provider dependency.** This crate knows nothing about `vercel-ai` or
  `coco-inference`. The `WireTap` adapter that feeds `CassetteBuilder` lives
  with its caller (e.g. `tests/live`), the same layering as
  `app/query::wire_tap_adapter` → `coco-wire-dump`.
- **Replay over a real loopback server**, not an injected transport: coco-rs
  providers call `reqwest` directly (the `provider-utils` `Fetch` alias is
  unused), so the high-fidelity "run the real codec against recorded bytes"
  guarantee is achieved by pointing the provider's `base_url` at the player.
- **Record/replay policy** (CI-fails-on-missing, `COCO_TEST_RECORD=1` to
  re-record) is enforced by the *test* via `Cassette::load` returning
  `CassetteError::Missing`, not baked into the crate.
