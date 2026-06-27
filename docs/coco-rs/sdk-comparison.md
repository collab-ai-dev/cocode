# SDK Design Comparison: cocode-rs vs Claude Code TS

> Comparison dimensions: Schema Generation, Event Consumption, Bidirectional Control Protocol, Transport, SDK Client
> Conclusion: cocode-rs is architecturally superior to TS across the board; needs to backfill TS runtime capabilities and consumer convenience

---

## 1. Schema Generation Pipeline

### 1.1 cocode-rs: Rust → JSON Schema → Multi-Language

```
Rust types (serde + schemars::JsonSchema derive)
  ↓
export.rs binary (cargo run --bin export-app-server-schema)
  ↓
schema_for!() → JSON Schema files (schema/json/*.json)
  ↓ individual:
    server_notification.json (59KB)
    client_request.json (40KB)
    server_request.json (7KB)
    thread_item.json (10KB)
    session_start_request.json (18KB)
    usage.json (1KB)
  ↓ bundled:
    cocode_app_server_protocol.schemas.json (all 80+ types)
  ↓
generate_schemas.sh → copy to cocode-sdk/schemas/json/
  ↓
postprocess_python.py (40KB custom generator)
  ↓
Python Pydantic models (protocol.py)
  - Tagged union support for enum variants
  - Accessor methods for method names
  - Proper type hints
```

**Source of truth**: Rust `struct`/`enum` with `#[derive(Serialize, Deserialize, JsonSchema)]`

**Types exported** (80+ in bundled schema):
- 6 core protocol types: ServerNotification, ClientRequest, ServerRequest, ThreadItem, Usage, SessionStartRequestParams
- 14 config types: McpServerConfig, AgentDefinitionConfig, SandboxConfig, ThinkingConfig, etc.
- 5 JSON-RPC types: JsonRpcRequest, JsonRpcNotification, JsonRpcResponse, JsonRpcError, RequestId
- 16 hook input types: PreToolUse, PostToolUse, Stop, SubagentStart, etc.
- 11 request param types: TurnStartRequestParams, ApprovalResolveRequestParams, etc.
- 15 notification param types: PlanModeChangedParams, CostWarningParams, etc.
- 13 auxiliary types: ErrorInfo, ErrorCategory, ItemStatus, ApprovalDecision, etc.

### 1.2 TS: Zod → TypeScript-only

```
Zod schemas (coreSchemas.ts, ~56K lines)
  ↓
bun scripts/generate-sdk-types.ts
  ↓
coreTypes.generated.js (TypeScript types)
  ↓
Re-exported via coreTypes.ts + agentSdkTypes.ts
```

**Source of truth**: Zod schema objects in `coreSchemas.ts`

**Types exported** (via agentSdkTypes.ts):
- Core types: SDKMessage (24 variants union), SDKAssistantMessage, SDKUserMessage, SDKResultMessage, etc.
- Control types: SDKControlRequest, SDKControlResponse (marked @alpha)
- Runtime types: callbacks, interfaces (non-serializable)
- Settings types: from settingsTypes.generated.js
- Tool types: marked @internal

### 1.3 Comparison

| Dimension | cocode-rs | TS | Assessment |
|-----------|-----------|-----|------------|
| **Source of truth** | Rust types (strong typing + derive) | Zod schemas (runtime + compile-time) | cocode-rs: compile-time guarantees; TS: runtime validation |
| **Schema format** | JSON Schema (ISO standard) | Zod (JS ecosystem only) | **cocode-rs wins**: JSON Schema is a language-agnostic standard |
| **Multi-language support** | Yes (Python implemented, TS/Go/Swift extensible) | No (TypeScript only) | **cocode-rs wins**: define once, generate for multiple languages |
| **Runtime validation** | serde deserialization (zero runtime overhead) | z.parse() (runtime overhead) | cocode-rs: compile-time type safety; TS: flexible runtime validation |
| **Schema export completeness** | 80+ types, bundled schema | ~40 types in SDKMessage union | **cocode-rs wins**: more complete coverage |
| **Generator complexity** | postprocess_python.py (40KB, custom) | generate-sdk-types.ts (lightweight) | Trade-offs on both sides |
| **Schema versioning** | JSON Schema files can be diffed/version-controlled | Zod inline, no standalone schema files | **cocode-rs wins**: schemas can be distributed independently |
| **Backward compatibility** | JSON Schema field `#[serde(default)]` | Zod `.optional()` | Equivalent |

### 1.4 Key Design Advantages of cocode-rs

1. **Language-agnostic Schema**: JSON Schema can be consumed by any language — Python (datamodel-code-generator), Go (go-jsonschema), Swift (Sourcery), C# (NJsonSchema)
2. **Compile-time consistency**: Rust type system guarantees schema and implementation stay in sync; TS Zod requires `z.infer<typeof schema>` runtime inference
3. **Incremental generation**: Adding new types only requires `#[derive(JsonSchema)]`, export.rs automatically includes them
4. **Schema distribution**: JSON files can be published independently as an SDK contract, without requiring Rust compilation

---

## 2. Event Consumption Architecture

### 2.1 cocode-rs: 3-Layer CoreEvent + StreamAccumulator

```
Agent Loop (core/loop)
  │
  ├─ emit(CoreEvent::Protocol(ServerNotification))  → all consumers
  ├─ emit(CoreEvent::Stream(StreamEvent))            → requires StreamAccumulator for state accumulation
  └─ emit(CoreEvent::Tui(TuiEvent))                  → TUI exclusive, SDK discards
      │
      ↓ mpsc::channel<CoreEvent>
      │
  ┌───┴──────────────────────────────────────────────┐
  │              Turn Runner / Processor               │
  │                                                    │
  │  CoreEvent::Protocol(n) → outbound.send(n)        │
  │  CoreEvent::Stream(e)   → accumulator.process(e)  │
  │                           → Vec<ServerNotification>│
  │                           → outbound.send(each)    │
  │  CoreEvent::Tui(t)       → match t {              │
  │      ApprovalRequired → ServerRequest::AskForApproval
  │      QuestionAsked    → ServerRequest::RequestUserInput
  │      _                → drop (TUI-only)           │
  │  }                                                 │
  └─────────────┬──────────────────────────────────────┘
                │ OutboundMessage
                ↓
           ┌─────────┐
           │Transport │
           ├─────────┤
           │ stdio    │ → NDJSON {"method":"...", "params":{...}}
           │ ws       │ → WebSocket JSON frame
           │ channel  │ → in-process mpsc (TUI)
           └─────────┘
```

**StreamAccumulator state machine** (407 LOC):
- Maintains: `text_buffer`, `thinking_buffer`, `active_items: HashMap<call_id, ThreadItem>`
- Input: 7 StreamEvent variants
- Output: ItemStarted/Updated/Completed + AgentMessageDelta/ReasoningDelta

```
ThinkingDelta* → ItemStarted(Reasoning) + ReasoningDelta* + ItemCompleted(Reasoning)
TextDelta*     → ItemStarted(AgentMessage) + AgentMessageDelta* + ItemCompleted(AgentMessage)
ToolUseQueued  → ItemStarted(tool-specific ThreadItem)
ToolUseStarted → ItemUpdated
ToolUseCompleted → ItemCompleted
```

**ThreadItem tool mapping** (semantic):
| Tool | ThreadItemDetails |
|------|-------------------|
| Bash | CommandExecution { command, output, exit_code, status } |
| Edit/Write | FileChange { changes: [{path, kind}], status } |
| WebSearch | WebSearch { query, status } |
| mcp__* | McpToolCall { server, tool, arguments, result?, error?, status } |
| Task/Agent | Subagent { agent_id, agent_type, description, is_background, result?, status } |
| Others | ToolCall { tool, input, output?, is_error, status } |

### 2.2 TS: Flat SDKMessage + normalizeMessage() + sdkEventQueue

```
QueryEngine.submitMessage()
  │
  ├─ yields Message (internal type)
  │    ↓ normalizeMessage() (sync generator, queryHelpers.ts:102-222)
  │    ↓ SDKMessage (24-variant union, type+subtype discriminant)
  │
  ├─ side-channel: sdkEventQueue.enqueueSdkEvent() (headless only)
  │    (TaskStarted, TaskProgress, TaskNotification, SessionStateChanged)
  │    ↓ drainSdkEvents() at 4 specific flush points in print.ts
  │
  └─ output.enqueue(message) → Stream<StdoutMessage> FIFO
      │
      ↓ for await (const msg of runHeadlessStreaming())
      │
      ↓ structuredIO.write(JSON.stringify(msg))
      │
      ↓ stdout (NDJSON only)
```

**normalizeMessage()** (sync generator):
- `assistant` → filter empty → SDKAssistantMessage
- `progress` → agent_progress/skill_progress → SDKAssistantMessage/SDKUserMessage (with parentToolUseID)
- `progress` → bash_progress/powershell_progress → SDKToolProgressMessage (30s throttle, Remote only)
- `user` → SDKUserMessage

**sdkEventQueue** (headless-only side channel):
- 4 event types: TaskStarted, TaskProgress, TaskNotification, SessionStateChanged
- MAX_QUEUE_SIZE = 1000, LRU eviction
- Only activated when `getIsNonInteractiveSession()` returns true
- Drained at 4 locations in print.ts: before result, after non-result, finally block, after task completion

### 2.3 Comparison

| Dimension | cocode-rs | TS | Assessment |
|-----------|-----------|-----|------------|
| **Event layering** | 3 layers (Protocol + Stream + Tui) | 1 layer (SDKMessage) + side channel | **cocode-rs wins**: explicit layering, separation of concerns |
| **Stream accumulation** | StreamAccumulator (explicit state machine, 407 LOC) | normalizeMessage (sync generator, 120 LOC) | **cocode-rs wins**: testable, observable state |
| **ThreadItem semantics** | 9 typed variants (semantic tool results) | Raw content blocks (consumer must parse) | **cocode-rs wins**: zero parsing burden for consumers |
| **TUI isolation** | TuiEvent layer automatically isolates | SDKMessage mixes in UI events | **cocode-rs wins**: no noise for SDK consumers |
| **Total event count** | 43 Protocol + 7 Stream + 20 Tui = 70 | 24 SDKMessage + 4 sdkEvent = 28 | cocode-rs has finer granularity |
| **Side channel** | Not needed (CoreEvent is already layered) | sdkEventQueue (headless-only, 4 types) | **cocode-rs wins**: no ad-hoc patches |
| **UI reverse parsing** | Not needed (TuiEvent consumed directly) | convertSDKMessage() (deserialize back for UI) | **cocode-rs wins**: no reverse conversion |
| **Throttle/Filter** | At emission side (sender) | At consumer side (normalizeMessage 30s throttle) | Trade-offs on both sides |

### 2.4 TS sdkEventQueue Design Flaws

TS needs `sdkEventQueue` as a headless-only side channel because:
1. The flat SDKMessage model cannot distinguish "SDK-needed" vs "UI-needed" events
2. TaskStarted/Progress/SessionStateChanged cannot be produced through the normalizeMessage() path (they are not Message types)
3. Can only be manually drained at 4 specific locations in print.ts, making timing fragile

cocode-rs does not need this pattern because:
- `CoreEvent::Protocol(ServerNotification::TaskStarted)` goes through the unified channel directly
- `CoreEvent::Protocol(ServerNotification::SessionStateChanged)` likewise
- Turn runner uniformly handles all CoreEvent variants, eliminating risk of missed events

---

## 3. Bidirectional Control Protocol

### 3.1 cocode-rs: JSON-RPC Structured Protocol

**Wire format**: Standard JSON-RPC 2.0
```json
// Client → Server (ClientRequest)
{"jsonrpc":"2.0","id":"1","method":"session/start","params":{...}}

// Server → Client (ServerNotification, no id)
{"jsonrpc":"2.0","method":"turn/started","params":{...}}

// Server → Client (ServerRequest, has id, expects response)
{"jsonrpc":"2.0","id":"srv-1","method":"approval/askForApproval","params":{...}}

// Client → Server (Response to ServerRequest)
{"jsonrpc":"2.0","id":"srv-1","result":{...}}
```

**ClientRequest (22 variants)**:
| Category | Variants |
|----------|----------|
| Session | `session/start`, `session/resume`, `session/list`, `session/read`, `session/archive` |
| Turn | `turn/start`, `turn/interrupt` |
| Approval | `approval/resolve` |
| Input | `input/resolveUserInput` |
| Control | `control/setModel`, `control/setPermissionMode`, `control/stopTask`, `control/setThinking`, `control/rewindFiles`, `control/updateEnv`, `control/keepAlive`, `control/cancelRequest` |
| Config | `config/read`, `config/value/write` |
| Hook | `hook/callbackResponse` |
| MCP | `mcp/routeMessageResponse` |

**ServerRequest (5 variants, server → client, expects response)**:
| Method | Purpose |
|--------|---------|
| `approval/askForApproval` | Permission approval request |
| `input/requestUserInput` | User input request |
| `mcp/routeMessage` | MCP message routing |
| `hook/callback` | Hook callback |
| `control/cancelRequest` | Cancel pending request |

### 3.2 TS: Custom Control Protocol

**Wire format**: Custom type-discriminated JSON
```json
// Client → Server (control_request)
{"type":"control_request","request_id":"1","request":{"subtype":"initialize",...}}

// Server → Client (SDKMessage, type-discriminated)
{"type":"assistant","message":{...},"session_id":"...","uuid":"..."}

// Server → Client (control_request, for approvals)
{"type":"control_request","request_id":"srv-1","request":{"subtype":"permission",...}}

// Client → Server (control_response)
{"type":"control_response","response":{"subtype":"success","request_id":"1",...}}
```

**Control Requests (21 subtypes)**:
| Subtype | Purpose |
|---------|---------|
| `initialize` | Initialization (hooks, agents, mcp, schema, prompt) |
| `interrupt` | Interrupt turn |
| `permission` | Permission approval (server→client direction) |
| `set_permission_mode` | Change permission mode |
| `set_model` | Switch model |
| `set_max_thinking_tokens` | Change thinking configuration |
| `mcp_status` | MCP status query |
| `get_context_usage` | Context usage query |
| `hook_callback` | Hook callback |
| `mcp_message` | MCP message routing |
| `rewind_files` | File rewind |
| `cancel_async_message` | Cancel async message |
| `seed_read_state` | Read cache warm-up |
| `mcp_set_servers` | Hot-update MCP |
| `reload_plugins` | Reload plugins |
| `mcp_reconnect` | MCP reconnect |
| `mcp_toggle` | MCP toggle |
| `stop_task` | Stop task |
| `apply_flag_settings` | Apply feature flags |
| `get_settings` | Get configuration |
| `elicitation` | MCP elicitation |

### 3.3 Comparison

| Dimension | cocode-rs | TS | Assessment |
|-----------|-----------|-----|------------|
| **Wire format** | JSON-RPC 2.0 (standard) | Custom type+subtype (proprietary) | **cocode-rs wins**: standard protocol, toolchain compatible |
| **Request/Response correlation** | JSON-RPC id mechanism | request_id field (custom) | cocode-rs is more standard |
| **Server → Client requests** | 5 ServerRequest variants (separate enum) | Mixed into control_request (direction unclear) | **cocode-rs wins**: direction is explicit |
| **Error handling** | JsonRpcError { code, message, data } | ControlErrorResponse { error: string } | **cocode-rs wins**: structured error codes |
| **Capability negotiation** | Initialize request (capabilities, protocol_version) | Initialize subtype (hooks, agents, mcp) | cocode-rs is more complete |
| **MCP management** | Missing mcp/status, mcp/setServers, mcp/reconnect, mcp/toggle | Has 4 MCP management subtypes | **TS wins**: more complete MCP runtime management |
| **Plugin management** | Missing plugin/reload | Has reload_plugins | **TS wins** |
| **Context query** | Missing context/usage | Has get_context_usage | **TS wins** |
| **Feature flags** | Missing config/applyFlags | Has apply_flag_settings | **TS wins** |

### 3.4 Control Requests Present in TS but Missing in cocode-rs (Need to Backfill)

| TS Control | Proposed cocode-rs ClientRequest | Priority |
|------------|-----------------------------------|----------|
| `mcp_status` | `mcp/status` → McpStatusResult | P1 |
| `get_context_usage` | `context/usage` → ContextUsageResult | P1 |
| `mcp_set_servers` | `mcp/setServers` → McpSetServersResult | P1 |
| `mcp_reconnect` | `mcp/reconnect` | P1 |
| `mcp_toggle` | `mcp/toggle` | P1 |
| `reload_plugins` | `plugin/reload` → PluginReloadResult | P1 |
| `apply_flag_settings` | `config/applyFlags` | P1 |
| `seed_read_state` | SKIP (TS internal optimization) | — |
| `cancel_async_message` | EVALUATE (TS-specific) | — |

---

## 4. Transport Layer

### 4.1 cocode-rs: 3 Transport Types

```rust
pub enum AppServerTransport {
    Stdio,                               // NDJSON stdin/stdout (single client)
    WebSocket { bind_address: SocketAddr }, // Axum WebSocket (multi client)
}
// + in-process mpsc channel (TUI, does not go through Transport)
```

**Stdio** (transport.rs):
- BufReader::read_line() → serde_json::from_str::<JsonRpcMessage>()
- Write: OutboundMessage → JsonRpcMessage → JSON + "\n"
- Single client, CLI SDK mode

**WebSocket** (transport.rs):
- Axum framework, ws:// bind
- Multi-client support, ConnectionId tracking
- Origin check middleware
- Bidirectional WebSocket frames

**In-process Channel** (TUI direct connection):
- `mpsc::Sender<CoreEvent>` / `mpsc::Receiver<CoreEvent>`
- Zero serialization overhead
- TUI directly consumes CoreEvent (including TuiEvent)

### 4.2 TS: Single Transport

```
NDJSON over stdin/stdout (this is the only option)
  ↓
structuredIO.ts: Stream<StdoutMessage> + AsyncGenerator<StdinMessage>
```

**StructuredIO** (structuredIO.ts):
- `outbound = new Stream<StdoutMessage>()` — FIFO queue, async iterable
- `structuredInput: AsyncGenerator<StdinMessage | SDKMessage>` — stdin parser
- Control request/response mixed on the same channel
- KeepAlive message used to prevent timeout

**VS Code IDE connection** (not direct WebSocket):
- Bridged through MCP + CCR daemon
- DirectConnect mode
- Not a native WebSocket transport

### 4.3 Comparison

| Dimension | cocode-rs | TS | Assessment |
|-----------|-----------|-----|------------|
| **Transport types** | 3 (stdio + WebSocket + channel) | 1 (stdio only) | **cocode-rs wins**: adapts to more scenarios |
| **IDE direct connection** | WebSocket natively supported | CCR bridge indirect connection | **cocode-rs wins**: lower latency |
| **Multi-client** | WebSocket supported | Not supported (single stdio) | **cocode-rs wins** |
| **Serialization format** | JSON-RPC (standard) | Custom JSON (proprietary) | **cocode-rs wins** |
| **In-process** | mpsc channel (zero-copy) | Not supported (must go through stdio) | **cocode-rs wins** |
| **Connection lifecycle** | ConnectionOpened/Closed events | None (process lifecycle) | **cocode-rs wins** |

---

## 5. SDK Client Implementation

### 5.1 cocode-rs Python SDK (cocode-sdk/)

```python
# Usage example
async with CocodeClient(prompt="Fix the bug") as client:
    async for event in client.events():
        print(event.method, event.params)
    # Multi-turn conversation
    async for event in client.send("Add tests"):
        print(event.method)
```

**Architecture**:
```
CocodeClient (client.py, 21KB)
  ├─ __init__(prompt, model, max_turns, cwd, system_prompt, env,
  │           agents, hooks, mcp_servers, tools, sandbox, thinking, ...)
  ├─ start() → SessionStartRequest over transport
  ├─ events() → AsyncIterator[ServerNotification]
  │   ├─ auto-handle approval/askForApproval (if can_use_tool callback)
  │   ├─ auto-handle hook/callback (if hook_handlers registered)
  │   ├─ auto-handle mcp/routeMessage (if tool_registry has handler)
  │   ├─ yield ServerNotification for everything else
  │   └─ break on turn/completed or turn/failed
  ├─ send(text) → TurnStartRequest + events()
  ├─ approve(request_id, decision)
  ├─ respond_to_question(request_id, response)
  ├─ interrupt()
  ├─ set_model(model)
  ├─ set_permission_mode(mode)
  ├─ stop_task(task_id)
  ├─ update_env(env)
  ├─ set_thinking(mode, max_tokens)
  ├─ rewind_files(turn_id)
  ├─ cancel_request(request_id)
  ├─ list_sessions() / read_session() / archive_session()
  └─ close()
```

**Generated types** (protocol.py):
- Pydantic BaseModel with `model_validate()` for all protocol types
- Tagged unions for ServerNotification / ClientRequest / ServerRequest
- Enum types: ApprovalDecision, ItemStatus, FileChangeKind, SandboxMode, ThinkingMode

**Transport** (subprocess_cli.py):
- `SubprocessCLITransport`: spawns cocode binary with `--sdk-mode`
- Bidirectional NDJSON over stdin/stdout
- Binary discovery: PATH → common locations
- `send_line(json)` / `read_lines() → AsyncIterator[dict]`

**Tool integration** (@tool decorator):
- `@tool()` decorator for registering Python functions as MCP tools
- ToolDefinition → SdkMcpToolDef → SessionStartRequest.mcp_servers
- Invoked via mcp/routeMessage routing

### 5.2 TS SDK (agentSdkTypes.ts)

```typescript
// Usage example
import { query } from '@anthropic-ai/claude-code'
const messages: SDKMessage[] = []
for await (const msg of query({ prompt: "Fix the bug" })) {
  messages.push(msg)
}
```

**Architecture**:
```
agentSdkTypes.ts (public API)
  ├─ query(options) → AsyncIterable<SDKMessage>
  ├─ tool(name, desc, schema, handler) → SdkMcpToolDefinition
  ├─ createSdkMcpServer(options) → McpSdkServerConfigWithInstance
  ├─ unstable_v2_createSession(options) → SDKSession
  ├─ unstable_v2_resumeSession(id) → SDKSession
  ├─ unstable_v2_prompt(session, prompt) → AsyncIterable<SDKMessage>
  ├─ getSessionMessages(id) → SessionMessage[]
  ├─ listSessions(options) → SDKSessionInfo[]
  ├─ getSessionInfo(id) → SDKSessionInfo
  ├─ renameSession(id, name)
  ├─ tagSession(id, tag)
  └─ forkSession(id, options) → ForkSessionResult
```

**Note**: All functions throw "not implemented" — actual implementation is in the runtime bridge layer:
- RemoteIO (remote sessions)
- REPLBridge (interactive mode)
- print.ts runHeadlessStreaming (headless mode)

**Message flow**:
```
query() → subprocess spawn → NDJSON stdout
  → StdinMessage/StdoutMessage via StructuredIO
  → SDKMessage via normalizeMessage() + sdkEventQueue drain
  → AsyncIterable<SDKMessage> to consumer
```

### 5.3 Comparison

| Dimension | cocode-rs Python SDK | TS SDK | Assessment |
|-----------|---------------------|--------|------------|
| **Type safety** | Pydantic (runtime validation + IDE completion) | TypeScript types (compile-time) | Both have advantages |
| **Type source** | JSON Schema → generated protocol.py | Zod → generated types | cocode-rs is more standard |
| **Multi-turn** | `client.send("...")` + `client.events()` | `unstable_v2_prompt(session, "...")` | cocode-rs is more stable (non-unstable) |
| **Session management** | `list_sessions`, `read_session`, `archive_session` | `listSessions`, `getSessionInfo`, `forkSession` | TS is richer (fork) |
| **Approval handling** | `can_use_tool` callback (auto-handle) | Must handle manually (or permissionMode) | **cocode-rs wins**: more user-friendly |
| **Hook handling** | `hook_handlers` dict (auto-dispatch) | Must handle manually | **cocode-rs wins** |
| **MCP tool routing** | `@tool()` decorator → auto mcp/routeMessage | `tool()` → SdkMcpToolDefinition | Equivalent |
| **Turn termination detection** | `events()` auto-break on turn/completed | Consumer must determine on its own | **cocode-rs wins** |
| **Event typing** | `ServerNotification` (typed, method-tagged) | `SDKMessage` (union, type+subtype) | **cocode-rs wins**: more precise |
| **Error handling** | `_safe_parse_notification` fallback | No fallback (type-cast) | cocode-rs is more robust |

---

## 6. Event Consumption: End-to-End Comparison

### 6.1 Event Sequence for a Complete Turn

**cocode-rs** (via NDJSON or WebSocket):
```json
{"method":"session/stateChanged","params":{"state":"running"}}
{"method":"turn/started","params":{"turn_id":"t1","turn_number":1}}
{"method":"item/started","params":{"item":{"id":"i1","details":{"type":"reasoning","text":""}}}}
{"method":"reasoning/delta","params":{"item_id":"i1","turn_id":"t1","delta":"Let me think..."}}
{"method":"item/completed","params":{"item":{"id":"i1","details":{"type":"reasoning","text":"Let me think..."}}}}
{"method":"item/started","params":{"item":{"id":"i2","details":{"type":"agent_message","text":""}}}}
{"method":"agentMessage/delta","params":{"item_id":"i2","turn_id":"t1","delta":"I'll fix the bug."}}
{"method":"item/completed","params":{"item":{"id":"i2","details":{"type":"agent_message","text":"I'll fix the bug."}}}}
{"method":"item/started","params":{"item":{"id":"i3","details":{"type":"file_change","changes":[{"path":"src/main.rs","kind":"edit"}],"status":"in_progress"}}}}
{"method":"item/completed","params":{"item":{"id":"i3","details":{"type":"file_change","changes":[{"path":"src/main.rs","kind":"edit"}],"status":"completed"}}}}
{"method":"turn/completed","params":{"turn_id":"t1","usage":{...}}}
{"method":"session/stateChanged","params":{"state":"idle"}}
```

**TS** (via NDJSON):
```json
{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Let me think..."},{"type":"text","text":"I'll fix the bug."},{"type":"tool_use","id":"tu1","name":"Edit","input":{...}}]},"session_id":"s1","uuid":"u1"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu1","content":"OK"}]},"session_id":"s1","uuid":"u2"}
{"type":"result","duration_ms":5000,"num_turns":1,"result":"I'll fix the bug.","is_error":false,"session_id":"s1","uuid":"u3"}
```

### 6.2 Key Differences

| Dimension | cocode-rs | TS |
|-----------|-----------|-----|
| **Granularity** | Item-level lifecycle (started/updated/completed) | Message-level (entire assistant message) |
| **Streaming** | delta-by-delta (AgentMessageDelta, ReasoningDelta) | Complete message (no streaming for SDK) |
| **Tool results** | Semantic ThreadItem (FileChange, CommandExecution) | Raw tool_result content block |
| **Turn boundary** | turn/started + turn/completed (explicit) | result message (implicit) |
| **Session state** | session/stateChanged (explicit) | session_state_changed (SDK event queue, async) |
| **Progress visibility** | item/started → item/updated → item/completed | Single assistant message (batch) |

### 6.3 Consumer Experience

**cocode-rs SDK consumer** (Python):
```python
async for event in client.events():
    match event.method:
        case "item/started":
            details = event.params["item"]["details"]
            if details["type"] == "file_change":
                print(f"Editing {details['changes'][0]['path']}")
        case "agentMessage/delta":
            print(event.params["delta"], end="")
        case "turn/completed":
            print(f"\nDone. Tokens: {event.params['usage']}")
```

**TS SDK consumer**:
```typescript
for await (const msg of query({ prompt: "..." })) {
  if (msg.type === "assistant") {
    for (const block of msg.message.content) {
      if (block.type === "text") console.log(block.text)
      if (block.type === "tool_use") console.log(`Using ${block.name}`)
    }
  }
  if (msg.type === "result") {
    console.log(`Done. Turns: ${msg.num_turns}`)
  }
}
```

**Assessment**: cocode-rs provides a fine-grained event stream (suited for real-time IDE rendering); TS provides coarse-grained messages (suited for simple consumption). The two target different consumption scenarios; cocode-rs is better suited for building rich UIs.

---

## 7. Summary Comparison Matrix

| Dimension | cocode-rs | TS | Winner |
|-----------|-----------|-----|--------|
| **Schema source** | Rust types + schemars | Zod schemas | cocode-rs (standard, strongly typed) |
| **Schema output** | JSON Schema (language-agnostic) | TypeScript types | cocode-rs (multi-language) |
| **Schema types** | 80+ types in bundled schema | ~40 types in SDKMessage | cocode-rs (more complete) |
| **Event architecture** | 3-layer CoreEvent | Flat SDKMessage + side channel | cocode-rs (clean separation) |
| **Stream accumulation** | StreamAccumulator (explicit state machine) | normalizeMessage (generator) | cocode-rs (testable, observable) |
| **ThreadItem semantics** | 9 typed variants | Raw content blocks | cocode-rs (consumer-friendly) |
| **Wire protocol** | JSON-RPC 2.0 | Custom JSON | cocode-rs (standard) |
| **Transport** | stdio + WebSocket + channel | stdio only | cocode-rs (3x coverage) |
| **Bidirectional** | ClientRequest (22) + ServerRequest (5) | control_request (21) | cocode-rs (clearer direction) |
| **MCP runtime mgmt** | Missing 4 requests | Available | TS (need to port) |
| **SDK Client** | Python (mature, typed) | TypeScript (partially unstable) | cocode-rs (more stable API) |
| **Auto-handling** | approval + hook + mcp routing | manual handling | cocode-rs (better DX) |
| **Session management** | list + read + archive | list + info + rename + tag + fork | TS (more features) |
| **Error handling** | JsonRpcError + safe_parse fallback | string error + no fallback | cocode-rs (more robust) |

### TS Capabilities to Backfill (cocode-rs → implementation)

| Priority | Gap | Source |
|----------|-----|--------|
| **P0** | SessionStateChanged notification | TS sdkEventQueue |
| **P0** | Hook lifecycle (Started/Progress/Response) | TS SDKHookStarted/Progress/Response |
| **P0** | Task params enhancement (tool_use_id, usage, summary) | TS SDKTaskStarted/Progress |
| **P0** | SessionResult enhancement (model_usage, denials, errors) | TS SDKResultMessage |
| **P1** | mcp/status ClientRequest | TS mcp_status control |
| **P1** | context/usage ClientRequest | TS get_context_usage control |
| **P1** | mcp/setServers ClientRequest | TS mcp_set_servers control |
| **P1** | mcp/reconnect ClientRequest | TS mcp_reconnect control |
| **P1** | mcp/toggle ClientRequest | TS mcp_toggle control |
| **P1** | plugin/reload ClientRequest | TS reload_plugins control |
| **P1** | config/applyFlags ClientRequest | TS apply_flag_settings control |
| ~~P2~~ | ~~AuthStatus notification~~ (N/A — coco-rs multi-provider auth model) | — |
| **P2** | LocalCommandOutput notification | TS SDKLocalCommandOutputMessage |
| **P2** | FilesPersisted notification | TS SDKFilesPersistedEvent |
| **P2** | ElicitationComplete notification | TS SDKElicitationCompleteMessage |
| **P2** | ToolUseSummary notification | TS SDKToolUseSummaryMessage |
| **P2** | RateLimit params enhancement | TS SDKRateLimitEventMessage |
| **P2** | Session fork/rename/tag API | TS forkSession/renameSession/tagSession |
