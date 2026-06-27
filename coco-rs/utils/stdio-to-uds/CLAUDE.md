# coco-stdio-to-uds

Connect to a Unix Domain Socket (or Windows equivalent via `uds_windows`) and bridge stdin / stdout bidirectionally. Used by the `coco` CLI for subprocess ↔ daemon IPC.

## Key API

- `run(socket_path: &Path) -> anyhow::Result<()>` — blocking; spawns a thread for socket→stdout, main thread copies stdin→socket, half-closes on EOF.
