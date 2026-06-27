# coco-hub-connector

Agent-side Event Hub connector boundary.

This crate owns future CoreEvent aggregation, bounded ring buffering, and
WebSocket sending. It must not depend on the hub server or web UI.
