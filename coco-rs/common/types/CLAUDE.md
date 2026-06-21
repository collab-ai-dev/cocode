# coco-types

Foundation types shared across all crates. **Source-level
vercel-ai-free.** Provider DTOs reach this crate via `coco-llm-types`
(the DTO seam) — no direct `vercel_ai_provider::*` import here.
Upgrading the SDK only edits `common/llm-types` + `services/inference`;
this crate stays unchanged. Guarded by `scripts/check-vercel-ai-seam.sh`.

## Key Types

Tool / Agent identity: `ToolName` (43 builtin variants, Copy), `ToolId` (Builtin/Mcp/Custom, flat-string serde), `SubagentType` (7 builtin variants), `AgentTypeId`, `ToolProgress`.

Permission: `PermissionMode` (camelCase wire), `PermissionBehavior`, `PermissionRule`, `PermissionRuleSource`, `PermissionDecision`, `PermissionDecisionReason`, `ToolPermissionContext`.

Hook / Task / Command: `HookEventType` (32 variants, `#[non_exhaustive]`), `HookOutcome`, `HookScope`, `TaskType`, `TaskStatus`, `TaskStateBase`, `CommandBase`, `CommandType`, `CommandSource`.

Provider / Model: `ProviderApi`, `ModelRole`, `ModelSpec`, `Capability`, `CapabilitySet`, `ApplyPatchToolType`, `WireApi`.

Thinking / Token / ID / Sandbox: `ThinkingLevel { effort, budget_tokens, options }`, `ReasoningEffort` (7 variants), `TokenUsage`, `ModelUsage`, `SessionId`, `AgentId`, `TaskId`, `SandboxMode`.

Event envelope (owned here — see `event-system-design.md`): `CoreEvent` (3-layer), `ServerNotification` (59 variants — Turn lifecycle is `TurnStarted` + `TurnEnded(TurnEndedParams)` with discriminated `TurnOutcome`: Completed/Failed/Interrupted/MaxTurnsReached/BudgetExhausted) + `NotificationMethod` (typed wire-method enum), `AgentStreamEvent`, `TuiOnlyEvent`, `ThreadItem`, plus 50+ event param structs.

Wire protocol: `ClientRequest` + `ClientRequestMethod` (30 variants), `ServerRequest` + `ServerRequestMethod` (5 variants), `JsonRpcMessage` family, `RequestId`, `error_codes`.

Attachment taxonomy: `AttachmentKind` (60 variants), `AttachmentEvent`, `Coverage`, `coverage_of`.

App-state: `ToolAppState`, `AppStatePatch`, `AppStateReadHandle` (typed cross-turn state).

Extended types: `AgentColorEntry`, `AttributionSnapshotEntry`, `CommandResultDisplay`, `PermissionExplanation`, `PromptRequest`, `RiskLevel`, `SessionMode`, `SummaryEntry`, etc.

### Message family (in `messages/` submodule, flat re-exported at crate root)

- **Envelope**: `Message` (7 variants), `UserMessage`, `AssistantMessage`, `ToolResultMessage`, `AttachmentMessage`, `ProgressMessage`, `TombstoneMessage`. Tool-use summaries are NOT a Message variant — they ride a `ServerNotification::ToolUseSummary` side-channel into `tool_group_summaries` (UI-only label cache, I-3).
- **System**: `SystemMessage` + 16 sub-variants + `SystemMessageLevel`.
- **Attachment payloads**: `AttachmentBody`, `SilentPayload`, 10 silent payload structs, `AttachmentEmitter`.
- **Tool / hook result**: `ToolResult<T>`, `HookResult`.
- **Persistence**: `SerializedMessage`, `TranscriptMessage`, `TranscriptEntry`.
- **Metadata enums**: `Visibility`, `MessageKind`, `MessageOrigin`, `StopReason`, `ApiError`, `PreservedSegment`, `PartialCompactDirection`.
- **Vercel-ai DTO aliases** (re-exported from `coco-llm-types`): `LlmMessage`, `LlmPrompt`, `UserContent` (= `UserContentPart`), `AssistantContent`, `ToolContent`, `TextContent`, `FileContent`, `ReasoningContent`, `ToolCallContent`, `ToolResultContent` (= `ToolResultPart`), `ToolResultOutput` (= raw `ToolResultContent` from vercel-ai), `ToolResultContentPart`, `DataContent`, plus the `tool_reference_content_part` builder.

The operations layer (`coco-messages`) re-exports these from
`coco_types::messages::*` so the established `coco_messages::Message`
import path keeps working.

`CompactTrigger` lives in coco-types root because `event::CompactionPhaseParams` references it.

### Wire-tagged-enum macro

`ServerNotification`, `ClientRequest`, and `ServerRequest` are emitted via the `wire_tagged_enum!` macro (`src/wire_tagged.rs`). From a single `"wire-string" => Variant` table the macro derives:

1. The tagged union (`#[serde(tag = "method", content = "params")]`) with per-variant `#[serde(rename = "wire/method")]`.
2. A companion `FooMethod` Copy enum with `serde` + `strum::Display` + `strum::IntoStaticStr` + `JsonSchema`.
3. A `pub const fn method(&self) -> FooMethod` accessor on the tagged union.

The same `$wire` literal drives `#[serde(rename)]` **and** `#[strum(serialize)]`, so the wire string cannot drift across accessors, schema, or cross-language codegens.

`ServerNotification::MessageAppended.message: Message` is the typed wire payload (Message lives at crate root). SDK JSON Schema for that field stays opaque (`schemars(with = "serde_json::Value")`) because vercel-ai DTOs that Message embeds don't derive `JsonSchema` — adding schemars across `vercel-ai-provider` is a separate cross-crate feature-gate task.

## Vercel-AI Seam

`coco-types` depends on `coco-llm-types` (the DTO seam crate) for the
LLM type aliases that Message embeds. It does NOT depend on
`vercel-ai-provider` directly — the seam CI gate
(`scripts/check-vercel-ai-seam.sh`) enforces. Two crates own the
direct vercel-ai dep by design:

- `common/llm-types` — DTO seam
- `services/inference` — runtime/client seam

## Conventions

- `ToolId` / `AgentTypeId` serialize as flat strings via `Display` / `FromStr` (not tagged JSON). `"Read"` / `"mcp__slack__send"` / `"my_plugin_tool"`.
- `PermissionMode` wire format is camelCase. Snake-case aliases accepted on deserialize for legacy transcripts.
- `side_query` module contains data types for the async `SideQuery` trait (trait itself lives in `coco-tool-runtime`).
