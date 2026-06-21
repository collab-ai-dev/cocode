# coco-bridge

Standalone bridge for SDK / IDE / remote-control consumers. Covers two distinct
subsystems:

1. **REPL bridge** — NDJSON over stdio for programmatic SDK use (Python, CCR).
2. **IDE bridge** — permission relay + message pump for VS Code / JetBrains
   (detected via MCP lockfiles in TS; coco-rs ships the skeleton for parity).

## Key Types

| Module | Types |
|--------|-------|
| `protocol` | `BridgeInMessage` (Submit/Approve/Deny/Cancel/Ping), `BridgeOutMessage` (Text/ToolUse/ToolResult/PermissionRequest/Status/Error/Pong), `BridgeTransport` (WebSocket/Sse/Ndjson) |
| `repl` | `ReplBridge`, `BridgeState`, `ReplInMessage`, `ReplOutMessage`, `ControlRequest`, `ControlRequestHandler`, `RejectingControlHandler`, `dispatch_control`, `ControlError` |
| `server` | `BridgeServer` — NDJSON / WS listener skeleton |
| `permission_callbacks` | `BridgePermissionRequest`, `BridgePermissionResponse`, `BridgeDecision`, `BridgeRiskLevel` — IDE-side permission arbitration |
| `jwt_utils` | `Claims`, `JwtError` — session-ingress JWT |
| `work_secret` | `derive_secret_from_material`, `generate_fresh_secret`, `account_name_for_workspace` — keyring-backed per-workspace secret |
| `trusted_device` | `TrustedDevice`, `TrustedDeviceStore` — long-lived device trust |
| `ide` | IDE-specific adapters (detection, outbound RPC: `openDiff`, `openFile`, `getDiagnostics`) |

## Architecture

```
SDK / IDE ─► BridgeInMessage ─► dispatch_control / ControlRequestHandler
                                       │
                                       ├─ Submit → QueryEngine (via handle)
                                       ├─ Approve/Deny → PermissionCallbacks
                                       └─ Cancel → CancellationToken
                                       
QueryEngine ─► CoreEvent ─► BridgeOutMessage ─► transport (NDJSON/WS/SSE)
```

The IDE bridge is NOT a direct IDE↔agent WebSocket relay. IDEs connect as MCP
servers (not through this crate). This crate owns the REPL bridge (stdio NDJSON)
and permission-relay primitives. The CCR daemon spawn modes (SingleSession /
Worktree / SameDir) are documented in `docs/coco-rs/crate-coco-bridge.md` and
wired from `coco-cli` (`Commands::RemoteControl`).

## Deliberately Not Implemented

| API | Reason |
|---|---|
| `updateBridgeSessionTitle(sessionId, title)` | Anthropic-cloud-only: PATCHes `BASE_API_URL/v1/sessions/{cse_*}` with Claude AI OAuth + `anthropic-beta: ccr-byoc-2025-07-29` + `x-organization-uuid`. The `cse_*` session ID namespace, claude.ai OAuth flow, and CCR-BYOC beta header all belong to claude.ai/code's CCR backend. Parallel to the `/ultraplan`, `/ultrareview`, `/passes` skips in `commands/CLAUDE.md` — that backend is not a coco-rs target. The `/rename` runner intentionally does not call any equivalent. |
| `fetchSession`, `archiveSession`, `createBridgeSession` | Same CCR-backend-only rationale. The `coco-bridge::repl` types model the local REPL transport, not remote session lifecycle. |

If a coco-rs deployment ever targets the CCR backend, the right architectural answer is a **separate** `coco-cloud` (or `coco-ccr`) crate that owns the Anthropic OAuth + cse-id translation; do NOT add provider-specific cloud calls to this crate or to `coco_cli::session_rename`.
