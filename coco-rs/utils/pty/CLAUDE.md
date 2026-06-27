# coco-utils-pty

Spawn and manage child processes over either a PTY (interactive) or pipes (non-interactive), with broadcast output channels.

## Key Types
- `spawn_pty_process` / `spawn_pipe_process` / `spawn_pipe_process_no_stdin` — spawn helpers
- `ProcessHandle` (alias `ExecCommandSession`) — control handle (stdin, resize, kill)
- `SpawnedProcess` (alias `SpawnedPty`) — handle + stdout/stderr/exit receivers
- `TerminalSize`, `combine_output_receivers`, `conpty_supported`, `DEFAULT_OUTPUT_BYTES_CAP`
- Windows only: `RawConPty` via `win::conpty`
