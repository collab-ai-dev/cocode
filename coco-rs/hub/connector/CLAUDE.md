# coco-hub-connector

Agent-side Event Hub connector boundary.

This crate owns CoreEvent/SessionEnvelope conversion and Hub v2 client-side
transport. It can open a WebSocket using the `coco-event-hub.v2` subprotocol,
send `announce` / `batch` frames, and validate `announce_ack` / `batch_ack`
responses. `HubConnectorWorker` adds the reusable background egress loop:
bounded producer channel, bounded pending event ring, durable-envelope
filtering, max-event and serialized-byte batching, jittered reconnect/backoff,
shutdown flushing, and durable `events_dropped` markers when the producer
channel overflows.
It must not depend on the hub server or web UI.

Drop marker policy: `try_enqueue` records full-queue drops only for durable
`SessionEnvelope`s that already have a `session_seq`; ephemeral drops remain
live-only. The worker emits one `events_dropped` marker per session range before
the next higher same-session event or during shutdown flush, so Hub cursors can
advance across locally shed events.
