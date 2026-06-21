# coco-hub-protocol

Wire-level Event Hub types only.

This crate must stay usable by both the agent-side connector and the hub
server. Do not add Axum, SQLite, session storage, config resolution, or UI
dependencies here.
