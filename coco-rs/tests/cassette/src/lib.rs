//! `coco-cassette` — VCR-style HTTP record/replay for deterministic provider
//! tests, ported from opencode's `@opencode-ai/http-recorder`.
//!
//! The problem it solves: coco-rs provider tests use hand-written `wiremock`
//! responses that match requests on method+path only, so the real outbound
//! wire shape is unverified and the responses drift from what providers
//! actually send. A cassette records a *real* request/response pair once, then
//! replays it deterministically forever — exercising the genuine reqwest + SSE
//! codec path against true provider bytes, with no network and no live key.
//!
//! Two halves, mirroring opencode:
//! - **Record** ([`CassetteBuilder`]): fed the same bytes a `WireTap` observes
//!   (request body, streamed response chunks). Redacts via `coco-secret-redact`
//!   as bytes arrive; [`Cassette::save`] re-scans the whole file and *refuses
//!   to write* if any credential survives (opencode's `UnsafeCassetteError`).
//! - **Replay** ([`CassettePlayer`]): a `wiremock` server that serves recorded
//!   responses in order, matches each incoming request body against the
//!   recorded one (canonicalized JSON, key-order-independent), and
//!   [`CassettePlayer::verify`]s that every interaction was consumed — the
//!   "Unused recorded interactions" guard.
//!
//! coco-rs already ships the recording substrate (`services/wire-dump`'s
//! `WireTap` seam) and the redaction (`utils/secret-redact`); this crate is the
//! missing replay consumer + cassette format. The `WireTap` adapter that feeds
//! [`CassetteBuilder`] lives with its caller (e.g. `tests/live`), keeping this
//! crate free of any provider dependency — the same layering as
//! `app/query::wire_tap_adapter`.

pub mod cassette;
pub mod player;

pub use cassette::CASSETTE_VERSION;
pub use cassette::Cassette;
pub use cassette::CassetteBuilder;
pub use cassette::CassetteError;
pub use cassette::Interaction;
pub use cassette::RecordedRequest;
pub use cassette::RecordedResponse;
pub use player::CassettePlayer;
