# coco-exec-server

JSON-RPC exec-server for local and remote execution capabilities.

## Capabilities

- `stdio` and loopback-only `ws://IP:PORT` server transports.
- JSON-RPC initialize/initialized handshake and detached session resume.
- Process start/read/write/signal/terminate.
- Remote-safe filesystem operations through `PathUri`.
- Buffered and streamed HTTP requests in the selected environment.
- Local and remote `Environment` capability bundles for exec, filesystem, and
  HTTP.

## Explicit v1 Exclusions

- Noise relay is not implemented.
- Environment registry is not implemented.
- Platform sandbox helpers are not implemented.

Protocol fields for sandbox intent are retained only so callers receive an
explicit unsupported/invalid-input error. Never silently run a sandboxed process
or filesystem operation without sandbox enforcement.

## Key Types

- `ExecServerClient`, `RemoteExecServerConnectArgs`
- `ExecBackend`, `ExecProcess`, `StartedExecProcess`
- `ExecutorFileSystem`, `LocalFileSystem`, `LOCAL_FS`
- `Environment`, `EnvironmentManager`, `HttpClient`
- `ExecServerRuntimePaths`
- `run_main`, `DEFAULT_LISTEN_URL`
