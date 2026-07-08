# coco-hub-protocol

Wire-level Event Hub types only.

This crate must stay usable by both the agent-side connector and the hub
server. Do not add Axum, SQLite, session storage, config resolution, or UI
dependencies here.

The wire contract is `coco-event-hub.v2`: announce frames carry the live
session set, cursor acknowledgements are per-session maps, and event envelopes
use typed `SessionId`, optional typed `AgentId`, and per-session
`session_seq`. Update the snapshot tests in `tests/wire_format.rs` whenever
this JSON shape changes intentionally.
