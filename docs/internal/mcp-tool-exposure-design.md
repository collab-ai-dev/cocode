# MCP Tool Exposure Architecture

Status: implemented.

This document defines how connected MCP server tools enter one provider
request. The public policy vocabulary and the internal placement vocabulary are
intentionally aligned:

| Config value | Internal placement | Meaning |
|---|---|---|
| `load` | `Loaded` | Put the full MCP tool schema in the provider tool list. |
| `defer` | `Deferred` or `Loaded` | Discover lazily, then promote or expand the schema. |
| `use_tool` | `UseTool` | Discover through `ToolSearch`; invoke through the stable `use_tool` carrier. |

There is no exposure value for disabling MCP. Full disablement belongs to MCP
activation (`Feature::Mcp`, server enable/deny policy, or tool filters), because
activation and schema-placement are different decisions.

## Configuration ownership

Exposure is a server-level policy. `mcp.tool_exposure` is the default for every
server; `mcp.server_tool_exposure` overrides that default by server name:

```jsonc
{
  "mcp": {
    "tool_exposure": "defer",
    "server_tool_exposure": {
      "memory": "load",
      "slack": "use_tool"
    }
  }
}
```

`COCO_MCP_TOOL_EXPOSURE` overrides only the global default. Accepted values are
exactly `load`, `defer`, and `use_tool`. Invalid input logs a warning and falls
back to `use_tool`, the valid mode that exposes the least schema.

Transport definitions do not own exposure. Keeping policy in `McpRuntimeConfig`
avoids duplicating it across stdio, HTTP, SSE, and in-process server variants.
The runtime resolves a tool's policy from its semantic `ToolId::Mcp { server,
tool }` identity.

## Placement rules

`ToolRegistry::materialize` applies ordinary feature/model/agent filters first.
Each surviving MCP tool is then assigned exactly one `ToolPlacement`:

```text
server exposure = load
  -> Loaded

server exposure = use_tool
  -> UseTool

server exposure = defer
  + tool alwaysLoad = true
      -> Loaded
  + model supports schema discovery/promotion
      + already discovered -> Loaded (still searchable/idempotent)
      + not discovered     -> Deferred
  + model has no supported promotion strategy
      -> UseTool
```

`alwaysLoad` is an MCP tool-level hint sourced from
`_meta["anthropic/alwaysLoad"]` or `_meta["alwaysLoad"]`. It is intentionally
consulted only under `defer`:

- `load` already loads every surviving tool;
- `defer` permits a high-frequency tool to opt into `Loaded`;
- explicit server policy `use_tool` wins over the hint.

This precedence prevents server-controlled metadata from overriding an
operator's schema-exposure decision.

## Model strategy matrix

| Exposure | Eager/no promotion | Client promotion | Anthropic reference | OpenAI native search |
|---|---|---|---|---|
| `load` | `Loaded` | `Loaded` | `Loaded` | `Loaded` |
| `defer` | `UseTool` | `Deferred` | `Deferred` | `Deferred` |
| `use_tool` | `UseTool` | `UseTool` | `UseTool` | `UseTool` |

For `defer`, falling back to `UseTool` is preferable to silently loading every
MCP schema on a model that cannot promote deferred schemas.

## Request materialization

`ToolUseContext` carries:

- `mcp_tool_exposure`: global default;
- `mcp_server_tool_exposure`: immutable `Arc<HashMap<server, exposure>>`;
- the current `ToolSearchStrategy`;
- the discovered canonical tool-name set.

`ToolMaterialization` is the immutable request snapshot. It contains
`Loaded`, `Deferred`, and `UseTool` targets plus canonical/wire-name indexes.
Provider projection, `ToolSearch`, call settlement, and stale-registration
checks all consume this same snapshot. They must not re-read the live registry
mid-request.

Canonical identity is `ToolId`; provider identity is `WireToolName`. MCP tools
are keyed by the qualified identity `mcp__<server>__<tool>`, so equal bare names
from different servers cannot promote or invoke each other.

## Transport closure

`ToolSearch` and `use_tool` are carriers, not MCP targets. They are registered
normally but materialized after target filtering:

- any `Deferred` or `UseTool` policy requires the stable `ToolSearch` carrier;
- any `UseTool` policy requires the stable `use_tool` carrier;
- a carrier never restores a target removed by feature, model, agent, server
  activation, or connection-state filtering;
- carrier presence follows configured policy rather than current candidate
  count, preserving a stable provider tool prefix as servers connect or
  disconnect.

The built-in `Feature::ToolSearch` controls lazy loading for built-in tools.
MCP `defer`/`use_tool` discovery remains available independently; otherwise
turning off built-in lazy loading would accidentally force eager MCP schemas.
`Feature::Mcp` remains the total MCP switch and also suppresses pending-server
metadata.

## `use_tool` execution

`use_tool` is a schema carrier only. Before validation and execution, the query
preparer resolves `{ name, arguments }` through the request snapshot:

1. `name` must resolve to a materialized `UseTool` target.
2. `Loaded` targets are rejected with guidance to call the tool by its own
   provider name.
3. `Deferred` targets are rejected with guidance to discover them first.
4. The live registration id must still match the request snapshot.
5. Validation, permission checks, hooks, concurrency classification, execution,
   and transcript metadata use the semantic MCP target.
6. Provider result pairing retains the `use_tool` carrier name and call id.

The carrier's own `execute` method fails closed. Reaching it means the preparer
was bypassed.

## Search projection

`ToolSearch` searches only `MaterializedTool::discoverable` entries. Search
output is bounded per description, per schema, in aggregate, and for pending
server metadata. Oversized schemas are omitted rather than truncated.

If any configured server needs `UseTool`, search results use bounded client JSON
instead of provider-side schema expansion: provider expansion implies the
returned schema can be invoked by its own name, which is false for a `UseTool`
target. Each `UseTool` result includes the exact carrier invocation form.

Client promotion records canonical names only for `Deferred` targets. `UseTool`
targets remain `UseTool` after discovery and therefore do not mutate the
provider tool list.

## Inheritance

Child agents, skills, workflows, forks, and in-process teammates inherit both
the default and per-server overrides. `McpToolExposure::restrict` orders policy
from most to least schema exposure:

```text
load > defer > use_tool
```

`restrict_server_overrides` evaluates the union of parent and requested server
keys, using each side's own default for missing keys. The child receives the
more restrictive value per server and cannot widen any parent policy.

## Architectural invariants

1. Activation decides whether a tool exists; exposure decides where a surviving
   tool is placed.
2. Every materialized tool has exactly one placement.
3. `load` always produces `Loaded` for surviving MCP targets.
4. `alwaysLoad` affects only `defer`.
5. Explicit `use_tool` cannot be widened by server metadata or a child model.
6. `UseTool` targets never enter the provider tool list under their own name.
7. Provider projection and execution settlement use one immutable snapshot.
8. Qualified semantic identity prevents cross-server discovery collisions.
9. Transport carriers preserve reachability without widening target filters.
10. MCP disablement never relies on an exposure enum value.

## Verification

Coverage is split by responsibility:

- `common/types/src/mcp_exposure.test.rs`: parsing and inheritance restriction;
- `common/config/src/sections.test.rs`: global/default and server override
  resolution;
- `core/tool-runtime/src/registry.test.rs`: placement matrix, mixed servers,
  `alwaysLoad`, identity, and stale snapshots;
- `core/tools/src/tools/tool_search.test.rs`: bounded search and `UseTool`
  projection;
- `app/query/src/tool_runner.test.rs`: carrier unwrap and semantic settlement;
- `app/query/tests/mcp_tool_exposure.rs`: provider request matrix, mixed-server
  request surface, and end-to-end `use_tool` execution.
