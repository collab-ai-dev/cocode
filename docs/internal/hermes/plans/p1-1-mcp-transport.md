# P1-1 — MCP Transport: `tools/list_changed` Refresh + Keepalive Ping

Status: not started · Size: M · Owner crates: `coco-rmcp-client`,
`coco-mcp`

## Problem (both halves verified ABSENT)

- **list_changed**: coco's only `ClientHandler` is
  `LoggingClientHandler`; its `on_tool_list_changed`
  (`services/rmcp-client/src/logging_client_handler.rs:96-98`) just logs
  `"MCP server tool list changed"`. Tool lists refresh only on reconnect
  (`services/mcp/src/discovery.rs:217`). A server that adds/removes
  tools mid-session silently drifts.
- **keepalive**: no periodic ping anywhere in `services/mcp` /
  `services/rmcp-client`; HTTP/SSE session death is handled reactively
  via 404 → `SessionExpired404` reconnect
  (`services/rmcp-client/src/rmcp_client.rs:101-104`). Short-TTL
  Streamable-HTTP servers expire silently mid-conversation and the
  first tool call after idle pays a failed round-trip.

## Hermes evidence (hermes-agent @ `a7f65e3bc`)

Releases v2026.6.19 #49221/#49208, v2026.3.30 #3812, v2026.5.7 MCP
batch. All in `tools/mcp_tool.py`:

- **Constants** (:339-348): `_DEFAULT_KEEPALIVE_INTERVAL = 180` s,
  `_MIN_KEEPALIVE_INTERVAL = 5` (clamp floor); comment cites
  Streamable-HTTP session TTLs as low as ~15 s.
- **Keepalive loop** — `_wait_for_lifecycle_event` (:1864-1957); cadence
  `max(_MIN, config.get("keepalive_interval", 180))` (:1899-1902); on
  tick: `await self._keepalive_probe()`, failure → `logger.warning(…) ;
  self._reconnect_event.set()` (:1939-1948).
- **Probe** — `_keepalive_probe` (:1824-1862):
  `asyncio.wait_for(session.send_ping(), timeout=30.0)` (:1842);
  JSON-RPC `-32601` (method not found) latches `_ping_unsupported` and
  falls back to `list_tools` (:1862); tools-incapable servers guarded by
  `_advertises_tools` (:1595-1613).
- **list_changed** — `_make_message_handler` (:1714-1755):
  `case ToolListChangedNotification():` (:1728) →
  `_schedule_tools_refresh()` (:1742, deliberately async so the stdio
  JSON-RPC stream never wedges). `_refresh_tools` (:1757-1801)
  re-fetches `list_tools()` under a lock, deregisters only stale names,
  re-registers **in place** ("Avoid nuke-and-repave… live agent turns
  may already have tool-call IDs pointing at existing handler
  functions", :1782-1788).
- **Cache-safe exposure** — `refresh_agent_mcp_tools` (:5231+): "the
  late-binding and between-turns paths only rebuild at a turn boundary,
  before that turn's `tools=` prefix is assembled" (:5267-5270);
  stale-write rejection via a registry generation snapshot (:5288-5294).

## Design

### Keepalive (transport layer, `coco-rmcp-client`)

1. Per-server config on `McpServerConfig`:
   `keepalive_interval_secs: i64`, `#[serde(default)]` → default 180,
   clamped to ≥ 5; `0` disables. Applies to HTTP/SSE transports;
   stdio default-off (process liveness is already observable).
2. A tokio task per connected HTTP/SSE client: every interval with no
   other traffic, send MCP `ping` with a 30 s timeout
   (`services/mcp-types` already defines ping). On JSON-RPC
   "method not found", latch `ping_unsupported` and fall back to
   `tools/list` *only if* the server advertises the tools capability
   (capability check parity with hermes's `_advertises_tools`).
3. Probe failure → trigger the existing reconnect path (same seam as
   `SessionExpired404`), log at `warn` with the standard otel field
   names.
4. Task lifecycle: spawned on connect, aborted on disconnect/drop
   (`CancellationToken`, matching async conventions).

### list_changed (service layer, `coco-mcp`)

1. Extend the client handler: `on_tool_list_changed` sets a per-server
   dirty flag / sends on a watch channel — never does I/O inline
   (hermes's "don't wedge the stream" rule).
2. `services/mcp` discovery consumes the flag **between turns**: re-list
   tools, diff against the registry, deregister stale names, register
   new ones in place (no nuke-and-repave — in-flight tool-call IDs must
   stay valid).
3. Model-facing exposure needs no new work: the existing deferred-tools
   delta / agent-listing system-reminder generators already advertise
   pool changes cache-safely at turn boundaries. Verify the refreshed
   registry feeds them (it does for the reconnect path today — same
   entry point).
4. Guard against stale writes with a discovery generation counter
   (mirror hermes's `registry._generation` snapshot).

## Implementation steps

1. Config + clamp + `EnvKey` (if an env override is wanted:
   `COCO_MCP_KEEPALIVE_INTERVAL` — optional, config-first).
2. Keepalive task in `rmcp_client` (HTTP/SSE only) + reconnect trigger.
3. Handler → dirty flag → between-turns refresh in `discovery.rs` with
   in-place diff.
4. `just test-crate coco-rmcp-client` + `coco-mcp`; integration test
   with a mock server (wiremock is already a workspace dep) that flips
   its tool list and expires sessions.

## Tests

- Ping unsupported (-32601) → falls back to `tools/list`; tools-incapable
  server → keepalive disabled, no error spam.
- Ping timeout → reconnect triggered exactly once (no storm).
- list_changed mid-turn → refresh deferred to the turn boundary; new
  tool callable next turn; removed tool's in-flight call unaffected.
- Generation guard: two overlapping refreshes → last-writer-wins without
  duplicate registration.

## Risks / non-goals

- Keepalive traffic against metered servers — default 180 s is
  negligible; per-server disable exists.
- Non-goals: hermes's curated MCP catalog/picker; mTLS client certs;
  OAuth flows (already covered elsewhere in `services/mcp`).
