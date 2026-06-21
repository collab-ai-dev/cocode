# coco-sdk Port Status

This directory was ported from `cocode-sdk/` as part of Phase 2.G of the
event system refactor. All mechanical renames have been applied:

| Old | New |
|-----|-----|
| `cocode_sdk` (Python package) | `coco_sdk` |
| `cocode-sdk` (pyproject name) | `coco-sdk` |
| `CocodeClient` / `CocodeSDKError` etc. | `CocoClient` / `CocoSDKError` |
| `cocode` (binary name) | `coco` |
| `COCODE_PATH` / `COCODE_CLI_PATH` / `COCODE_ENTRYPOINT` env vars | `COCO_PATH` / `COCO_CLI_PATH` / `COCO_ENTRYPOINT` |
| `cocode_app_server_protocol.schemas.json` | `coco_app_server_protocol.schemas.json` |
| `~/.cocode/` config path | `~/.coco/` |
| `cocode_vscode` / `cocode_web` client name examples | `coco_vscode` / `coco_web` |

## Regeneration Pipeline (Phase 2.A.5 ✅ done)

The full Rust → JSON Schema → Python pipeline is working:

```bash
# One-shot regeneration of everything:
./coco-sdk/scripts/generate_all.sh

# Or step by step:
./coco-sdk/scripts/generate_schemas.sh   # coco-rs types → schemas/json/*.json
./coco-sdk/scripts/generate_python.sh    # schemas → generated/protocol.py + stubs + __init__.py
```

### Pipeline internals

1. **`generate_schemas.sh`** runs:
   ```
   cargo run -p coco-types --features schema --example export_schema
   ```
   which invokes `coco-rs/common/types/examples/export_schema.rs`. It writes
   directly to `coco-sdk/schemas/json/`:
   - 11 individual schema files (server_notification, notification_method,
     agent_stream_event, tui_only_event, client_request, client_request_method,
     server_request, server_request_method, jsonrpc_message, thread_item,
     session_start_request)
   - 1 bundled file `coco_app_server_protocol.schemas.json` containing every
     type reachable from a small set of entry points (the union envelopes plus
     ProviderApi / ModelRole / ModelSpec / Capability / WireApi / ApplyPatchToolType).
     New wire types appear automatically as long as they're reachable via `$ref`
     from any entry point.

2. **`generate_python.sh`** runs in three phases:
   - `postprocess_python.py` reads the bundle + individual schemas and emits
     Pydantic models with tagged-union accessors (`as_session_started()`,
     `as_turn_completed()`, etc.). Validation warnings for accessor/method
     mismatches are non-fatal.
   - `append_stubs.py` scans `src/coco_sdk/**/*.py` + `tests/**/*.py` for
     imports from `coco_sdk.generated.protocol` that reference classes the
     generator did NOT emit. For each missing name, appends a loose
     `BaseModel` subclass with `model_config = {"extra": "allow"}`. This
     bridges legacy imports from cocode-sdk until the coco-rs schema grows.
   - `regen_init.py` rewrites `__init__.py` with the current set of protocol
     class names plus the 17 static SDK exports (CocoClient, query, @tool,
     @hook, errors, etc.).

### Current generator output

- **149 protocol types** emitted from the coco-rs schema bundle
- **17 compatibility stubs** auto-appended for legacy cocode-sdk names:
  AgentDefinitionConfig, AgentMessageDeltaParams, ApprovalDecision,
  CommandExecutionItem, ConfigReadRequest, HookBehavior, HookCallbackConfig,
  HookCallbackOutput, ItemStatus, KeepAliveRequest, McpServerConfig,
  PostToolUseHookInput, PreToolUseHookInput, SessionEndedReason,
  SessionListRequest, TurnInterruptRequest, Usage
- **20 tests passing, 3 skipped** (test_client.py, test_protocol_types.py,
  test_protocol_sync.py — they need the stubbed types to be real models with
  specific fields; skipped pending full schema emission).

### Stub shrinkage plan

As Phase 2 adds emit points for more param types, each one should be
exported in `examples/export_schema.rs` so it flows into the bundle, which
removes it from the stub list automatically. The test files that are
currently skipped should be re-enabled as their stub types become real.

## Layout

```
coco-sdk/
├── python/
│   ├── src/coco_sdk/           # SDK source (renamed from cocode_sdk)
│   │   ├── __init__.py          # Public API exports — NEEDS trimming
│   │   ├── client.py            # CocoClient multi-turn session
│   │   ├── query.py             # one-shot query()
│   │   ├── tools.py             # @tool() decorator for SDK-hosted MCP tools
│   │   ├── decorators.py        # @hook() decorator
│   │   ├── structured.py        # TypedClient[T] for Pydantic structured output
│   │   ├── errors.py            # CocoSDKError, CLINotFoundError, etc.
│   │   ├── _internal/transport/ # StdioTransport + ReconnectingTransport
│   │   └── generated/protocol.py # NEEDS regeneration from coco-rs schemas
│   ├── examples/                # Ported examples (binary name updated)
│   ├── tests/                   # Ported tests (MockTransport, no subprocess)
│   └── pyproject.toml           # Hatch-based build
├── schemas/json/                # Schema artifacts (need regeneration)
└── scripts/                     # generate_schemas.sh + postprocess_python.py
```

## Dependencies on coco-rs

The Python SDK talks to `coco` via:
- Subprocess: `coco` binary with `--sdk-mode` flag (not yet implemented —
  Phase 2.B.3 StdioTransport + Phase 2.C SdkServer dispatch).
- Wire protocol: NDJSON messages matching the `JsonRpcMessage` / `ClientRequest`
  / `ServerRequest` / `ServerNotification` shapes defined in
  `coco-rs/common/types/src/{jsonrpc,client_request,server_request,event}.rs`.

Until Phase 2.B and 2.C land, the SDK client side is complete but has no
counterpart to talk to.
