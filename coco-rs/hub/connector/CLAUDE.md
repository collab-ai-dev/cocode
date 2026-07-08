# coco-hub-connector

Agent-side Event Hub connector boundary.

This crate owns CoreEvent/SessionEnvelope conversion and Hub v2 client-side
transport. It can open a WebSocket using the `coco-event-hub.v2` subprotocol,
send `announce` / `batch` frames, and validate `announce_ack` / `batch_ack`
responses. `HubConnectorWorker` adds the reusable background egress loop:
bounded producer channel, bounded pending event ring, durable-envelope
filtering, max-event and serialized-byte batching, jittered reconnect/backoff,
and shutdown flushing.
It must not depend on the hub server or web UI.

Still future work: durable `events_dropped` markers for backlog shedding. That
needs a sequence-safe marker policy across producer-channel overflow and the
AppServer-owned stamper.
