# coco-types

Foundation types shared across all crates. **Source-level
vercel-ai-free** — provider DTOs arrive via `coco-llm-types`; see
"Vercel-AI Seam" below.

## Key Types

Tool / Agent identity: `ToolName` (builtin tool names, Copy — see `src/tool.rs`), `ToolId` (Builtin/Mcp/Custom, flat-string serde), `SubagentType` (builtin subagents — see `src/agent.rs`), `AgentTypeId`, `ToolProgress`.

Permission: `PermissionMode` (camelCase wire), `PermissionBehavior`, `PermissionRule`, `PermissionRuleSource`, `PermissionDecision`, `PermissionDecisionReason`, `ToolPermissionContext`.

Hook / Task / Command: `HookEventType` (`#[non_exhaustive]` — see `src/hook.rs`), `HookOutcome`, `HookScope`, `TaskType`, `TaskStatus`, `TaskStateBase`, `CommandBase`, `CommandType`, `CommandSource`.

Provider / Model: `ProviderApi`, `ModelRole`, `ModelSpec`, `Capability`, `CapabilitySet`, `ApplyPatchToolType`, `WireApi`.

Thinking / Token / ID / Sandbox: `ThinkingLevel { effort, budget_tokens, options }`, `ReasoningEffort`, `TokenUsage`, `ModelUsage`, `SessionId`, `AgentId`, `TaskId`, `SandboxMode`.

Event envelope (owned here — see `event-system-design.md`): `CoreEvent` (3-layer), `ServerNotification` (see `src/event.rs`; Turn lifecycle is `TurnStarted` + `TurnEnded(TurnEndedParams)` with discriminated `TurnOutcome`) + `NotificationMethod` (typed wire-method enum), `AgentStreamEvent`, `TuiOnlyEvent`, `ThreadItem`, plus per-event param structs.

Wire protocol: `ClientRequest` + `ClientRequestMethod` (see `src/client_request.rs`), `ServerRequest` + `ServerRequestMethod` (see `src/server_request.rs`), `JsonRpcMessage` family, `RequestId`, `error_codes`.
`TurnStartParams` — shared SDK/TUI local-AppServer turn DTO: prompt + optional paste images, slash metadata attachment text, and turn-scoped model / permission-mode / thinking overrides.

Attachment taxonomy: `AttachmentKind` (see `src/attachment_kind.rs`), `AttachmentEvent`, `Coverage`, `coverage_of`.

App-state: `ToolAppState`, `AppStatePatch`, `AppStateReadHandle` (typed cross-turn state).

Extended types: `AgentColorEntry`, `AttributionSnapshotEntry`, `CommandResultDisplay`, `PermissionExplanation`, `PromptRequest`, `RiskLevel`, `SessionMode`, `SummaryEntry`, etc.

### Message family (in `messages/` submodule, flat re-exported at crate root)

- **Envelope**: `Message` + `UserMessage`, `AssistantMessage`, `ToolResultMessage`, `AttachmentMessage`, `ProgressMessage`, `TombstoneMessage`. Tool-use summaries are NOT a Message variant — they ride a `ServerNotification::ToolUseSummary` side-channel into `tool_group_summaries` (UI-only label cache, I-3).
- **System**: `SystemMessage` + sub-variants + `SystemMessageLevel`.
- **Attachment payloads**: `AttachmentBody`, `SilentPayload` + payload structs, `AttachmentEmitter`.
- **Tool / hook result**: `ToolResult<T>`, `HookResult`.
- **Persistence**: `SerializedMessage`, `TranscriptMessage`, `TranscriptEntry`.
- **Metadata enums**: `Visibility`, `MessageKind`, `MessageOrigin`, `StopReason`, `ApiError`, `PreservedSegment`, `PartialCompactDirection`.
- **Vercel-ai DTO aliases** (re-exported from `coco-llm-types`): `LlmMessage`, `LlmPrompt`, `UserContent` (= `UserContentPart`), `AssistantContent`, `ToolContent`, `TextContent`, `FileContent`, `ReasoningContent`, `ToolCallContent`, `ToolResultContent` (= `ToolResultPart`), `ToolResultOutput` (= raw `ToolResultContent` from vercel-ai), `ToolResultContentPart`, `DataContent`, plus the `tool_reference_content_part` builder.

The operations layer (`coco-messages`) re-exports these from `coco_types::messages::*`, so the established `coco_messages::Message` import path keeps working.

`CompactTrigger` lives in coco-types root because `event::CompactionPhaseParams` references it.

### Wire-tagged-enum macro

`ServerNotification`, `ClientRequest`, and `ServerRequest` are emitted via the `wire_tagged_enum!` macro (`src/wire_tagged.rs`). From a single `"wire-string" => Variant` table the macro derives:

1. The tagged union (`#[serde(tag = "method", content = "params")]`) with per-variant `#[serde(rename = "wire/method")]`.
2. A companion `FooMethod` Copy enum with `serde` + `strum::Display` + `strum::IntoStaticStr` + `JsonSchema`.
3. A `pub const fn method(&self) -> FooMethod` accessor on the tagged union.

The same `$wire` literal drives `#[serde(rename)]` **and** `#[strum(serialize)]`, so the wire string cannot drift across accessors, schema, or cross-language codegens.

`ServerNotification::MessageAppended.message: Message` is the typed wire payload (Message lives at crate root). SDK JSON Schema for that field stays opaque (`schemars(with = "serde_json::Value")`) because vercel-ai DTOs that Message embeds don't derive `JsonSchema` — adding schemars across `vercel-ai-provider` is a separate cross-crate feature-gate task.

## Vercel-AI Seam

Depends on `coco-llm-types` (DTO seam) for the LLM aliases Message
embeds — never on `vercel-ai-provider` directly. Upgrading the SDK
edits only `common/llm-types` (DTO seam) + `services/inference`
(runtime/client seam), the two crates that own the direct vercel-ai
dep; this crate stays unchanged. CI gate: `scripts/check-vercel-ai-seam.sh`.

## Conventions

- `ToolId` / `AgentTypeId` serialize as flat strings via `Display` / `FromStr` (not tagged JSON). `"Read"` / `"mcp__slack__send"` / `"my_plugin_tool"`.
- `PermissionMode` wire format is camelCase. Snake-case aliases accepted on deserialize for legacy transcripts.
- `side_query` module contains data types for the async `SideQuery` trait (trait itself lives in `coco-tool-runtime`).
