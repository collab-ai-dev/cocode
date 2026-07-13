# coco-sdk-server

SDK JSON-RPC/NDJSON adapter for `coco` SDK mode. This crate owns connection
and wire concerns only: stdio/sidecar transports, ordered outbound writing,
callback reply correlation, slow-consumer behavior, and AppServer JSON-RPC
bridge wiring.

Session/runtime behavior belongs in `coco-agent-host`. If logic would still
matter after deleting the SDK transport, keep it out of this crate and expose
it through `app_server_host`, `session_runtime`, or another protocol-neutral
host module.

## Key Types

| Type | Purpose |
|------|---------|
| `run_sdk_mode` | SDK surface adapter startup over an already prepared `coco-agent-host::remote_host::PreparedRemoteHost` plus `SdkSidecarConfig`; owns stdio, sidecars, dispatch, and shutdown sequencing. |
| `SdkServer` | SDK connection adapter over an injected `SdkTransport` and `RemoteAppServerBridgeHost` capability handle. |
| `SdkServer::run_app_server_connection` | Runs the SDK transport through the AppServer JSON-RPC adapter and shared host handler. |
| `SdkTransport` | Frame-level transport trait for SDK JSON-RPC traffic. |
| `StdioTransport` | stdin/stdout NDJSON transport used by `coco sdk`. |
| `SdkSidecarConfig` | SDK endpoint configuration already mapped by the process composer; this crate does not read `RuntimeConfig`. |
| `SdkSidecarListeners` | Optional Unix/WebSocket/named-pipe SDK AppServer sidecar listeners bound in SDK mode. |
| `InMemoryTransport` | Test transport for SDK bridge and live harness tests. |
| `RemoteAppServerBridgeError` | Adapter-level transport/bridge error type from SDK transport into the remote AppServer host. |

## Boundaries

- Keep DTO parsing, SDK transport, callback correlation, and outbound ordering
  here.
- Keep runtime construction, session selection, history, MCP ownership, reload,
  file history, permissions, and turn accounting in `coco-agent-host`.
- SDK startup must not rebuild `SessionRuntimeFactory` or direct MCP/history
  owners; `coco-cli` prepares `coco_agent_host::remote_host::PreparedRemoteHost`, and
  this crate wraps it with SDK transports.
- Sidecar listeners bind Unix/WebSocket/named-pipe transports here, but host
  handler and outbound-forwarder bindings come from `PreparedRemoteHost`; do not
  reconstruct `AppServerHostHandler` wiring in this crate.
- The stdio/AppServer bridge also obtains its handler/outbound binding from
  `coco_agent_host::remote_host`; this crate may render and write outbound frames,
  but must not construct host handlers directly.
- Use remote host aliases from `coco_agent_host::remote_host` for AppServer
  connection types; do not import `app_session::AppSessionHandle` here.
- `coco-cli` chooses startup mode and maps Clap into
  `coco_agent_host::remote_host::RemoteHostOptions` plus `SdkSidecarConfig`; SDK
  surface startup and request handling stay in this crate.
