/* eslint-disable */

// Generated protocol types for the coco TypeScript SDK.

// Regenerate with: ./coco-sdk/scripts/generate_typescript.sh

// DO NOT EDIT MANUALLY.



export const ClientRequestMethod = {
  INITIALIZE: "initialize",
  SESSION_START: "session/start",
  SESSION_RESUME: "session/resume",
  SESSION_LIST: "session/list",
  SESSION_READ: "session/read",
  SESSION_ARCHIVE: "session/archive",
  TURN_START: "turn/start",
  TURN_INTERRUPT: "turn/interrupt",
  APPROVAL_RESOLVE: "approval/resolve",
  INPUT_RESOLVE_USER_INPUT: "input/resolveUserInput",
  ELICITATION_RESOLVE: "elicitation/resolve",
  CONTROL_SET_MODEL: "control/setModel",
  CONTROL_SET_PERMISSION_MODE: "control/setPermissionMode",
  CONTROL_SET_THINKING: "control/setThinking",
  CONTROL_STOP_TASK: "control/stopTask",
  CONTROL_REWIND_FILES: "control/rewindFiles",
  CONTROL_UPDATE_ENV: "control/updateEnv",
  CONTROL_KEEP_ALIVE: "control/keepAlive",
  CONTROL_CANCEL_REQUEST: "control/cancelRequest",
  AGENT_INTERRUPT_CURRENT_WORK: "agent/interruptCurrentWork",
  CONFIG_READ: "config/read",
  CONFIG_VALUE_WRITE: "config/value/write",
  MCP_STATUS: "mcp/status",
  CONTEXT_USAGE: "context/usage",
  MCP_SET_SERVERS: "mcp/setServers",
  MCP_RECONNECT: "mcp/reconnect",
  MCP_TOGGLE: "mcp/toggle",
  PLUGIN_RELOAD: "plugin/reload",
  CONFIG_APPLY_FLAGS: "config/applyFlags",
} as const;
export type ClientRequestMethod = (typeof ClientRequestMethod)[keyof typeof ClientRequestMethod];

export const NotificationMethod = {
  SESSION_STARTED: "session/started",
  SESSION_RESULT: "session/result",
  SESSION_ENDED: "session/ended",
  SESSION_USAGE_UPDATED: "session/usageUpdated",
  HISTORY_MESSAGE_APPENDED: "history/messageAppended",
  HISTORY_MESSAGE_TRUNCATED: "history/messageTruncated",
  HISTORY_RESET_FOR_RESUME: "history/resetForResume",
  HISTORY_REPLACED: "history/replaced",
  HISTORY_REASONING_METADATA_ATTACHED: "history/reasoningMetadataAttached",
  GOAL_ACTIVE_CHANGED: "goal/activeChanged",
  TURN_STARTED: "turn/started",
  TURN_ENDED: "turn/ended",
  ITEM_STARTED: "item/started",
  ITEM_UPDATED: "item/updated",
  ITEM_COMPLETED: "item/completed",
  AGENT_MESSAGE_DELTA: "agentMessage/delta",
  REASONING_DELTA: "reasoning/delta",
  MCP_STARTUP_STATUS: "mcp/startupStatus",
  MCP_STARTUP_COMPLETE: "mcp/startupComplete",
  LSP_PREWARM_COMPLETE: "lsp/prewarmComplete",
  CONTEXT_COMPACTED: "context/compacted",
  CONTEXT_USAGE_WARNING: "context/usageWarning",
  CONTEXT_COMPACTION_STARTED: "context/compactionStarted",
  CONTEXT_COMPACTION_PHASE: "context/compactionPhase",
  CONTEXT_COMPACTION_FAILED: "context/compactionFailed",
  CONTEXT_CLEARED: "context/cleared",
  TASK_STARTED: "task/started",
  TASK_COMPLETED: "task/completed",
  TASK_PROGRESS: "task/progress",
  TASK_PANEL_CHANGED: "task_panel/changed",
  PLAN_APPROVAL_REQUESTED: "plan_approval/requested",
  AGENTS_KILLED: "agents/killed",
  MODEL_FALLBACK_STARTED: "model/fallbackStarted",
  MODEL_FALLBACK_COMPLETED: "model/fallbackCompleted",
  MODEL_FAST_MODE_CHANGED: "model/fastModeChanged",
  MODEL_ROLE_CHANGED: "model/roleChanged",
  PERMISSION_MODE_CHANGED: "permission/modeChanged",
  PROMPT_SUGGESTION: "prompt/suggestion",
  ERROR: "error",
  RATE_LIMIT: "rateLimit",
  KEEP_ALIVE: "keepAlive",
  IDE_SELECTION_CHANGED: "ide/selectionChanged",
  IDE_DIAGNOSTICS_UPDATED: "ide/diagnosticsUpdated",
  QUEUE_STATE_CHANGED: "queue/stateChanged",
  QUEUE_COMMAND_QUEUED: "queue/commandQueued",
  QUEUE_COMMAND_DEQUEUED: "queue/commandDequeued",
  REWIND_COMPLETED: "rewind/completed",
  REWIND_FAILED: "rewind/failed",
  COST_WARNING: "cost/warning",
  SANDBOX_STATE_CHANGED: "sandbox/stateChanged",
  SANDBOX_VIOLATIONS_DETECTED: "sandbox/violationsDetected",
  AGENTS_REGISTERED: "agents/registered",
  HOOK_STARTED: "hook/started",
  HOOK_PROGRESS: "hook/progress",
  HOOK_RESPONSE: "hook/response",
  WORKTREE_ENTERED: "worktree/entered",
  WORKTREE_EXITED: "worktree/exited",
  SUMMARIZE_COMPLETED: "summarize/completed",
  SUMMARIZE_FAILED: "summarize/failed",
  STREAM_STALL_DETECTED: "stream/stallDetected",
  STREAM_WATCHDOG_WARNING: "stream/watchdogWarning",
  STREAM_REQUEST_END: "stream/requestEnd",
  SESSION_STATE_CHANGED: "session/stateChanged",
  LOCAL_COMMAND_OUTPUT: "localCommand/output",
  FILES_PERSISTED: "files/persisted",
  ELICITATION_COMPLETE: "elicitation/complete",
  TOOL_USE_SUMMARY: "tool/useSummary",
  TOOL_PROGRESS: "tool/progress",
  PLUGINS_CHANGED: "plugins/changed",
} as const;
export type NotificationMethod = (typeof NotificationMethod)[keyof typeof NotificationMethod];

export const ServerRequestMethod = {
  APPROVAL_ASK_FOR_APPROVAL: "approval/askForApproval",
  INPUT_REQUEST_USER_INPUT: "input/requestUserInput",
  MCP_ROUTE_MESSAGE: "mcp/routeMessage",
  HOOK_CALLBACK: "hook/callback",
  CONTROL_CANCEL_REQUEST: "control/cancelRequest",
  MCP_REQUEST_ELICITATION: "mcp/requestElicitation",
} as const;
export type ServerRequestMethod = (typeof ServerRequestMethod)[keyof typeof ServerRequestMethod];

/**
 * Session-scoped goal metadata for `/goal`.
 */
export interface ActiveGoal {
  condition: string;
  iterations: number;
  last_reason?: string | null;
  set_at_ms: number;
  tokens_at_start: number;
}

/**
 * Active `/goal` snapshot.
 *
 * `goal = None` means no active goal. Terminal goal_status attachments
 * still carry achieved / failed details; this event is only the live-state
 * mirror.
 */
export interface ActiveGoalChangedParams {
  goal?: ActiveGoal | null;
}

/**
 * `AgentColorName`. Validated set; unknown values are dropped at parse
 * time with a warning so the runtime never sees an invalid color.
 */
export type AgentColorName = "red" | "blue" | "green" | "yellow" | "purple" | "orange" | "pink" | "cyan";

export interface AgentInfo {
  description?: string | null;
  name: string;
}

/**
 * Params for `agent/interruptCurrentWork`.
 *
 * Aborts the target teammate's current model/tool turn while keeping the
 * teammate process alive for later messages.
 */
export interface AgentInterruptCurrentWorkParams {
  agent_id: string;
}

/**
 * One entry in `AgentDefinition.mcp_servers`:
 * either a `string` (reference to an existing MCP server config) or
 * an inline `{name: config}` mapping that stands up a dynamic
 * server scoped to this agent. Inline configs are stored as
 * `serde_json::Value` because the underlying `McpServerConfig`
 * shape lives in `coco-mcp` (a higher layer that depends on
 * `coco-types`); keeping it as opaque JSON avoids a back-edge.
 */
export type AgentMcpServerSpec = string | { [key: string]: unknown; };

/**
 * Where an agent definition came from. Drives precedence when the same
 * `agent_type` is defined in multiple places: later source wins.
 */
export type AgentSource = "built-in" | "plugin" | "userSettings" | "projectSettings" | "flagSettings" | "policySettings";

/**
 * Agent-loop stream events. Higher-level than `coco_types::StreamEvent`
 * (which represents raw LLM inference deltas). Adds:
 * - Tool lifecycle states (Queued → Started → Completed)
 * - MCP tool call tracking
 * - Turn-scoped item IDs
 *
 * Input to `StreamAccumulator`.
 * See `event-system-design.md` Section 1.5.
 */
export type AgentStreamEvent = {
  delta: string;
  turn_id: string;
  type: "text_delta";
} | {
  delta: string;
  turn_id: string;
  type: "thinking_delta";
} | {
  call_id: string;
  input: unknown;
  name: string;
  type: "tool_use_queued";
} | {
  batch_id?: string | null;
  call_id: string;
  name: string;
  type: "tool_use_started";
} | {
  call_id: string;
  is_error: boolean;
  name: string;
  output: string;
  type: "tool_use_completed";
} | {
  call_id: string;
  server: string;
  tool: string;
  type: "mcp_tool_call_begin";
} | {
  call_id: string;
  is_error: boolean;
  server: string;
  tool: string;
  type: "mcp_tool_call_end";
};

/**
 * One row in the `/agents` Library tab.
 */
export interface AgentsDialogEntry {
  color?: AgentColorName | null;
  description: string;
  is_overridden?: boolean;
  name: string;
  source: AgentSource;
  source_path?: string | null;
}

/**
 * Payload for [`TuiOnlyEvent::OpenAgentsDialog`]. Built by the
 * `/agents` slash handler with everything the 2-tab dialog needs
 * (Running tab reads `SessionState.subagents` directly, so the
 * payload only carries Library data).
 *
 * Carries Library tab data for the agents overlay.
 */
export interface AgentsDialogPayload {
  entries: Array<AgentsDialogEntry>;
}

export interface AgentsKilledParams {
  agent_ids?: Array<string>;
  count: number;
}

/**
 * TS carries the (potentially truncated) file content inline for UI display
 * even though `normalizeAttachmentForAPI` returns `[]`. coco-rs follows
 * suit — `content` is the last-known file body used by transcript viewers.
 */
export interface AlreadyReadFilePayload {
  content?: string;
  display_path: string;
  filename: string;
  truncated?: boolean;
}

/**
 * API error attached to an assistant message.
 * `error_type` carries the short canonical code
 * on `AssistantMessage.error` (`max_output_tokens`, `prompt_too_long`,
 * `content_filter`, `rate_limited`, `overloaded`, `blocking_limit`,
 * `model_error`, …). The C3 death-spiral guard forwards it as the
 * `error` field of the StopFailure hook input so hook matchers can
 * filter by specific error type; surfaces in the SDK transcript as
 * the typed equivalent of the `lastMessage.error` wire field.
 */
export interface ApiError {
  error_type?: string | null;
  message: string;
  status_code?: number | null;
}

/**
 * Active API backend.
 */
export type ApiProvider = "firstParty" | "bedrock" | "vertex" | "foundry";

/**
 * Bounded, structured preview of an `apply_patch` body for UI rendering.
 */
export interface ApplyPatchPreview {
  rows: Array<ApplyPatchPreviewRow>;
}

export type ApplyPatchPreviewAction = "add" | "delete" | "update";

export type ApplyPatchPreviewRow = {
  action: ApplyPatchPreviewAction;
  kind: "header";
  target: string;
} | {
  content: string;
  kind: "line";
  sign: ApplyPatchPreviewSign;
} | {
  content: string;
  kind: "raw";
} | {
  kind: "omitted";
  rows: number;
};

export type ApplyPatchPreviewSign = "added" | "removed" | "context";

/**
 * How a model's `apply_patch` tool is presented to the model. Per-model,
 * **optional** (`None` → the default `Freeform`). Mirrors codex-rs, whose
 * `ApplyPatchToolType` likewise carries only `Freeform`. Read by
 * `apply_patch.rs::tool_spec` via `SchemaContext`.
 */
export type ApplyPatchToolType = "freeform";

/**
 * Permission approval decision.
 */
export type ApprovalDecision = "allow" | "deny";

/**
 * The SDK is *resolving* a pending approval request, sent client→server.
 */
export interface ApprovalResolveParams {
  content_blocks?: Array<unknown> | null;
  decision: ApprovalDecision;
  feedback?: string | null;
  permission_update?: PermissionUpdate | null;
  request_id: string;
  updated_input?: unknown;
}

export interface AskForApprovalParams {
  agent_id?: string | null;
  blocked_path?: string | null;
  cwd?: string | null;
  decision_reason?: string | null;
  description?: string | null;
  display_name?: string | null;
  input: unknown;
  permission_suggestions?: Array<unknown>;
  request_id: string;
  title?: string | null;
  tool_name: string;
  tool_use_id: string;
}

/**
 * One answered (or unanswered) question in an [`AskUserQuestionResult`].
 */
export interface AskUserQuestionAnswered {
  answers: Array<string>;
  note?: string | null;
  question: string;
}

/**
 * Per-question answers for a completed AskUserQuestion call. Built by the tool
 * from the spliced `answers`/`annotations` envelope; the model still sees the
 * prose in `ToolResultMessage.message`.
 */
export interface AskUserQuestionResult {
  questions: Array<AskUserQuestionAnswered>;
}

/**
 * Assistant message content parts.
 */
export type AssistantContentPart = TextPart | FilePart | ReasoningPart | ReasoningFilePart | CustomPart | ToolCallPart | ToolResultPart | SourcePart | ToolApprovalRequestPart;

export interface AssistantMessage {
  api_error?: ApiError | null;
  cost_usd?: number | null;
  message: LanguageModelV4Message;
  model?: string;
  request_id?: string | null;
  stop_reason?: UnifiedFinishReason | null;
  usage?: TokenUsage | null;
  uuid: string;
}

/**
 * Typed payload for an [`AttachmentMessage`](super::AttachmentMessage).
 */
export type AttachmentBody = LanguageModelV4Message | SilentPayload | {
  body: "unit";
};

/**
 * Typed structured extras carried alongside an [`AttachmentBody::Api`] body.
 * Used for kinds that surface both a rendered model-visible prompt
 * **and** a structured payload that downstream consumers (transcript
 * persistence, SDK observers, telemetry) want preserved verbatim.
 * `body` carries the rendered prompt; `extras` carries the
 * derived-from-structure original data.
 */
export type AttachmentExtras = SkillDiscoveryPayload | CompactFileReferencePayload | MentionSummaryPayload;

/**
 * Every `AttachmentKind` discriminator, plus coco-rs-synthetic
 * reminder kinds. 65 variants.
 * Wire format is snake_case via `#[serde(rename_all = "snake_case")]`
 * to match `AttachmentKind` exactly, so transcripts round-trip.
 */
export type AttachmentKind = "plan_mode" | "plan_mode_reentry" | "plan_mode_exit" | "auto_mode" | "auto_mode_exit" | "todo_reminder" | "task_reminder" | "compaction_reminder" | "date_change" | "verify_plan_reminder" | "ultrathink_effort" | "workflow_keyword_request" | "token_usage" | "budget_usd" | "output_token_usage" | "companion_intro" | "deferred_tools_delta" | "agent_listing_delta" | "mcp_instructions_delta" | "hook_success" | "hook_blocking_error" | "hook_additional_context" | "hook_stopped_continuation" | "async_hook_response" | "diagnostics" | "output_style" | "queued_command" | "task_status" | "skill_listing" | "invoked_skills" | "teammate_mailbox" | "team_context" | "mcp_resource" | "agent_mention" | "selected_lines_in_ide" | "opened_file_in_ide" | "nested_memory" | "relevant_memories" | "already_read_file" | "edited_image_file" | "file" | "directory" | "pdf_reference" | "compact_file_reference" | "plan_file_reference" | "edited_text_file" | "command_permissions" | "hook_cancelled" | "hook_error_during_execution" | "hook_non_blocking_error" | "hook_permission_decision" | "hook_system_message" | "goal_status" | "structured_output" | "dynamic_skill" | "skill_discovery" | "context_efficiency" | "max_turns_reached" | "current_session_memory" | "teammate_shutdown_batch" | "bagel_console" | "critical_system_reminder" | "slash_command_metadata" | "user_context" | "tool_search_usage_reminder";

/**
 * Attachment message: `kind` carries the discriminant (60 variants),
 * `body` carries the typed payload.
 * **Invariant**: `kind` and `body` must agree — e.g. `kind = HookCancelled`
 * must come with `body = Silent(SilentPayload::HookCancelled(..))`. Do **not**
 * construct via struct literal; use the typed constructor helpers below.
 */
export interface AttachmentMessage {
  body: AttachmentBody;
  extras?: AttachmentExtras | null;
  kind: AttachmentKind;
  uuid: string;
}

export interface AttachmentTypeBreakdown {
  name: string;
  tokens: number;
}

/**
 * Payload for [`TurnOutcome::BudgetExhausted`]. `budget_tokens` is
 * the configured `max_tokens` ceiling; `None` when no explicit max
 * was set (the 90%-of-window heuristic still drove the stop, but
 * there is no honest single number to emit).
 */
export interface BudgetExhaustedOutcome {
  budget_tokens?: number | null;
  used_tokens: number;
}

/**
 * Params for `control/cancelRequest`.
 */
export interface CancelRequestParams {
  reason?: string | null;
  request_id: string;
}

/**
 * Model capabilities (checked at request time).
 */
export type Capability = "text_generation" | "streaming" | "vision" | "audio" | "tool_calling" | "embedding" | "extended_thinking" | "structured_output" | "reasoning_summaries" | "parallel_tool_calls" | "fast_mode" | "prompt_cache" | "context_1m" | "interleaved_thinking" | "context_management" | "adaptive_thinking" | "token_efficient_tools" | "anthropic_tool_reference" | "client_side_tool_search_promotion" | "open_ai_native_tool_search";

/**
 * Typed command argument shape used by UI completion and submit semantics.
 */
export type CommandArgumentKind = "none" | "free_text" | "file_path" | "directory_path" | "session_id";

export interface CommandPermissionsPayload {
  allowedTools: Array<string>;
  model?: string | null;
}

/**
 * How a command was loaded.
 * Payload-carrying variants (`Plugin { name }`, `Mcp { server_name }`)
 * ensure source and attribution can never disagree. This replaces the
 * older `loaded_from + plugin_name` dual-field layout, which allowed
 * nonsensical states (e.g. `loaded_from = Builtin` paired with
 * `plugin_name = Some(...)`).
 */
export type CommandSource = {
  kind: "builtin";
} | {
  kind: "bundled";
} | {
  kind: "user";
} | {
  kind: "project";
} | {
  kind: "managed";
} | {
  kind: "skills";
} | {
  kind: "commands_deprecated";
} | {
  kind: "plugin";
  name: string;
} | {
  kind: "mcp";
  server_name: string;
};

/**
 * Tag-only projection of [`CommandType`]. Implements [`Copy`] so the
 * UI snapshot ([`SlashCommandInfo`]) and the autocomplete ranker can
 * pass it around without cloning.
 */
export type CommandTypeTag = "prompt" | "local" | "local_overlay";

/**
 * compact file reference attachment.
 */
export interface CompactFileReferencePayload {
  display_path: string;
  filename: string;
}

/**
 * How compaction was triggered.
 * Stays in `coco-types` (rather than `coco-messages`) because
 * `event::CompactionPhaseParams` references it; the rest of the message
 * family lives in `coco-messages`.
 * Variants: manual `/compact`, threshold-based auto, PTL-413
 * reactive recovery, gap-based time-based microcompact, session-memory
 * short-circuit (no LLM), and staged context-collapse commit.
 */
export type CompactTrigger = "manual" | "auto" | "reactive" | "time_based" | "session_memory" | "context_collapse";

export interface CompactionFailedParams {
  attempts?: number;
  error: string;
}

export type CompactionHookType = "pre_compact" | "post_compact" | "session_start";

/**
 * Sub-phase of a compaction in progress (TS `onCompactProgress`).
 *
 * Mirrors the TS phase taxonomy at Tool.ts:150-156:
 *   - `HooksStart { hook_type }` for PreCompact / PostCompact / SessionStart
 *   - `Summarizing` for the LLM summarizer call
 *   - `Done` to clear the spinner
 */
export type CompactionPhase = "hooks_start" | "summarizing" | "done";

export interface CompactionPhaseParams {
  hook_type?: CompactionHookType | null;
  phase: CompactionPhase;
}

/**
 * Payload for [`TurnOutcome::Completed`]. `stop_reason` is the *last
 * LLM round's* finish reason at the moment the engine returned.
 * `None` when the cycle ended without ever resolving a real model
 * finish reason (structured-output retry cap, Stop-hook prevent
 * before any round resolved one) — emitters must not fabricate.
 */
export interface CompletedOutcome {
  stop_reason?: UnifiedFinishReason | null;
}

/**
 * Params for `config/applyFlags`.
 */
export interface ConfigApplyFlagsParams {
  settings: { [key: string]: unknown; };
}

/**
 * Input for ConfigChange hooks.
 */
export interface ConfigChangeInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  file_path?: string | null;
  permission_mode?: string | null;
  session_id: string;
  source: ConfigChangeSource;
  transcript_path?: string;
  hook_event_name: "ConfigChange";
}

/**
 * ConfigChange `source`: `user_settings`, `project_settings`, `local_settings`, `policy_settings`, or `skills`.
 */
export type ConfigChangeSource = "user_settings" | "project_settings" | "local_settings" | "policy_settings" | "skills";

/**
 * Aggregate response to `ClientRequest::ConfigRead`.
 */
export interface ConfigReadResult {
  config: unknown;
  sources?: { [key: string]: unknown; };
}

/**
 * Params for `config/value/write`.
 */
export interface ConfigWriteParams {
  key: string;
  scope?: string | null;
  value: unknown;
}

export interface ContentDeltaParams {
  delta: string;
  item_id?: string | null;
  turn_id?: string | null;
}

export interface ContextAgent {
  agent_type: string;
  source: string;
  tokens: number;
}

/**
 * Closed set of context-window usage categories. Each renderer maps the
 * kind to its own label + color; the wire carries the typed kind so the
 * vocabulary cannot drift across SDK / TUI / cross-language codegens.
 */
export type ContextCategoryKind = "system_prompt" | "tools" | "mcp_tools" | "agents" | "memory_files" | "skills" | "messages" | "free";

export interface ContextClearedParams {
  new_mode?: string | null;
}

export interface ContextCompactedParams {
  post_tokens?: number | null;
  pre_tokens?: number | null;
  removed_messages: number;
  summary_tokens: number;
  trigger?: CompactTrigger;
}

export interface ContextMcpTool {
  is_loaded: boolean;
  name: string;
  server_name: string;
  tokens: number;
}

export interface ContextMemoryFile {
  path: string;
  source: string;
  tokens: number;
}

export interface ContextSkill {
  name: string;
  source: string;
  tokens: number;
}

/**
 * An actionable suggestion shown under the `/context` view.
 */
export interface ContextSuggestion {
  detail: string;
  savings_tokens?: number | null;
  severity: SuggestionSeverity;
  title: string;
}

export interface ContextUsageCategory {
  kind: ContextCategoryKind;
  tokens: number;
}

/**
 * Response to `ClientRequest::ContextUsage`.
 * Simplified subset — TS includes a rich breakdown grid that's UI-specific.
 */
export interface ContextUsageResult {
  agents?: Array<ContextAgent>;
  auto_compact_threshold?: number | null;
  categories: Array<ContextUsageCategory>;
  is_auto_compact_enabled: boolean;
  max_tokens: number;
  mcp_tools?: Array<ContextMcpTool>;
  memory_files?: Array<ContextMemoryFile>;
  message_breakdown?: MessageBreakdown | null;
  model: string;
  percentage: number;
  raw_max_tokens: number;
  skills?: Array<ContextSkill>;
  suggestions?: Array<ContextSuggestion>;
  total_tokens: number;
}

export interface ContextUsageWarningParams {
  estimated_tokens: number;
  percent_left: number;
  warning_threshold: number;
}

export interface CostWarningParams {
  budget_cents?: number | null;
  current_cost_cents: number;
  threshold_cents: number;
}

/**
 * A custom content part for provider-specific extensions.
 *
 * Used in both prompts (with `provider_options`) and responses (with `provider_metadata`).
 */
export interface CustomPart {
  kind: string;
  providerMetadata?: ProviderMetadata | null;
  providerOptions?: ProviderOptions | null;
}

/**
 * Input for CwdChanged hooks.
 */
export interface CwdChangedInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  new_cwd: string;
  old_cwd: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "CwdChanged";
}

export interface DynamicSkillPayload {
  displayPath: string;
  skillDir: string;
  skillNames: Array<string>;
}

/**
 * Image bytes can't be diffed textually; the UI renders a marker / thumbnail.
 */
export interface EditedImageFilePayload {
  display_path: string;
  filename: string;
}

/**
 * Model effort tier.
 */
export type EffortLevel = "low" | "medium" | "high" | "max";

/**
 * Elicitation user action. TS:
 * `z.enum(['accept', 'decline', 'cancel'])`.
 */
export type ElicitationAction = "accept" | "decline" | "cancel";

/**
 * Matches TS `SDKElicitationCompleteMessage` (coreSchemas.ts:1779-1792).
 *
 * Emitted after an MCP server's elicitation request is resolved
 * (either submitted or cancelled).
 */
export interface ElicitationCompleteParams {
  elicitation_id: string;
  mcp_server_name: string;
}

/**
 * Input for Elicitation hooks.
 *
 * Fields: `{mcp_server_name, message, mode?, url?, elicitation_id?, requested_schema?}`.
 */
export interface ElicitationInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  elicitation_id?: string | null;
  mcp_server_name: string;
  message: string;
  mode?: ElicitationMode | null;
  permission_mode?: string | null;
  requested_schema?: unknown;
  session_id: string;
  transcript_path?: string;
  url?: string | null;
  hook_event_name: "Elicitation";
}

/**
 * Elicitation `mode`: `form` or `url`.
 */
export type ElicitationMode = "form" | "url";

/**
 * Params for `elicitation/resolve`.
 *
 * Sent client→server in response to a prior `ServerRequest` that
 * asked the client to collect structured input on behalf of an MCP
 * server (form values, OAuth tokens, etc.). The client populates
 * `values` with the user's input and sets `approved=true`, or sets
 * `approved=false` to reject the elicitation.
 */
export interface ElicitationResolveParams {
  approved: boolean;
  mcp_server_name: string;
  request_id: string;
  values?: { [key: string]: unknown; };
}

/**
 * Input for ElicitationResult hooks.
 *
 * Fields: `{mcp_server_name, elicitation_id?, mode?, action, content?}`.
 */
export interface ElicitationResultInput {
  action: ElicitationAction;
  agent_id?: string | null;
  agent_type?: string | null;
  content?: unknown;
  cwd: string;
  elicitation_id?: string | null;
  mcp_server_name: string;
  mode?: ElicitationMode | null;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "ElicitationResult";
}

/**
 * Wire-stable error category. Category-level (11 variants) rather
 * than leaf `StatusCode` (~35 variants) so the protocol does not
 * break every time a new internal status is added. The mapping
 * from `coco_error::StatusCategory` lives in `coco_query::error_code`
 * (the seam between the error layer and the wire types).
 */
export type ErrorCode = "common" | "input" | "io" | "network" | "auth" | "config" | "provider" | "resource" | "system_reminder" | "hook_blocked" | "unknown";

export interface ErrorParams {
  category?: string | null;
  message: string;
  retryable?: boolean;
}

/**
 * Structured error payload for `TurnOutcome::Failed`. Replaces
 * the older opaque `error: String` so Hub / SDK consumers can
 * filter / aggregate by category without parsing the message.
 */
export interface ErrorPayload {
  code: ErrorCode;
  message: string;
}

export interface ExitPlanModeAllowedPrompt {
  prompt: string;
  tool: string;
}

/**
 * First-class outcome of an `ExitPlanMode` tool call.
 */
export type ExitPlanModeOutcome = "implementation_plan" | "no_implementation_plan";

/**
 * UI-only data needed to render an ExitPlanMode result without reading private
 * tool input/output wire fields.
 */
export interface ExitPlanModeResult {
  awaitingLeaderApproval: boolean;
  filePath?: string | null;
  isAgent: boolean;
  outcome: ExitPlanModeOutcome;
  plan: string;
  planWasEdited: boolean;
}

/**
 * SessionEnd `reason`: `clear`, `resume`, `logout`, `prompt_input_exit`, `other`, or `bypass_permissions_disabled`.
 */
export type ExitReason = "clear" | "resume" | "logout" | "prompt_input_exit" | "other" | "bypass_permissions_disabled";

/**
 * Which panel the TUI should have expanded in the task area.
 *
 * **`Teammates` ≠ general subagents.** `Teammates` strictly shows agents
 * with persistent teammate identity (`agentId@teamName`, survives `/clear`,
 * mailbox-based). Async subagents spawned by the `Agent` tool render inline
 * in the transcript and in the `BackgroundTaskStatus` pill row — **not** here.
 *
 * A subagent only appears in this view when the Agent tool was invoked with
 * `teamName` set, which routes through `spawnTeammate()` and transforms the
 * worker into a first-class teammate.
 */
export type ExpandedView = "none" | "tasks" | "teammates";

/**
 * Payload for [`TurnOutcome::Failed`]. `error.code` is the
 * wire-stable category; `error.message` is the human-readable
 * detail. No `stop_reason` — the failure did not originate from
 * the model's finish_reason.
 */
export interface FailedOutcome {
  error: ErrorPayload;
}

/**
 * Matches TS `FastModeStateSchema` (coreSchemas.ts:1883-1889).
 */
export type FastModeState = "off" | "cooldown" | "on";

/**
 * FileChanged `event`: `change`, `add`, or `unlink`.
 */
export type FileChangeEvent = "change" | "add" | "unlink";

export interface FileChangeInfo {
  kind: FileChangeKind;
  path: string;
}

export type FileChangeKind = "create" | "modify" | "delete";

/**
 * Input for FileChanged hooks.
 */
export interface FileChangedInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  event: FileChangeEvent;
  file_path: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "FileChanged";
}

/**
 * A file content part (image, document, etc.).
 *
 * `data` is a tagged discriminated union:
 * - `Data { data }` — raw bytes or base64-encoded string.
 * - `Url { url }` — a URL pointing to the file.
 * - `Reference { reference }` — a provider reference (`{ [provider]: id }`).
 * - `Text { text }` — inline text content.
 */
export interface FilePart {
  data: SharedV4FileData;
  filename?: string | null;
  mediaType: string;
  providerMetadata?: ProviderMetadata | null;
}

/**
 * Matches TS `SDKFilesPersistedEvent` (coreSchemas.ts:1672-1692).
 *
 * TS emits this when files are uploaded or persisted (e.g. after a
 * successful `filesApi` operation).
 */
export interface FilesPersistedParams {
  failed?: Array<PersistedFileError>;
  files: Array<PersistedFileInfo>;
  processed_at: string;
}

/**
 * `/goal` status attachment (`goal_status`).
 * Sentinels (`sentinel: true`) are emitted by `/goal` set/clear and are
 * ignored by "last achieved goal" lookup. Non-sentinel payloads are emitted
 * by the Stop-hook evaluator for achieved, failed, and still-unmet checks.
 */
export interface GoalStatusPayload {
  condition: string;
  durationMs?: number | null;
  failed?: boolean;
  iterations?: number | null;
  met: boolean;
  reason?: string | null;
  sentinel?: boolean;
  tokens?: number | null;
}

/**
 * Hook callback matcher with optional tool-name filter and callback IDs.
 */
export interface HookCallbackMatcher {
  hook_callback_ids: Array<string>;
  matcher?: string | null;
  timeout?: number | null;
}

/**
 * Invoke an SDK-registered hook callback.
 * Correlation is via the **outer** JSON-RPC `request_id` on the
 * envelope — there is no inner `request_id` field. The SDK replies
 * with a `HookCallbackResult` payload as the synchronous response.
 */
export interface HookCallbackParams {
  callback_id: string;
  event_type: HookEventType;
  input: HookInput;
  tool_use_id?: string | null;
}

/**
 * Result body for the synchronous JSON-RPC reply to a
 * `hook/callback` server request.
 * Correlation is via the outer JSON-RPC `request_id` on the response
 * envelope — there is no inner correlation field. The whole body is
 * the hook output in shape; downstream parsers consume
 * `SdkHookOutput` directly via [`HookCallbackResult::output`].
 */
export interface HookCallbackResult {
  output: SdkHookOutput;
}

export interface HookCancelledPayload {
  command?: string | null;
  duration_ms?: number | null;
  hook_event: HookEventType;
  hook_name: string;
  tool_use_id: string;
}

/**
 * Top-level `decision` field. TS:
 * `z.enum(['approve', 'block'])`.
 */
export type HookDecision = "approve" | "block";

export interface HookErrorDuringExecutionPayload {
  content: string;
  hook_event: HookEventType;
  hook_name: string;
  tool_use_id: string;
}

/**
 * 27 hook event types
 * Event type identifiers for hook registration and dispatch.
 * Wire format is **PascalCase** (e.g. `"PreToolUse"`) — identical to
 * TS settings.json keys. Variant names serialize as-is via serde
 * default and strum default; do not add `rename_all`.
 * `#[non_exhaustive]` so future TS additions can land without
 * breaking match exhaustiveness in downstream crates.
 */
export type HookEventType = "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "SessionStart" | "SessionEnd" | "Setup" | "Stop" | "StopFailure" | "SubagentStart" | "SubagentStop" | "UserPromptSubmit" | "PermissionRequest" | "PermissionDenied" | "Notification" | "Elicitation" | "ElicitationResult" | "PreCompact" | "PostCompact" | "TeammateIdle" | "TaskCreated" | "TaskCompleted" | "ConfigChange" | "InstructionsLoaded" | "CwdChanged" | "FileChanged" | "WorktreeCreate" | "WorktreeRemove";

/**
 * Generic hook input — unified envelope for every hook event.
 *
 * Internally tagged on `hook_event_name` (PascalCase wire literal,
 * matching `HookEventType`). The tag field is supplied by serde from
 * the variant identity, so inner structs do NOT carry a redundant
 * `hook_event_name` field. Wire shape:
 *
 * ```json
 * {"hook_event_name":"PreToolUse","session_id":"s","tool_name":"Read",...}
 * ```
 *
 * Compared with `untagged`, this representation:
 *  - lets schemars emit a discriminated `oneOf` with `const` on the
 *    tag field, which downstream codegen (Pydantic discriminated
 *    unions) consumes natively;
 *  - replaces serde's try-each-variant deserialize loop with O(1)
 *    dispatch on the tag value.
 */
export type HookInput = PreToolUseInput | PostToolUseInput | PostToolUseFailureInput | SessionStartInput | SessionEndInput | SetupInput | StopInput | StopFailureInput | PreCompactInput | PostCompactInput | SubagentStartInput | SubagentStopInput | UserPromptSubmitInput | PermissionRequestInput | PermissionDeniedInput | NotificationInput | ElicitationInput | ElicitationResultInput | FileChangedInput | ConfigChangeInput | InstructionsLoadedInput | CwdChangedInput | WorktreeCreateInput | WorktreeRemoveInput | TaskCreatedInput | TaskCompletedInput | TeammateIdleInput;

export interface HookNonBlockingErrorPayload {
  error: string;
  hook_event: HookEventType;
  hook_name: string;
  tool_use_id: string;
}

export type HookOutcomeStatus = "success" | "error" | "cancelled";

/**
 * `allow` / `deny` / `ask` decision from hook output.
 */
export type HookPermissionDecision = "allow" | "deny" | "ask";

export interface HookPermissionDecisionPayload {
  decision: HookPermissionDecision;
  hook_event: HookEventType;
  tool_use_id: string;
}

/**
 * Matches TS `SDKHookProgressMessage` (coreSchemas.ts:1616-1629).
 */
export interface HookProgressParams {
  hook_event: string;
  hook_id: string;
  hook_name: string;
  output?: string;
  stderr?: string;
  stdout?: string;
}

/**
 * Matches TS `SDKHookResponseMessage` (coreSchemas.ts:1631-1646).
 */
export interface HookResponseParams {
  exit_code?: number | null;
  hook_event: string;
  hook_id: string;
  hook_name: string;
  outcome: HookOutcomeStatus;
  output: string;
  stderr?: string;
  stdout?: string;
}

/**
 * Event-specific hook output. Tagged by `hookEventName`.
 * Variants cover every `HOOK_EVENT` value that can carry structured
 * fields back to the agent.
 * Each variant carries `rename_all = "camelCase"` so its inner fields
 * match the TS canonical wire shape (`permissionDecision`,
 * `additionalContext`, `updatedInput`, etc.). The enum-level
 * `rename_all` only applies to variant names, not variant fields, so
 * each struct-like variant needs its own attribute.
 */
export type HookSpecificOutput = {
  additionalContext?: string | null;
  hookEventName: "PreToolUse";
  permissionDecision?: HookPermissionDecision | null;
  permissionDecisionReason?: string | null;
  updatedInput?: unknown;
} | {
  additionalContext?: string | null;
  hookEventName: "PostToolUse";
  updatedMCPToolOutput?: unknown;
} | {
  additionalContext?: string | null;
  hookEventName: "PostToolUseFailure";
} | {
  additionalContext?: string | null;
  hookEventName: "UserPromptSubmit";
} | {
  additionalContext?: string | null;
  hookEventName: "SessionStart";
  initialUserMessage?: string | null;
  watchPaths?: Array<string> | null;
} | {
  additionalContext?: string | null;
  hookEventName: "Setup";
} | {
  additionalContext?: string | null;
  hookEventName: "SubagentStart";
} | {
  hookEventName: "PermissionDenied";
  retry?: boolean | null;
} | {
  additionalContext?: string | null;
  hookEventName: "Notification";
} | {
  decision?: PermissionRequestDecision | null;
  hookEventName: "PermissionRequest";
} | {
  action?: ElicitationAction | null;
  content?: unknown;
  hookEventName: "Elicitation";
} | {
  action?: ElicitationAction | null;
  content?: unknown;
  hookEventName: "ElicitationResult";
} | {
  hookEventName: "CwdChanged";
  watchPaths?: Array<string> | null;
} | {
  hookEventName: "FileChanged";
  watchPaths?: Array<string> | null;
} | {
  hookEventName: "WorktreeCreate";
  worktreePath?: string | null;
};

export interface HookStartedParams {
  hook_event: string;
  hook_id: string;
  hook_name: string;
}

export interface HookSystemMessagePayload {
  content: string;
  hook_event: HookEventType;
  hook_name: string;
  tool_use_id: string;
}

export interface IdeDiagnosticsUpdatedParams {
  diagnostics?: Array<unknown>;
  file_path: string;
  new_count: number;
}

export interface IdeSelectionChangedParams {
  end_line: number;
  file_path: string;
  selected_text: string;
  start_line: number;
}

/**
 * Sent once at session start for capability negotiation. Carries hooks,
 * SDK MCP servers, output format, system prompt, and agent definitions
 * so the agent can construct its registries before the first turn.
 */
export interface InitializeParams {
  agent_progress_summaries?: boolean | null;
  agents?: { [key: string]: SdkAgentDefinition; } | null;
  append_system_prompt?: string | null;
  hooks?: {
  ConfigChange?: Array<HookCallbackMatcher>;
  CwdChanged?: Array<HookCallbackMatcher>;
  Elicitation?: Array<HookCallbackMatcher>;
  ElicitationResult?: Array<HookCallbackMatcher>;
  FileChanged?: Array<HookCallbackMatcher>;
  InstructionsLoaded?: Array<HookCallbackMatcher>;
  Notification?: Array<HookCallbackMatcher>;
  PermissionDenied?: Array<HookCallbackMatcher>;
  PermissionRequest?: Array<HookCallbackMatcher>;
  PostCompact?: Array<HookCallbackMatcher>;
  PostToolUse?: Array<HookCallbackMatcher>;
  PostToolUseFailure?: Array<HookCallbackMatcher>;
  PreCompact?: Array<HookCallbackMatcher>;
  PreToolUse?: Array<HookCallbackMatcher>;
  SessionEnd?: Array<HookCallbackMatcher>;
  SessionStart?: Array<HookCallbackMatcher>;
  Setup?: Array<HookCallbackMatcher>;
  Stop?: Array<HookCallbackMatcher>;
  StopFailure?: Array<HookCallbackMatcher>;
  SubagentStart?: Array<HookCallbackMatcher>;
  SubagentStop?: Array<HookCallbackMatcher>;
  TaskCompleted?: Array<HookCallbackMatcher>;
  TaskCreated?: Array<HookCallbackMatcher>;
  TeammateIdle?: Array<HookCallbackMatcher>;
  UserPromptSubmit?: Array<HookCallbackMatcher>;
  WorktreeCreate?: Array<HookCallbackMatcher>;
  WorktreeRemove?: Array<HookCallbackMatcher>;
} | null;
  json_schema?: unknown;
  prompt_suggestions?: boolean | null;
  sdk_mcp_servers?: Array<string> | null;
  system_prompt?: string | null;
}

/**
 * Response to `ClientRequest::Initialize`.
 * Returned synchronously after the client sends `initialize`; gives the
 * client the full bootstrap context it needs before calling `session/start`.
 */
export interface InitializeResult {
  _cocoRsProtocolVersion?: string;
  _cocoRsVersion?: string;
  account?: SdkAccountInfo;
  agents?: Array<SdkAgentInfo>;
  available_output_styles?: Array<string>;
  commands?: Array<SdkSlashCommand>;
  fast_mode_state?: FastModeState | null;
  models?: Array<SdkModelInfo>;
  output_style: string;
  pid?: number | null;
}

/**
 * Input-side token breakdown.
 *
 * Shape mirrors `vercel_ai_provider::InputTokens` — `total` is the
 * normalized count and equals `no_cache + cache_read + cache_write`
 * when the provider reports every bucket. Provider converters in
 * `services/inference` are responsible for normalizing per-provider
 * raw shapes (Anthropic exclusive-bucket vs OpenAI inclusive-total)
 * before populating this struct, so consumers can rely on `total`
 * being the post-cache-aware true input count.
 *
 * `i64` is used in place of vercel-ai's `Option<u64>` to match the
 * rest of coco-rs's token-count idiom; "not reported" surfaces as `0`.
 */
export interface InputTokens {
  cache_read?: number;
  cache_write?: number;
  no_cache?: number;
  total?: number;
}

/**
 * InstructionsLoaded `load_reason`: `session_start`, `nested_traversal`, `path_glob_match`, `include`, or `compact`.
 */
export type InstructionsLoadReason = "session_start" | "nested_traversal" | "path_glob_match" | "include" | "compact";

/**
 * Input for InstructionsLoaded hooks.
 *
 * Fields: `{file_path, memory_type, load_reason, globs?, trigger_file_path?, parent_file_path?}`.
 */
export interface InstructionsLoadedInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  file_path: string;
  globs?: Array<string> | null;
  load_reason: InstructionsLoadReason;
  memory_type: MemoryType;
  parent_file_path?: string | null;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  trigger_file_path?: string | null;
  hook_event_name: "InstructionsLoaded";
}

/**
 * Payload for [`TurnOutcome::Interrupted`]. `abort_reason`
 * distinguishes user Ctrl+C, submit interrupt, permission abort, and
 * system pre-empt. Never carries a model `stop_reason`.
 */
export interface InterruptedOutcome {
  abort_reason: TurnAbortReason;
}

export type ItemStatus = "in_progress" | "completed" | "failed" | "declined";

/**
 * Error response payload. Mirrors JSON-RPC 2.0 error structure.
 */
export interface JsonRpcError {
  error: JsonRpcErrorObject;
  id: RequestId;
  jsonrpc: string;
}

export interface JsonRpcErrorObject {
  code: number;
  data?: unknown;
  message: string;
}

/**
 * Top-level JSON-RPC 2.0 message.
 */
export type JsonRpcMessage = JsonRpcRequest | JsonRpcResponse | JsonRpcNotification | JsonRpcError;

/**
 * Fire-and-forget notification. In coco-rs this is the primary outbound
 * format for `ServerNotification` events (no `request_id`).
 */
export interface JsonRpcNotification {
  jsonrpc: string;
  method: string;
  params?: unknown;
}

/**
 * A JSON-RPC request wrapper. Holds the method name + params.
 */
export interface JsonRpcRequest {
  id: RequestId;
  jsonrpc: string;
  method: string;
  params?: unknown;
}

/**
 * Successful response payload.
 */
export interface JsonRpcResponse {
  id: RequestId;
  jsonrpc: string;
  result?: unknown;
}

/**
 * Generated file data — either raw bytes/base64 or a URL.
 *
 * Matches the 2-arm `SharedV4FileDataData | SharedV4FileDataUrl` tagged union
 * from the v4 spec.
 */
export type LanguageModelV4FileData = {
  data: string;
  type: "data";
} | {
  type: "url";
  url: string;
};

/**
 * A message in a language model prompt.
 */
export type LanguageModelV4Message = {
  content: Array<UserContentPart>;
  provider_options?: ProviderOptions | null;
  role: "system";
} | {
  content: Array<UserContentPart>;
  provider_options?: ProviderOptions | null;
  role: "developer";
} | {
  content: Array<UserContentPart>;
  provider_options?: ProviderOptions | null;
  role: "user";
} | {
  content: Array<AssistantContentPart>;
  provider_options?: ProviderOptions | null;
  role: "assistant";
} | {
  content: Array<ToolContentPart>;
  provider_options?: ProviderOptions | null;
  role: "tool";
};

/**
 * Matches TS `SDKLocalCommandOutputMessage` (coreSchemas.ts:1590-1602).
 *
 * TS emits this when the user runs a local bash command via the REPL `!`
 * prefix (not a tool call). The `content` field is the command output;
 * TS types it as the raw output structure (typically stdout/stderr).
 */
export interface LocalCommandOutputParams {
  content: unknown;
}

export interface LspPrewarmCompleteParams {
  root: string;
  started: Array<string>;
}

/**
 * Payload for [`TurnOutcome::MaxTurnsReached`]. The variant IS the
 * terminal reason; consumers know "you ran out of turns".
 */
export interface MaxTurnsReachedOutcome {
  max_turns: number;
}

export interface MaxTurnsReachedPayload {
  maxTurns: number;
  turnCount: number;
}

/**
 * MCP server connection state on the wire.
 * Values: `'connected' | 'failed' | 'needs-auth' | 'pending' | 'disabled'`.
 * `Disconnected` is a local extension used when the connection manager
 * has no record of a named server.
 */
export type McpConnectionStatus = "connected" | "pending" | "failed" | "needs-auth" | "disabled" | "disconnected";

/**
 * Params for `mcp/reconnect`.
 */
export interface McpReconnectParams {
  server_name: string;
}

/**
 * Route an MCP JSON-RPC message to an SDK-hosted server.
 * Correlation is via the outer JSON-RPC `request_id` on the envelope —
 * no inner `request_id`. The SDK replies with a `McpRouteMessageResult`
 * payload carrying the forwarded MCP server's JSON-RPC response.
 */
export interface McpRouteMessageParams {
  message: unknown;
  server_name: string;
}

/**
 * Result body for the synchronous JSON-RPC reply to a
 * `mcp/routeMessage` server request.
 * Carries the forwarded JSON-RPC response from the SDK-hosted MCP
 * server verbatim. Correlation is via the outer JSON-RPC
 * `request_id` on the response envelope.
 */
export interface McpRouteMessageResult {
  message: unknown;
}

/**
 * MCP server init entry (inline struct in TS).
 */
export interface McpServerInit {
  name: string;
  status: McpConnectionStatus;
}

export interface McpServerStatus {
  error?: string | null;
  name: string;
  skipped_tools?: Array<McpSkippedToolStatus>;
  status: McpConnectionStatus;
  tombstoned_tools?: Array<string>;
  tool_count?: number;
}

/**
 * Params for `mcp/setServers`.
 */
export interface McpSetServersParams {
  servers: { [key: string]: unknown; };
}

/**
 * Response to `ClientRequest::McpSetServers`.
 */
export interface McpSetServersResult {
  added: Array<string>;
  errors: { [key: string]: string; };
  removed: Array<string>;
}

/**
 * One MCP tool dropped at registration because its wire schema was rejected (v4.2).
 */
export interface McpSkippedToolStatus {
  error: string;
  tool_name: string;
}

export interface McpStartupCompleteParams {
  failed?: Array<string>;
  servers: Array<string>;
}

export interface McpStartupStatusParams {
  server: string;
  status: McpConnectionStatus;
}

/**
 * Response to `ClientRequest::McpStatus`.
 * The `mcpServers` field is camelCase on the wire to match the TS
 * zod schema. Internal Rust uses snake_case for the field name.
 */
export interface McpStatusResult {
  mcpServers: Array<McpServerStatus>;
}

/**
 * Params for `mcp/toggle`.
 */
export interface McpToggleParams {
  enabled: boolean;
  server_name: string;
}

/**
 * One row in the `/memory` file-picker overlay. Built by the slash
 * dispatcher and shipped to the TUI via [`TuiOnlyEvent::OpenMemoryDialog`].
 */
export interface MemoryDialogEntry {
  label: string;
  path: string;
  row_kind?: MemoryDialogRowKind;
  scope: MemoryDialogScope;
}

/**
 * Semantic row kind for the `/memory` picker.
 */
export type MemoryDialogRowKind = {
  exists?: boolean;
  kind: "file";
  read_only?: boolean;
} | {
  enabled?: boolean;
  kind: "folder";
} | {
  enabled?: boolean;
  kind: "toggle";
};

/**
 * Scope tag for a memory file picker entry. Mirrors
 * `coco_commands::MemoryScope` — kept in `coco-types` so the TUI can
 * consume the event without depending on `coco-commands`.
 */
export type MemoryDialogScope = "managed" | "user" | "project" | "project_local" | "project_config" | "subdir" | "imported" | "auto_mem_folder" | "team_mem_folder" | "agent_mem_folder";

/**
 * Scope for agent memory persistence. Controls where MEMORY.md is stored/read.
 */
export type MemoryScope = "user" | "project" | "local";

/**
 * InstructionsLoaded `memory_type`: `User`, `Project`, `Local`, or `Managed`.
 */
export type MemoryType = "User" | "Project" | "Local" | "Managed";

/**
 * One resolved `@`-mention, for the transcript's compact display row.
 */
export type MentionItemKind = "file" | "already_read" | "directory" | "image" | "pdf";

/**
 * A single `@`-mention's display metadata.
 */
export interface MentionSummaryItem {
  count?: number | null;
  display_path: string;
  kind: MentionItemKind;
  truncated?: boolean;
}

/**
 * Display-only payload for [`AttachmentExtras::MentionSummary`].
 */
export interface MentionSummaryPayload {
  items: Array<MentionSummaryItem>;
}

/**
 * Top-level message enum.
 * Tool-use summaries are intentionally **not** a `Message` variant —
 * they're UI-only polish (mobile-row label generated by the Fast
 * model post-turn). The engine emits
 * [`crate::ServerNotification::ToolUseSummary`] for SDK consumers
 * and the TUI side-caches the text without writing it to
 * `MessageHistory`. Keeping UI-only data out of the authoritative
 * transcript upholds I-3 from
 * `docs/coco-rs/engine-tui-unified-transcript-plan.md`.
 */
export type Message = UserMessage | AssistantMessage | SystemMessage | AttachmentMessage | ToolResultMessage | ProgressMessage | TombstoneMessage;

export interface MessageBreakdown {
  assistant_message_tokens: number;
  attachment_tokens: number;
  attachments_by_type?: Array<AttachmentTypeBreakdown>;
  tool_call_tokens: number;
  tool_calls_by_type?: Array<ToolTypeBreakdown>;
  tool_result_tokens: number;
  user_message_tokens: number;
}

/**
 * Which message variant was tombstoned.
 */
export type MessageKind = "user" | "assistant" | "system" | "attachment" | "tool_result" | "progress" | "tombstone";

/**
 * Where a message originated.
 */
export type MessageOrigin = "user_input" | "system_injected" | "tool_result" | "compact_summary" | "subagent_reply" | "slash_command" | "plan_implementation" | "queued_steering";

export interface ModelFallbackParams {
  from_model: string;
  reason: string;
  to_model: string;
}

/**
 * Which purpose a model serves. Multiple roles can map to different models.
 */
export type ModelRole = "main" | "fast" | "explore" | "review" | "memory" | "hook_agent" | "plan" | "subagent";

/**
 * Payload for [`crate::ServerNotification::ModelRoleChanged`]. Carries
 * the resolved binding (model + provider + thinking effort) that the
 * TUI applies to `state.session.model_by_role[role]` and, when
 * `role == Main`, also to `state.session.{model, provider,
 * thinking_effort}` for the status bar.
 *
 * Emitted by `tui_runner` after applying an in-memory override via
 * `SessionRuntime::apply_role_override` / `apply_role_effort`. No
 * persistence to settings.json — that's the user's job.
 */
export interface ModelRoleChangedParams {
  context_window?: number | null;
  effort?: ReasoningEffort | null;
  model_id: string;
  provider: string;
  role: ModelRole;
}

/**
 * A resolved model identity: provider + model ID.
 * Produced by coco-config, consumed by coco-inference.
 */
export interface ModelSpec {
  api: ProviderApi;
  display_name: string;
  model_id: string;
  provider: string;
}

/**
 * Input for Notification hooks.
 *
 * Fields: `{message, title?, notification_type}`.
 */
export interface NotificationInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  message: string;
  notification_type: string;
  permission_mode?: string | null;
  session_id: string;
  title?: string | null;
  transcript_path?: string;
  hook_event_name: "Notification";
}

/**
 * Output-side token breakdown.
 *
 * Shape mirrors `vercel_ai_provider::OutputTokens` — `total` is the
 * total emitted, and `text + reasoning` decompose it when the provider
 * reports the breakdown.
 */
export interface OutputTokens {
  reasoning?: number;
  text?: number;
  total?: number;
}

/**
 * One option in a multi-choice permission dialog.
 * Used by `ToolCheckResult::Ask.choices` and surfaced on the wire via
 * `PermissionDecision::Ask.choices`. The TUI renders one row per
 * choice; the picked `value` is echoed back so the tool's `execute()`
 * can branch on the user's selection.
 */
export interface PermissionAskChoice {
  description?: string | null;
  label: string;
  value: string;
}

/**
 * Permission behavior for a rule.
 */
export type PermissionBehavior = "allow" | "deny" | "ask";

/**
 * Matches TS `SDKPermissionDenialSchema` (coreSchemas.ts:1399-1405).
 */
export interface PermissionDenialInfo {
  tool_input: unknown;
  tool_name: string;
  tool_use_id: string;
}

/**
 * Input for PermissionDenied hooks.
 *
 * Fields: `{tool_name, tool_input, tool_use_id, reason}`.
 */
export interface PermissionDeniedInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  reason: string;
  session_id: string;
  tool_input: unknown;
  tool_name: string;
  tool_use_id: string;
  transcript_path?: string;
  hook_event_name: "PermissionDenied";
}

/**
 * Bounded, UI-ready permission input display.
 *
 * This is separate from the raw tool input because approval UIs should
 * consume sanitized display data while keeping `original_input` only for
 * updated-input response construction and permission-rule derivation.
 */
export type PermissionDisplayInput = {
  kind: "command";
  value: string;
} | {
  kind: "json";
  value: string;
} | {
  kind: "text";
  value: string;
} | {
  kind: "empty";
};

/**
 * Human-readable explanation of a permission decision.
 */
export interface PermissionExplanation {
  explanation: string;
  reasoning: string;
  risk: string;
  risk_level: RiskLevel;
}

/**
 * Permission mode (top-level because it's used by both message and permission modules).
 * Wire format is camelCase. The serde aliases on the drifting variants accept
 * legacy snake_case input so old session transcripts deserialize cleanly.
 */
export type PermissionMode = "default" | "plan" | "bypassPermissions" | "dontAsk" | "acceptEdits" | "auto" | "bubble";

export interface PermissionModeChangedParams {
  bypass_available?: boolean;
  mode: PermissionMode;
}

/**
 * Decision returned by a PermissionRequest hook. Tagged by
 * `behavior`. TS: `permissionRequestDecisionSchema`.
 */
export type PermissionRequestDecision = {
  behavior: "allow";
  updatedInput?: unknown;
} | {
  behavior: "deny";
  interrupt?: boolean | null;
  message?: string | null;
};

/**
 * Tool-specific payload for permission UIs.
 */
export type PermissionRequestDetail = {
  allowed_prompts?: Array<ExitPlanModeAllowedPrompt>;
  kind: "exit_plan_mode";
  outcome: ExitPlanModeOutcome;
  plan?: string | null;
  plan_file_path?: string | null;
};

/**
 * Input for PermissionRequest hooks.
 *
 * Fields: `{tool_name, tool_input, permission_suggestions?}` — note that
 * `tool_use_id` is NOT included on this event.
 */
export interface PermissionRequestInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  permission_suggestions?: unknown;
  session_id: string;
  tool_input: unknown;
  tool_name: string;
  transcript_path?: string;
  hook_event_name: "PermissionRequest";
}

/**
 * A single permission rule.
 */
export interface PermissionRule {
  behavior: PermissionBehavior;
  source: PermissionRuleSource;
  value: PermissionRuleValue;
}

/**
 * Source of a permission rule (ordered by priority: Session is most specific).
 */
export type PermissionRuleSource = "user_settings" | "project_settings" | "local_settings" | "flag_settings" | "policy_settings" | "cli_arg" | "command" | "session";

/**
 * Permission rule value — tool_pattern is a glob/wildcard expression.
 * Examples: "Read", "Bash(git *)", "mcp__slack__*", "*"
 */
export interface PermissionRuleValue {
  rule_content?: string | null;
  tool_pattern: string;
}

/**
 * A permission update action.
 */
export type PermissionUpdate = {
  destination: PermissionUpdateDestination;
  rules: Array<PermissionRule>;
  type: "add_rules";
} | {
  destination: PermissionUpdateDestination;
  rules: Array<PermissionRule>;
  type: "replace_rules";
} | {
  destination: PermissionUpdateDestination;
  rules: Array<PermissionRule>;
  type: "remove_rules";
} | {
  mode: PermissionMode;
  type: "set_mode";
} | {
  destination: PermissionUpdateDestination;
  directories: Array<string>;
  type: "add_directories";
} | {
  destination: PermissionUpdateDestination;
  directories: Array<string>;
  type: "remove_directories";
};

/**
 * Destination for persisting permission updates.
 * Persistable destinations (`User`/`Project`/`LocalSettings`) write to
 * disk; in-memory destinations (`Session`/`CliArg`/`Command`) live only
 * for the running session. same split as
 * `persistPermissionUpdates` in `PermissionUpdate.ts`.
 * `Command` is reserved for rules contributed by an invoked command or
 * skill's frontmatter (`allowed-tools:`).
 * `alwaysAllowRules.command` populated by `SkillTool` /
 * `createGetAppStateWithAllowedTools`.
 */
export type PermissionUpdateDestination = "user_settings" | "project_settings" | "local_settings" | "session" | "cli_arg" | "command";

/**
 * One additional-working-directory row in the Workspace tab.
 */
export interface PermissionsEditorDir {
  path: string;
  source: PermissionRuleSource;
}

/**
 * Payload for [`TuiOnlyEvent::OpenPermissionsEditor`]. Built by the
 * `/permissions` slash handler (CLI side) with everything the tabbed
 * rule editor needs: every file-backed rule keyed by behavior + source,
 * every additional working directory, and the current working directory.
 *
 * The TUI cannot read the settings stores itself, so the CLI snapshots
 * them here and re-emits this payload after each persisted edit (the
 * open overlay refreshes in place).
 * Backing data: `getPermissionRules()` + `getAdditionalWorkingDirectories()`.
 */
export interface PermissionsEditorPayload {
  cwd: string;
  directories: Array<PermissionsEditorDir>;
  managed_only?: boolean;
  rules: Array<PermissionsEditorRule>;
}

/**
 * One rule row in the `/permissions` editor. Carries the structured
 * `(tool_pattern, rule_content)` so the TUI can both render the rule
 * string and reconstruct a `PermissionRuleValue` for removal — display
 * shaping (and i18n) stays TUI-side.
 */
export interface PermissionsEditorRule {
  behavior: PermissionBehavior;
  rule_content?: string | null;
  source: PermissionRuleSource;
  tool_pattern: string;
}

export interface PersistedFileError {
  error: string;
  filename: string;
}

export interface PersistedFileInfo {
  file_id: string;
  filename: string;
}

/**
 * A teammate's plan-approval request, surfaced to the team lead's
 * TUI for approve/deny. Payload byte-matches TS
 * `PlanApprovalRequestSchema` — see `tools/ExitPlanModeTool/`.
 */
export interface PlanApprovalRequestedParams {
  "from": string;
  plan_content: string;
  plan_file_path?: string | null;
  request_id: string;
}

export interface PluginDialogAction {
  label: string;
  plugin_args: string;
}

export interface PluginDialogErrorRow {
  message: string;
  plugin_id: string;
}

export interface PluginDialogInstalledRow {
  actions?: Array<PluginDialogAction>;
  blocked_by_policy: boolean;
  description?: string | null;
  enabled: boolean;
  id: string;
  mcp_servers?: Array<PluginDialogMcpServerRow>;
  name: string;
  options?: Array<PluginDialogOptionRow>;
  path: string;
  source: string;
  version?: string | null;
}

export interface PluginDialogMarketplaceRow {
  actions?: Array<PluginDialogAction>;
  name: string;
  official: boolean;
  plugin_count: number;
  source?: string | null;
}

export interface PluginDialogMcpServerRow {
  actions?: Array<PluginDialogAction>;
  display_name: string;
  enabled: boolean;
  name: string;
  needs_config: boolean;
  tools?: Array<PluginDialogMcpToolRow>;
}

export interface PluginDialogMcpToolRow {
  description?: string | null;
  name: string;
}

export interface PluginDialogOptionRow {
  current_value?: unknown;
  description: string;
  key: string;
  required: boolean;
  title: string;
  value_type: string;
}

/**
 * Payload for [`TuiOnlyEvent::OpenPluginDialog`].
 */
export interface PluginDialogPayload {
  errors: Array<PluginDialogErrorRow>;
  installed: Array<PluginDialogInstalledRow>;
  marketplaces: Array<PluginDialogMarketplaceRow>;
}

/**
 * Plugin init entry (inline struct in TS).
 */
export interface PluginInit {
  name: string;
  path: string;
  source?: string | null;
}

/**
 * Response to `ClientRequest::PluginReload`.
 */
export interface PluginReloadResult {
  agents: Array<string>;
  commands: Array<string>;
  error_count: number;
  plugins: Array<string>;
}

/**
 * Input for PostCompact hooks.
 *
 * Fields: `{trigger: enum('manual','auto'), compact_summary: string}`. Both
 * fields are required.
 */
export interface PostCompactInput {
  agent_id?: string | null;
  agent_type?: string | null;
  compact_summary: string;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  trigger: CompactTrigger;
  hook_event_name: "PostCompact";
}

/**
 * Input for PostToolUseFailure hooks.
 *
 * Fields: `{tool_name, tool_input, tool_use_id, error, is_interrupt?}`.
 */
export interface PostToolUseFailureInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  error: string;
  is_interrupt?: boolean | null;
  permission_mode?: string | null;
  session_id: string;
  tool_input: unknown;
  tool_name: string;
  tool_use_id: string;
  transcript_path?: string;
  hook_event_name: "PostToolUseFailure";
}

/**
 * Input for PostToolUse hooks.
 */
export interface PostToolUseInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  tool_input: unknown;
  tool_name: string;
  tool_response: unknown;
  tool_use_id: string;
  transcript_path?: string;
  hook_event_name: "PostToolUse";
}

/**
 * Input for PreCompact hooks.
 *
 * Fields: `{trigger: enum('manual','auto'), custom_instructions: string | null}`.
 * `custom_instructions` is **nullable, not optional** — the field is
 * always present on the wire, with `null` indicating no instructions.
 */
export interface PreCompactInput {
  agent_id?: string | null;
  agent_type?: string | null;
  custom_instructions?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  trigger: CompactTrigger;
  hook_event_name: "PreCompact";
}

/**
 * Input for PreToolUse hooks.
 */
export interface PreToolUseInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  tool_input: unknown;
  tool_name: string;
  tool_use_id: string;
  transcript_path?: string;
  hook_event_name: "PreToolUse";
}

/**
 * Preserved message segment after partial compaction.
 */
export interface PreservedSegment {
  anchor_uuid: string;
  head_uuid: string;
  tail_uuid: string;
}

export interface ProgressMessage {
  data: unknown;
  parent_message_uuid?: string | null;
  tool_use_id: string;
}

/**
 * Which LLM provider implementation to use.
 * Consumed by coco-config (ProviderInfo) and coco-inference (ProviderFactory).
 */
export type ProviderApi = "anthropic" | "openai" | "gemini" | "volcengine" | "zai" | "openai_compat";

/**
 * Provider-specific metadata attached to responses or stream events.
 *
 * Similar to ProviderOptions but used for data returned from providers.
 */
export type ProviderMetadata = { [key: string]: unknown; };

/**
 * Unresolved provider/model selection from user-facing config.
 * This intentionally does not include [`ProviderApi`]: config surfaces
 * write `provider/model_id`, and the runtime resolves `provider` through
 * the live provider catalog before constructing a runtime slot.
 */
export interface ProviderModelSelection {
  model_id: string;
  provider: string;
}

/**
 * Provider-specific options that can be passed to various API calls.
 *
 * This is a map of provider names to their specific options.
 * For example: `{ "anthropic": { "thinking": { "type": "enabled" } } }`
 */
export type ProviderOptions = { [key: string]: { [key: string]: unknown; }; };

/**
 * Image payload paired with a queued command edit restore.
 */
export interface QueuedCommandEditImage {
  data_base64: string;
  media_type: string;
}

export interface RateLimitParams {
  limit?: number | null;
  provider?: string | null;
  rate_limit_type?: string | null;
  remaining?: number | null;
  reset_at?: number | null;
  status?: RateLimitStatus | null;
  utilization?: number | null;
}

export type RateLimitStatus = "allowed" | "allowed_warning" | "rejected";

/**
 * Reasoning effort. Ordered from "off" through numeric intensity.
 * Provider-agnostic — `thinking_convert` maps to per-provider wire shapes.
 */
export type ReasoningEffort = "minimal" | "low" | "medium" | "high" | "x_high" | "off" | "auto";

/**
 * A reasoning file content part (file data that is part of reasoning).
 *
 * `data` is a 2-arm tagged union:
 * - `Data { data }` — raw bytes or base64-encoded string.
 * - `Url { url }` — a URL pointing to the file.
 */
export interface ReasoningFilePart {
  data: LanguageModelV4FileData;
  mediaType: string;
  providerMetadata?: ProviderMetadata | null;
}

/**
 * Reasoning aggregates anchored to a specific assistant message.
 *
 * Emitted by the engine right after `TurnCompleted` when the model
 * reported non-zero `reasoning_tokens`. The TUI handler indexes its
 * `reasoning_metadata` side-cache by `message_uuid`, eliminating the
 * O(n) "find latest `AssistantThinking` cell" walk and removing the
 * last vestige of the I-2 exception.
 */
export interface ReasoningMetadataAttachedParams {
  duration_ms?: number | null;
  message_uuid: string;
  reasoning_tokens: number;
}

/**
 * A reasoning content part (for thinking models).
 */
export interface ReasoningPart {
  providerMetadata?: ProviderMetadata | null;
  text: string;
}

/**
 * Forward an MCP server's elicitation request to the SDK client.
 * The MCP protocol lets servers ask the user for structured input
 * (form fields with a JSON schema). When the bound MCP transport
 * fires its `elicitation/create` callback, the SDK server bridges
 * it to the connected SDK client via this `ServerRequest`. The
 * client renders a form and returns an [`ElicitationResolveParams`]
 * payload as the synchronous JSON-RPC response.
 */
export interface RequestElicitationParams {
  elicitation: unknown;
  mcp_server_name: string;
  request_id: string;
}

/**
 * Request identifier. Can be a string or integer per JSON-RPC 2.0.
 * SDK clients typically use integers; coco-rs accepts both.
 */
export type RequestId = number | string;

/**
 * Ask the SDK to request user input (free-form or choice-list).
 */
export interface RequestUserInputParams {
  choices?: Array<string>;
  default?: string | null;
  description?: string | null;
  prompt: string;
  request_id: string;
}

export interface RewindCompletedParams {
  messages_removed: number;
  restored_files: number;
  rewound_turn: number;
}

/**
 * Diff stats payload shared by per-row metadata and the selected
 * restore preview event. `file_paths.is_empty()` means "snapshot
 * exists but nothing changed".
 *
 * `file_paths` matches `DiffStats.filesChanged: string[]` and is used by the
 * confirm screen to assemble single / two / many-file labels.
 *
 * # Two semantics, one wire type
 *
 * `insertions` / `deletions` interpretation depends on which event
 * carries this payload:
 *
 * - [`TuiOnlyEvent::RewindRowMetadataReady`] — **forward-time**.
 *   `insertions` = lines added between two adjacent user-message
 *   checkpoints. Computed via [`FileHistoryState::get_diff_stats_between`]
 *   counting `+` lines from `structuredPatch`.
 * - [`TuiOnlyEvent::RewindRestorePreviewReady`] — **rewind-direction**.
 *   `insertions` = lines that rewind would add back; `deletions` =
 *   lines that rewind would remove. Computed via
 *   [`FileHistoryState::get_diff_stats`] with
 *   `diffLines(originalContent, backupContent)` direction.
 *
 * The same `DiffStats` shape covers both call sites;
 * context disambiguates the direction.
 */
export interface RewindDiffStatsPayload {
  deletions: number;
  file_paths: Array<string>;
  insertions: number;
}

/**
 * Params for `control/rewindFiles`.
 */
export interface RewindFilesParams {
  dry_run?: boolean;
  user_message_id: string;
}

/**
 * Response to `ClientRequest::RewindFiles`.
 * Reports which files would be (or were) restored to the snapshot
 * keyed by `user_message_id`, plus a diff summary.
 */
export interface RewindFilesResult {
  deletions?: number;
  dry_run?: boolean;
  files_changed?: Array<string>;
  insertions?: number;
}

/**
 * Per-row metadata for a single rewind picker row.
 * `metadata == None` corresponds to `canRestore = false` (no
 * snapshot — picker shows "⚠ No code restore").
 */
export interface RewindRowMetadata {
  message_id: string;
  metadata?: RewindDiffStatsPayload | null;
}

/**
 * Risk level for permission explanations.
 */
export type RiskLevel = "LOW" | "MEDIUM" | "HIGH";

export interface SandboxStateChangedParams {
  active: boolean;
  enforcement: string;
}

/**
 * Account + auth info for the logged-in user. All fields optional
 * — clients that don't sign in get an empty struct.
 */
export interface SdkAccountInfo {
  apiKeySource?: string | null;
  apiProvider?: ApiProvider | null;
  email?: string | null;
  organization?: string | null;
  subscriptionType?: string | null;
  tokenSource?: string | null;
}

/**
 * SDK-supplied custom subagent spec carried on `InitializeParams.agents`.
 *
 * Wire-level DTO. **Distinct** from the internal [`crate::AgentDefinition`]
 * which is the resolved post-load representation merged from markdown /
 * plugin / SDK sources.
 */
export interface SdkAgentDefinition {
  background?: boolean | null;
  criticalSystemReminder_EXPERIMENTAL?: string | null;
  description: string;
  disallowed_tools?: Array<string> | null;
  effort?: ReasoningEffort | null;
  initial_prompt?: string | null;
  max_turns?: number | null;
  mcp_servers?: Array<AgentMcpServerSpec> | null;
  memory?: MemoryScope | null;
  model?: string | null;
  permission_mode?: PermissionMode | null;
  prompt: string;
  skills?: Array<string> | null;
  tools?: Array<string> | null;
}

/**
 * Available subagent descriptor for `InitializeResult.agents`.
 * Named `SdkAgentInfo` to avoid colliding with `event::AgentInfo`
 * (the payload for the `agents/registered` notification, which has a
 * different schema — `description: Option<String>` without `model`).
 */
export interface SdkAgentInfo {
  description: string;
  model?: string | null;
  name: string;
}

/**
 * SDK hook callback output — a flat object whose `async` field
 * discriminates async-mode.
 */
export interface SdkHookOutput {
  "async"?: boolean | null;
  asyncTimeout?: number | null;
  "continue"?: boolean | null;
  decision?: HookDecision | null;
  hookSpecificOutput?: HookSpecificOutput | null;
  reason?: string | null;
  stopReason?: string | null;
  suppressOutput?: boolean | null;
  systemMessage?: string | null;
}

/**
 * Model capability descriptor for `InitializeResult.models`. The wire uses
 * `value` + camelCase capability keys.
 * Named `SdkModelInfo` to match the existing re-export name at the
 * crate root and to leave breathing room for other model-info shapes
 * (e.g. per-provider config models) elsewhere in the codebase.
 */
export interface SdkModelInfo {
  description: string;
  displayName: string;
  supportedEffortLevels?: Array<EffortLevel>;
  supportsAdaptiveThinking?: boolean | null;
  supportsAutoMode?: boolean | null;
  supportsEffort?: boolean | null;
  supportsFastMode?: boolean | null;
  value: string;
}

/**
 * Minimal session metadata returned by `session/list` and `session/read`.
 */
export interface SdkSessionSummary {
  created_at: string;
  cwd: string;
  message_count?: number;
  model: string;
  session_id: string;
  title?: string | null;
  total_tokens?: number;
  updated_at?: string | null;
}

/**
 * Slash command descriptor for `InitializeResult.commands`.
 * Named `SdkSlashCommand` to avoid colliding with the existing coco-rs
 * `commands` crate notion of a slash command (which has richer
 * internal fields not on the SDK wire).
 */
export interface SdkSlashCommand {
  argumentHint: string;
  description: string;
  name: string;
}

/**
 * Cancel a previously-sent ServerRequest.
 */
export interface ServerCancelRequestParams {
  reason?: string | null;
  request_id: string;
}

/**
 * Params for `session/archive`.
 */
export interface SessionArchiveParams {
  session_id: string;
}

/**
 * Input for SessionEnd hooks.
 */
export interface SessionEndInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  reason: ExitReason;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "SessionEnd";
}

export interface SessionEndedParams {
  reason: string;
}

/**
 * Response to `ClientRequest::SessionList`.
 */
export interface SessionListResult {
  sessions: Array<SdkSessionSummary>;
}

/**
 * Matches TS `ModelUsageSchema` (coreSchemas.ts:17-28).
 */
export interface SessionModelUsage {
  cache_creation_input_tokens: number;
  cache_read_input_tokens: number;
  context_window: number;
  cost_usd: number;
  input_tokens: number;
  max_output_tokens: number;
  output_tokens: number;
  web_search_requests: number;
}

/**
 * Cumulative usage for a single `(provider, model_id)` bucket.
 */
export interface SessionModelUsageEntry {
  cache_creation_cost_usd: number;
  cache_creation_input_tokens: number;
  cache_read_cost_usd: number;
  cache_read_input_tokens: number;
  input_cost_usd: number;
  input_tokens: number;
  model_id: string;
  output_cost_usd: number;
  output_tokens: number;
  priced: boolean;
  provider: string;
  request_count: number;
  total_cost_usd: number;
  unpriced_input_tokens?: number;
  unpriced_output_tokens?: number;
  unpriced_request_count?: number;
  web_search_requests?: number;
}

/**
 * Params for `session/read`.
 */
export interface SessionReadParams {
  cursor?: string | null;
  limit?: number | null;
  session_id: string;
}

/**
 * Response to `ClientRequest::SessionRead`.
 * Phase 2.C.11 returns session metadata only. Message-history
 * retrieval (via the JSONL transcript) is a future enhancement — the
 * `messages` / `next_cursor` / `has_more` fields are reserved for
 * when the transcript reader is wired.
 */
export interface SessionReadResult {
  has_more?: boolean;
  messages?: Array<unknown>;
  next_cursor?: string | null;
  session: SdkSessionSummary;
}

/**
 * Matches TS `SDKResultMessageSchema` (coreSchemas.ts:1407-1451).
 * TS has two subtype variants (success/error) unified here with `is_error` flag.
 */
export interface SessionResultParams {
  duration_api_ms: number;
  duration_ms: number;
  errors?: Array<string>;
  fast_mode_state?: FastModeState | null;
  is_error?: boolean;
  model_usage?: { [key: string]: SessionModelUsage; };
  num_api_calls?: number | null;
  permission_denials?: Array<PermissionDenialInfo>;
  result?: string | null;
  session_id: string;
  stop_reason: string;
  structured_output?: unknown;
  total_cost_usd: number;
  total_turns: number;
  usage: TokenUsage;
}

/**
 * Params for `session/resume`.
 */
export interface SessionResumeParams {
  session_id: string;
}

/**
 * Response to `ClientRequest::SessionResume`.
 * Returned after the server loads a previously-persisted session
 * from disk and installs it as the active session. The SDK client
 * can then issue `turn/start` to continue the conversation.
 */
export interface SessionResumeResult {
  session: SdkSessionSummary;
}

/**
 * Input for SessionStart hooks.
 */
export interface SessionStartInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  model?: string | null;
  permission_mode?: string | null;
  session_id: string;
  source: SessionStartSource;
  transcript_path?: string;
  hook_event_name: "SessionStart";
}

/**
 * Params for `session/start`.
 */
export interface SessionStartParams {
  append_system_prompt?: string | null;
  cwd?: string | null;
  initial_prompt?: string | null;
  max_budget_usd?: number | null;
  max_turns?: number | null;
  model?: string | null;
  permission_mode?: PermissionMode | null;
  system_prompt?: string | null;
}

/**
 * Response to `ClientRequest::SessionStart`.
 * Returned after the server creates an agent session and emits the
 * `session/started` notification. Subsequent ClientRequests
 * (turn/start, approval/resolve, etc.) operate on this session.
 */
export interface SessionStartResult {
  session_id: string;
}

/**
 * SessionStart `source`: `startup | resume | clear | compact`.
 */
export type SessionStartSource = "startup" | "resume" | "clear" | "compact";

/**
 * Matches TS `SDKSystemMessageSchema` with subtype 'init' (coreSchemas.ts:1457-1494).
 * Sent once at session startup; carries the full bootstrap context the SDK
 * consumer needs to render a UI.
 */
export interface SessionStartedParams {
  agents?: Array<string>;
  api_key_source?: string | null;
  betas?: Array<string>;
  cwd: string;
  fast_mode_state?: FastModeState | null;
  lsp_active?: boolean;
  mcp_servers?: Array<McpServerInit>;
  model: string;
  output_style?: string | null;
  permission_mode: string;
  plugins?: Array<PluginInit>;
  protocol_version: string;
  provider?: string;
  session_id: string;
  skills?: Array<string>;
  slash_commands?: Array<string>;
  tools?: Array<string>;
  version: string;
}

export type SessionState = "idle" | "running" | "requires_action";

/**
 * Persisted and protocol-visible cumulative usage for one session.
 */
export interface SessionUsageSnapshot {
  models?: Array<SessionModelUsageEntry>;
  session_id: string;
  totals: SessionUsageTotals;
  unpriced_models?: Array<ProviderModelSelection>;
  updated_at_ms: number;
  version: number;
}

/**
 * Session-level token and cost totals.
 */
export interface SessionUsageTotals {
  cache_creation_cost_usd: number;
  cache_creation_input_tokens: number;
  cache_read_cost_usd: number;
  cache_read_input_tokens: number;
  input_cost_usd: number;
  input_tokens: number;
  output_cost_usd: number;
  output_tokens: number;
  request_count: number;
  total_cost_usd: number;
  unpriced_input_tokens?: number;
  unpriced_output_tokens?: number;
  unpriced_request_count?: number;
  web_search_requests?: number;
}

/**
 * Params for `control/setModel`.
 */
export interface SetModelParams {
  model?: string | null;
}

/**
 * Params for `control/setPermissionMode`.
 *
 * The `ultraplan` field (CCR web-UI refinement flow) is intentionally
 * omitted — see CLAUDE.md "Plan Mode — Skip Ultraplan Only".
 */
export interface SetPermissionModeParams {
  mode: PermissionMode;
}

/**
 * Params for `control/setThinking`.
 * Uses `ThinkingLevel` which includes effort level and per-provider options.
 */
export interface SetThinkingParams {
  thinking_level?: ThinkingLevel | null;
}

/**
 * Input for Setup hooks.
 */
export interface SetupInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  trigger: SetupTrigger;
  hook_event_name: "Setup";
}

/**
 * Setup `trigger`: `init` or `maintenance`.
 */
export type SetupTrigger = "init" | "maintenance";

/**
 * File data as a tagged discriminated union (v4 spec).
 *
 * - `Data` — raw bytes or base64-encoded string.
 * - `Url`  — a URL pointing to the file.
 * - `Reference` — a provider reference (`{ [provider]: id }`).
 * - `Text` — inline text content.
 */
export type SharedV4FileData = {
  data: string;
  type: "data";
} | {
  type: "url";
  url: string;
} | {
  reference: { [key: string]: string; };
  type: "reference";
} | {
  text: string;
  type: "text";
};

/**
 * Typed payload for silent attachment kinds.
 * Variant names map 1:1 to the [`AttachmentKind`](crate::AttachmentKind)
 * silent variants. Adding a new silent kind requires adding a matching
 * variant here — enforced by the constructor helpers on
 * [`AttachmentMessage`](super::AttachmentMessage) +
 * the `silent_kind_round_trips_through_payload` parity test.
 */
export type SilentPayload = HookCancelledPayload | HookErrorDuringExecutionPayload | HookNonBlockingErrorPayload | HookSystemMessagePayload | HookPermissionDecisionPayload | CommandPermissionsPayload | GoalStatusPayload | StructuredOutputPayload | DynamicSkillPayload | MaxTurnsReachedPayload | AlreadyReadFilePayload | EditedImageFilePayload;

export interface SkillDiscoveryPayload {
  signal: string;
  skills: Array<SkillDiscoverySkill>;
  source: SkillDiscoverySource;
}

export interface SkillDiscoverySkill {
  description: string;
  name: string;
  shortId?: string | null;
}

export type SkillDiscoverySource = "native" | "aki" | "both";

/**
 * A non-overridable lock on a skill row in the `/skills` dialog.
 * Carries both the originating tier ([`Self::source`]) and the
 * forced 4-state value ([`Self::forced_value`]) so downstream
 * renderers don't need to re-derive the value from per-tier maps.
 *
 * TS mirror: `oT5` returns `{ value, source }` —
 * `cli_inner_pretty.js:476885-476893`.
 */
export interface SkillLock {
  forced_value: SkillOverrideState;
  source: SkillLockSource;
}

/**
 * Which precedence layer originated a non-overridable lock on a
 * skill's `skill_overrides` state. Mirrors the four `lock.source`
 * values returned by TS `oT5` (`cli_inner_pretty.js:476885-476893`).
 *
 * In precedence order (highest first): [`Self::Policy`] →
 * [`Self::Flag`] → [`Self::Author`] → [`Self::Plugin`]. A lock means
 * the `/skills` dialog renders `🔒 <label>` for the row and refuses
 * to cycle it (Space is a no-op).
 */
export type SkillLockSource = "policy" | "flag" | "author" | "plugin";

/**
 * Per-skill override state stored under `skill_overrides` in any
 * settings tier. Drives the `/skills` 4-state editor ladder.
 *
 * Wire format is kebab-case (`"on"`, `"name-only"`,
 * `"user-invocable-only"`, `"off"`) — JSON settings files round-trip
 * without translation.
 */
export type SkillOverrideState = "on" | "name-only" | "user-invocable-only" | "off";

/**
 * Categorical save-failure source. Mirrors the
 * [`coco_config::SettingsWriteError`] variants + a runtime tier
 * for "settings hot-reload was disabled at session start so we
 * can't republish".
 */
export type SkillOverridesSaveErrorKind = "io" | "parse" | "rebuild" | "no_publisher";

/**
 * Outcome of a `/skills` dialog save dispatch. CLI bridge populates
 * this after `SettingsWriter::write_local`.
 *
 * Carries **no display data** — TUI owns toast text generation and
 * stashes the pre-write `total_edits` count on its own state
 * before dispatching. CLI only reports success vs typed failure.
 */
export type SkillOverridesSaveResult = {
  outcome: "ok";
} | {
  kind: SkillOverridesSaveErrorKind;
  message: string;
  outcome: "err";
};

/**
 * One row in the `/skills` dialog. Every field is required so the dialog
 * never has to fabricate defaults for `baseline` / `lock` / etc.
 */
export interface SkillsDialogEntry {
  baseline: SkillOverrideState;
  current_local?: SkillOverrideState | null;
  description: string;
  frontmatter_bytes: number;
  lock?: SkillLock | null;
  name: string;
  plugin_name?: string | null;
  source: SkillsDialogSource;
}

/**
 * Payload for [`TuiOnlyEvent::OpenSkillsDialog`]. Built once by the
 * `/skills` slash handler so the TUI doesn't recompute paths, token
 * estimates, or grouping.
 *
 * A flat editable list with 4-state override cycling, source labels
 * inline, and lock annotations for policy/flag/author/plugin-locked rows.
 */
export interface SkillsDialogPayload {
  bytes_per_token: number;
  entries: Array<SkillsDialogEntry>;
}

/**
 * Source group for a skill dialog entry. Collapsed from
 * `SettingSource | 'plugin' | 'mcp'` to a closed enum so the wire shape is statically typed.
 *
 * The dialog normalises `bundled`/`builtin` → display label `"built-in"`; the
 * dialog filter matches against that lowercased label.
 */
export type SkillsDialogSource = "built_in" | "project" | "user" | "policy" | "plugin" | "mcp";

/**
 * UI-facing projection of a slash command. The TUI receives a `Vec` of
 * these at startup (and again after `/reload-plugins`) so the
 * autocomplete popup and command palette can render and rank without
 * reaching into [`CommandBase`] every time.
 * Lives in `coco-types` (rather than `coco-tui`) so it can travel on a
 * [`crate::TuiOnlyEvent`] variant — events are the only path between
 * the agent driver and the TUI, and event payload types must be
 * foundation-layer.
 */
export interface SlashCommandInfo {
  aliases?: Array<string>;
  argument_hint?: string | null;
  argument_kind?: CommandArgumentKind;
  description?: string | null;
  kind?: CommandTypeTag;
  name: string;
  source?: CommandSource | null;
  usage_score?: number;
}

/**
 * Categorization of a `SlashCommandStatus` payload. Each variant maps to
 * a `slash.status.*` key in the TUI locale catalog.
 *
 * Wire format intentionally tagged so SDK clients can render their own
 * localized strings instead of consuming the TUI's English fallback.
 */
export type SlashCommandStatusKind = {
  kind: "no_handler";
} | {
  error: string;
  kind: "failed";
} | {
  kind: "empty_prompt";
} | {
  dialog_kind: string;
  kind: "dialog_pending";
} | {
  kind: "permissions_usage_allow";
} | {
  kind: "permissions_usage_deny";
};

/**
 * A source reference content part (for citations).
 */
export interface SourcePart {
  filename?: string | null;
  id: string;
  mediaType?: string | null;
  providerMetadata?: ProviderMetadata | null;
  sourceType: SourceType;
  title?: string | null;
  url?: string | null;
}

/**
 * Types of sources.
 */
export type SourceType = "url" | "document";

/**
 * Input for StopFailure hooks.
 */
export interface StopFailureInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  error: string;
  error_details?: string | null;
  last_assistant_message?: string | null;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "StopFailure";
}

/**
 * Input for Stop hooks.
 *
 * Fields: `{stop_hook_active, last_assistant_message?}`.
 */
export interface StopInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  last_assistant_message?: string | null;
  permission_mode?: string | null;
  session_id: string;
  stop_hook_active: boolean;
  transcript_path?: string;
  hook_event_name: "Stop";
}

/**
 * Params for `control/stopTask`.
 */
export interface StopTaskParams {
  task_id: string;
}

/**
 * `structured_output` attachment type.
 */
export interface StructuredOutputPayload {
  data: unknown;
}

/**
 * Input for SubagentStart hooks.
 */
export interface SubagentStartInput {
  agent_id: string;
  agent_type: string;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "SubagentStart";
}

/**
 * Input for SubagentStop hooks.
 *
 * Fields: `{stop_hook_active, agent_id, agent_transcript_path, agent_type, last_assistant_message?}`.
 */
export interface SubagentStopInput {
  agent_id: string;
  agent_transcript_path: string;
  agent_type: string;
  cwd: string;
  last_assistant_message?: string | null;
  permission_mode?: string | null;
  session_id: string;
  stop_hook_active: boolean;
  transcript_path?: string;
  hook_event_name: "SubagentStop";
}

/**
 * Severity of a context suggestion. `Warning` sorts before `Info`.
 */
export type SuggestionSeverity = "warning" | "info";

export interface SummarizeCompletedParams {
  from_turn: number;
  summary_tokens: number;
}

export interface SystemAgentsKilledMessage {
  count: number;
  uuid: string;
}

export interface SystemApiErrorMessage {
  error: string;
  status_code?: number | null;
  uuid: string;
}

export interface SystemApiMetricsMessage {
  cost_usd?: number | null;
  model: string;
  usage: TokenUsage;
  uuid: string;
}

export interface SystemAwaySummaryMessage {
  summary: string;
  uuid: string;
}

export interface SystemBridgeStatusMessage {
  connected: boolean;
  message?: string | null;
  uuid: string;
}

export interface SystemCompactBoundaryMessage {
  messages_summarized?: number | null;
  pre_compact_discovered_tools?: Array<string>;
  preserved_segment?: PreservedSegment | null;
  tokens_after: number;
  tokens_before: number;
  trigger?: CompactTrigger;
  user_context?: string | null;
  uuid: string;
}

/**
 * Payload for [`SystemMessage::ContextUsage`] — a `/context` snapshot.
 * Holds the runtime-analyzed report verbatim; the renderer derives the
 * colored grid, legend, and grouped Memory/MCP/Agents/Skills sections
 * from it. Persisted to the transcript like any other system row, so a
 * resumed session re-paints the historical snapshot.
 */
export interface SystemContextUsageMessage {
  result: ContextUsageResult;
  uuid: string;
}

export interface SystemInformationalMessage {
  level: SystemMessageLevel;
  message: string;
  title: string;
  uuid: string;
}

export interface SystemLocalCommandMessage {
  command: string;
  output: string;
  uuid: string;
}

/**
 * Surfaced to the user's transcript when the auto-memory subsystem
 * has just landed new memory writes.
 * (which overrides
 * `verb` to `"Improved"` for dream consolidations vs. the default
 * `"Saved"` for extract).
 */
export interface SystemMemorySavedMessage {
  uuid: string;
  verb?: string;
  written_paths?: Array<string>;
}

/**
 * System messages have sub-types for different notification kinds.
 * All system messages are `role: "user"` with `is_meta: true` for the API.
 * Wire discriminator is `kind`, matching the rest of the coco-rs
 * closed-set enums (`PermissionRule`, `ContentReplacementRecord`,
 * `event::*`). The semantic taxonomy (compact_boundary,
 * microcompact_boundary, …) values, but we
 * don't mirror the field name — see `coco-session::storage` doc for
 * the snake_case wire policy.
 */
export type SystemMessage = SystemInformationalMessage | SystemApiErrorMessage | SystemCompactBoundaryMessage | SystemMicrocompactBoundaryMessage | SystemLocalCommandMessage | SystemPermissionRetryMessage | SystemBridgeStatusMessage | SystemMemorySavedMessage | SystemAwaySummaryMessage | SystemAgentsKilledMessage | SystemApiMetricsMessage | SystemStopHookSummaryMessage | SystemTurnDurationMessage | SystemScheduledTaskFireMessage | SystemContextUsageMessage | SystemUserInterruptionMessage;

export type SystemMessageLevel = "info" | "warning" | "error";

/**
 * Micro-compaction boundary marker.
 * Reserved for future use. coco-rs aligns with TS: micro-compaction
 * (client-side tool-result clearing + time-based MC) does NOT emit a
 * user-visible boundary message — it broadcasts a `ContextCompacted`
 * event with `trigger=TimeBased`/`Auto` instead, and the TUI renders
 * a transient toast (`toast.micro_compaction`) rather than persisting
 * a boundary in the transcript. The variant stays defined so wire
 * transcripts that include one (e.g. cached-microcompact builds) can
 * be parsed without errors.
 */
export interface SystemMicrocompactBoundaryMessage {
  uuid: string;
}

export interface SystemPermissionRetryMessage {
  message: string;
  tool_name: string;
  uuid: string;
}

export interface SystemScheduledTaskFireMessage {
  schedule: string;
  task_id: string;
  uuid: string;
}

export interface SystemStopHookSummaryMessage {
  hook_name: string;
  outcome: string;
  uuid: string;
}

export interface SystemTurnDurationMessage {
  duration_ms: number;
  uuid: string;
}

/**
 * Authoritative cancel marker. `for_tool_use` is computed by the engine
 * cancel finalizer (which sees the in-flight tool-call state) and stored
 * here; every other consumer reads the field. This eliminates the
 * engine ↔ TUI recomputation race that the legacy text-based interrupt
 * markers exhibited.
 */
export interface SystemUserInterruptionMessage {
  for_tool_use: boolean;
  uuid: string;
}

/**
 * Per-tool-invocation activity row for the in-process teammate
 * spinner tree.
 * Carries the tool name and an optional one-line summary; the
 * caller decides how to render. Designed to ride in
 * [`TaskProgress::recent_activities`] as a small ring buffer.
 */
export interface TaskActivity {
  summary?: string | null;
  tool_name: string;
}

/**
 * Input for TaskCompleted hooks.
 *
 * Fields: `{task_id, task_subject, task_description?, teammate_name?, team_name?}`.
 */
export interface TaskCompletedInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  task_description?: string | null;
  task_id: string;
  task_subject: string;
  team_name?: string | null;
  teammate_name?: string | null;
  transcript_path?: string;
  hook_event_name: "TaskCompleted";
}

/**
 * SDK task completion notification. `Killed` tasks are surfaced to SDK
 * consumers as `status: "stopped"` plus `killed_by` attribution.
 */
export interface TaskCompletedParams {
  killed_by?: TaskKilledBy | null;
  output_file: string;
  status: TaskCompletionStatus;
  summary: string;
  task_id: string;
  tool_use_id?: string | null;
  usage?: TaskUsage | null;
}

/**
 * Matches TS `z.enum(['completed', 'failed', 'stopped'])` for task_notification status.
 */
export type TaskCompletionStatus = "completed" | "failed" | "stopped";

/**
 * Input for TaskCreated hooks.
 *
 * Fields: `{task_id, task_subject, task_description?, teammate_name?, team_name?}`.
 */
export interface TaskCreatedInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  task_description?: string | null;
  task_id: string;
  task_subject: string;
  team_name?: string | null;
  teammate_name?: string | null;
  transcript_path?: string;
  hook_event_name: "TaskCreated";
}

/**
 * Actor that caused a running task to be stopped.
 */
export type TaskKilledBy = "user" | "parent" | "system";

/**
 * Task status wire format. **Distinct** from [`crate::TaskStatus`],
 * which is the 6-variant running-task lifecycle enum.
 */
export type TaskListStatus = "pending" | "in_progress" | "completed";

/**
 * Snapshot of the task panel state — tools emit this post-mutation
 * so the TUI can redraw without reaching into `ToolAppState` directly.
 */
export interface TaskPanelChangedParams {
  expanded_view: ExpandedView;
  plan_tasks: Array<TaskRecord>;
  todos_by_agent?: { [key: string]: Array<TodoRecord>; };
  verification_nudge_pending: boolean;
}

/**
 * Matches TS `SDKTaskProgressMessage` (coreSchemas.ts:1750-1767).
 * In TS, `description` and `usage` are required; other fields optional.
 * The `workflow_progress` field carries the streaming state of local_workflow
 * tasks — a delta batch of workflow state changes.
 */
export interface TaskProgressParams {
  agent_type?: string | null;
  description: string;
  last_tool_name?: string | null;
  recent_activities?: Array<TaskActivity>;
  summary?: string | null;
  task_id: string;
  tool_use_id?: string | null;
  usage: TaskUsage;
  workflow_progress?: Array<WorkflowProgressEvent>;
}

/**
 * A durable plan-item.
 */
export interface TaskRecord {
  activeForm?: string | null;
  blockedBy?: Array<string>;
  blocks?: Array<string>;
  description?: string;
  id: string;
  metadata?: { [key: string]: unknown; } | null;
  owner?: string | null;
  status: TaskListStatus;
  subject: string;
}

/**
 * Matches TS `SDKTaskStartedMessage` (`coreSchemas.ts:1715-1733`)
 * plus optional teammate-metadata fields used when
 * `task_type == "in_process_teammate"`.
 *
 * TS encodes both teammate and async-subagent spawn through this same
 * `task_started` SDK event, discriminated by `task_type` — the canonical
 * strings live in `Task.ts:6-13`: `"local_bash"`, `"local_agent"`,
 * `"remote_agent"`, `"in_process_teammate"`, `"local_workflow"`,
 * `"monitor_mcp"`, `"dream"`. The teammate-roster rich metadata that TS
 * stores in `AppState.teamContext.teammates` rides along as the
 * optional fields below so the TUI in coco-rs (no shared store across
 * processes) can construct the same `SubagentInstance { kind:
 * Teammate, ... }` projection on the wire alone.
 */
export interface TaskStartedParams {
  agent_name?: string | null;
  backend_kind?: string | null;
  color?: string | null;
  description: string;
  prompt?: string | null;
  task_id: string;
  task_type?: string | null;
  team_name?: string | null;
  tool_use_id?: string | null;
  workflow_name?: string | null;
}

export interface TaskUsage {
  cache_read_tokens?: number;
  cost_usd?: number;
  duration_ms: number;
  input_tokens?: number;
  output_tokens?: number;
  tool_uses: number;
  total_tokens: number;
}

/**
 * Input for TeammateIdle hooks.
 *
 * Fields: `{teammate_name, team_name}`.
 */
export interface TeammateIdleInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  team_name: string;
  teammate_name: string;
  transcript_path?: string;
  hook_event_name: "TeammateIdle";
}

/**
 * A text content part.
 */
export interface TextPart {
  providerMetadata?: ProviderMetadata | null;
  text: string;
}

/**
 * Unified thinking configuration for all providers.
 *
 * `effort` carries provider-agnostic intent — `Disable`, `Auto`, or
 * one of the numeric levels (`Minimal`..`XHigh`). Provider-specific
 * wire toggles (e.g. DeepSeek's `{"thinking":{"type":"enabled"}}`)
 * flow through `options` verbatim.
 *
 * Semantic states:
 *   * `Disable` — explicit "thinking off"; emit explicit-off signals
 *     where the provider supports them, otherwise omit reasoning fields.
 *   * `Auto`    — "let the provider decide"; omit reasoning fields
 *     so the server-side default applies.
 *   * `Minimal`..`XHigh` — explicit numeric efforts; emitted via the
 *     provider's typed reasoning channel.
 */
export interface ThinkingLevel {
  budget_tokens?: number | null;
  effort: ReasoningEffort;
  options?: { [key: string]: unknown; };
}

/**
 * Semantic representation of a conversation thread item.
 * Produced by `StreamAccumulator` from `AgentStreamEvent` sequences.
 * Used in `ServerNotification::ItemStarted / ItemUpdated / ItemCompleted`.
 *
 * See `event-system-design.md` Section 1.6.
 */
export interface ThreadItem {
  details: ThreadItemDetails;
  item_id: string;
  turn_id: string;
}

/**
 * Tool-specific semantic mapping.
 *
 * Mapping rules (from `event-system-design.md` Section 6.2):
 * - Bash → `CommandExecution`
 * - Edit/Write → `FileChange`
 * - WebSearch → `WebSearch`
 * - mcp__* → `McpToolCall`
 * - Agent/Task → `Subagent`
 * - all others → `ToolCall`
 * - text content → `AgentMessage`
 * - thinking → `Reasoning`
 * - errors → `Error`
 */
export type ThreadItemDetails = {
  command: string;
  exit_code?: number | null;
  output: string;
  status: ItemStatus;
  type: "command_execution";
} | {
  changes: Array<FileChangeInfo>;
  status: ItemStatus;
  type: "file_change";
} | {
  query: string;
  status: ItemStatus;
  type: "web_search";
} | {
  arguments: unknown;
  error?: string | null;
  result?: string | null;
  server: string;
  status: ItemStatus;
  tool: string;
  type: "mcp_tool_call";
} | {
  agent_id: string;
  agent_type: string;
  description: string;
  is_background?: boolean;
  result?: string | null;
  status: ItemStatus;
  type: "subagent";
} | {
  input: unknown;
  is_error?: boolean;
  output?: string | null;
  status: ItemStatus;
  tool: string;
  type: "tool_call";
} | {
  text: string;
  type: "agent_message";
} | {
  text: string;
  type: "reasoning";
} | {
  message: string;
  type: "error";
};

/**
 * A TodoWrite item (no `id` field, positional identity).
 */
export interface TodoRecord {
  activeForm: string;
  content: string;
  status: "pending" | "in_progress" | "completed";
}

/**
 * Per-request token counts (returned by LLM API).
 *
 * Shape mirrors `vercel_ai_provider::Usage` — nested input/output
 * breakdown with named cache buckets, so `usage.input_tokens.total`
 * is unambiguously the normalized count and the implicit "is this
 * the no-cache subset or the inclusive total?" contract that the
 * previous flat shape carried is gone.
 */
export interface TokenUsage {
  input_tokens?: InputTokens;
  output_tokens?: OutputTokens;
}

export interface TombstoneMessage {
  original_kind: MessageKind;
  uuid: string;
}

/**
 * Structured reason for a tool abort notification.
 */
export type ToolAbortReasonPayload = {
  kind: "turn";
  reason: TurnAbortReason;
} | {
  kind: "self_abort";
  message: string;
} | {
  failed_tool: string;
  kind: "sibling_error";
};

/**
 * A tool approval request content part (for provider-executed tools).
 *
 * This is used for flows where the provider executes the tool (e.g. MCP tools)
 * but requires an explicit user approval before continuing.
 */
export interface ToolApprovalRequestPart {
  approvalId: string;
  context?: string | null;
  providerMetadata?: ProviderMetadata | null;
  toolCallId: string;
  toolName?: string | null;
}

/**
 * A tool approval response part (for tools that require approval).
 *
 * This contains the user's decision to approve or deny a provider-executed tool call.
 */
export interface ToolApprovalResponsePart {
  approvalId: string;
  approved: boolean;
  providerMetadata?: ProviderMetadata | null;
  reason?: string | null;
}

/**
 * A tool call content part.
 *
 * `input` always carries the model's best-known emission — even when the
 * call is `invalid`. Adapters that fail JSON parsing fall back to
 * `JSONValue::Object({})` (so schema validation can report
 * specific missing fields; mirrors TS `parsed ?? {}` in
 * `utils/messages.ts:2694`). Adapters that detect a truly unrecoverable
 * `Value::String` payload preserve the raw bytes inside `input` so the
 * agent loop can surface the original emission in diagnostics.
 */
export interface ToolCallPart {
  input: unknown;
  invalid?: boolean;
  invalidReason?: ToolInputInvalidReason | null;
  providerExecuted?: boolean | null;
  providerMetadata?: ProviderMetadata | null;
  toolCallId: string;
  toolName: string;
}

/**
 * Tool message content parts.
 */
export type ToolContentPart = ToolResultPart | ToolApprovalResponsePart;

/**
 * UI-only side channel for bounded display data produced by tools.
 * This data is for transcript/rendering surfaces only. Provider history and
 * model-visible tool output must continue to use `ToolResultMessage.message`.
 */
export type ToolDisplayData = {
  data: ApplyPatchPreview;
  kind: "apply_patch_preview";
} | {
  data: AskUserQuestionResult;
  kind: "ask_user_question_result";
} | {
  data: ExitPlanModeResult;
  kind: "exit_plan_mode_result";
};

/**
 * Structured cause for an invalid tool call. Drives the wrap prefix
 * chosen by `app/query`'s tool result synthesizer:
 * - [`Self::JsonParseFailed`] → `<tool_use_error>JSON parse failed: …</tool_use_error>`
 * - [`Self::SchemaViolation`] → `<tool_use_error>InputValidationError: …</tool_use_error>`
 * - [`Self::NoSuchTool`] → `<tool_use_error>No such tool available: …</tool_use_error>`
 *
 * Structured cause for an invalid tool call — three failure modes.
 */
export type ToolInputInvalidReason = {
  error: string;
  kind: "json_parse_failed";
  raw: string;
} | {
  kind: "schema_violation";
  message: string;
} | {
  kind: "no_such_tool";
  tool_name: string;
};

/**
 * Matches TS `SDKToolProgressMessage` (coreSchemas.ts:1648-1659).
 *
 * Long-running tool progress (Bash, PowerShell). TS throttles emission to
 * ≤1 per 30 seconds per `parent_tool_use_id`. coco-rs StreamAccumulator
 * may emit this independently from `AgentStreamEvent::ToolUseStarted`.
 */
export interface ToolProgressParams {
  elapsed_time_seconds: number;
  parent_tool_use_id?: string | null;
  task_id?: string | null;
  tool_name: string;
  tool_use_id: string;
}

/**
 * Content of a tool result.
 *
 * This matches the LanguageModelV4ToolResultOutput type from the v4 spec.
 */
export type ToolResultContent = {
  providerOptions?: ProviderOptions | null;
  type: "text";
  value: string;
} | {
  providerOptions?: ProviderOptions | null;
  type: "json";
  value: unknown;
} | {
  providerOptions?: ProviderOptions | null;
  reason?: string | null;
  type: "execution-denied";
} | {
  providerOptions?: ProviderOptions | null;
  type: "error-text";
  value: string;
} | {
  providerOptions?: ProviderOptions | null;
  type: "error-json";
  value: unknown;
} | {
  providerOptions?: ProviderOptions | null;
  type: "content";
  value: Array<ToolResultContentPart>;
};

/**
 * A part of tool result content.
 *
 * Matches the `content` array items in `LanguageModelV4ToolResultOutput`
 * from the v4 spec — TS source has 5 variants: `text`, `file-data`,
 * `file-url`, `file-reference`, `custom`. Image / non-image are
 * distinguished by `media_type` (image/png vs application/pdf etc.),
 * not by separate variants.
 */
export type ToolResultContentPart = {
  providerOptions?: ProviderOptions | null;
  text: string;
  type: "text";
} | {
  data: string;
  filename?: string | null;
  mediaType: string;
  providerOptions?: ProviderOptions | null;
  type: "file-data";
} | {
  mediaType: string;
  providerOptions?: ProviderOptions | null;
  type: "file-url";
  url: string;
} | {
  providerOptions?: ProviderOptions | null;
  providerReference: { [key: string]: string; };
  type: "file-reference";
} | {
  providerOptions?: ProviderOptions | null;
  type: "custom";
};

export interface ToolResultMessage {
  display_data?: ToolDisplayData | null;
  is_error?: boolean;
  message: LanguageModelV4Message;
  source_assistant_uuid?: string | null;
  tool_id: string;
  tool_use_id: string;
  uuid: string;
}

/**
 * A tool result content part.
 */
export interface ToolResultPart {
  isError?: boolean;
  output: ToolResultContent;
  providerMetadata?: ProviderMetadata | null;
  toolCallId: string;
  toolName: string;
}

export interface ToolTypeBreakdown {
  call_tokens: number;
  name: string;
  result_tokens: number;
}

/**
 * Matches TS `SDKToolUseSummaryMessage` (coreSchemas.ts:1769-1777).
 *
 * Background Haiku-based summary of a batch of tool uses. TS uses this
 * to compress verbose tool output before it's displayed or archived.
 */
export interface ToolUseSummaryParams {
  preceding_tool_use_ids: Array<string>;
  summary: string;
}

/**
 * TUI-exclusive events.
 *
 * These events are dropped by SDK and App-Server consumers. They drive
 * overlays, toasts, and UI-only state transitions that are not part of the
 * protocol contract.
 *
 * Per `event-system-design.md` Section 1.7, the design listed this type as
 * owned by `coco-tui`. Since `CoreEvent::Tui(TuiOnlyEvent)` is part of the
 * envelope enum defined in `coco-types`, the type itself must live in
 * `coco-types` to avoid a cyclic dependency. The semantic contract
 * (TUI-only, never sent to SDK) is preserved via consumer dispatch rules
 * in `StreamAccumulator` and `handle_core_event()`.
 *
 * 23 variants (20 from design §4.1 + 3 local extensions).
 */
export type TuiOnlyEvent = {
  choices?: Array<PermissionAskChoice> | null;
  cwd?: string | null;
  description: string;
  detail?: PermissionRequestDetail | null;
  display_input: PermissionDisplayInput;
  original_input?: unknown;
  permission_suggestions?: Array<PermissionUpdate>;
  request_id: string;
  show_always_allow?: boolean;
  tool_name: string;
  type: "approval_required";
  worker_badge?: WorkerBadge | null;
} | {
  input: unknown;
  request_id: string;
  type: "question_asked";
} | {
  request_id: string;
  schema: unknown;
  server: string;
  type: "elicitation_requested";
} | {
  operation: string;
  request_id: string;
  type: "sandbox_approval_required";
} | {
  explanation?: PermissionExplanation | null;
  request_id: string;
  type: "permission_explanation_ready";
} | {
  plugins: Array<unknown>;
  type: "plugin_data_ready";
} | {
  styles: Array<string>;
  type: "output_styles_ready";
} | {
  commands: Array<SlashCommandInfo>;
  type: "available_commands_refreshed";
} | {
  id: string;
  images?: Array<QueuedCommandEditImage>;
  prompt: string;
  type: "queued_command_edit_ready";
} | {
  cursor: number;
  ids: Array<string>;
  images?: Array<QueuedCommandEditImage>;
  prompt: string;
  type: "queued_commands_edit_ready";
} | {
  id: string;
  reason: string;
  type: "queued_command_edit_unavailable";
} | {
  sessions: Array<SdkSessionSummary>;
  type: "open_session_browser";
} | {
  rows: Array<RewindRowMetadata>;
  type: "rewind_row_metadata_ready";
} | {
  message_id: string;
  stats?: RewindDiffStatsPayload | null;
  type: "rewind_restore_preview_ready";
} | {
  messages: Array<Message>;
  type: "rewind_pre_clear_snapshot";
} | {
  failures: number;
  type: "compaction_circuit_breaker_open";
} | {
  removed: number;
  type: "micro_compaction_applied";
} | {
  summary_tokens: number;
  type: "session_memory_compact_applied";
} | {
  reason: string;
  type: "speculative_rolled_back";
} | {
  type: "session_memory_extraction_started";
} | {
  extracted: number;
  type: "session_memory_extraction_completed";
} | {
  error: string;
  type: "session_memory_extraction_failed";
} | {
  job_id: string;
  reason: string;
  type: "cron_job_disabled";
} | {
  count: number;
  type: "cron_jobs_missed";
} | {
  call_id: string;
  name: string;
  type: "tool_call_stream_start";
} | {
  call_id: string;
  delta: string;
  type: "tool_call_delta";
} | {
  data: unknown;
  tool_use_id: string;
  type: "tool_progress";
} | {
  interruptible: boolean;
  type: "tool_interruptibility_changed";
} | {
  reason: ToolAbortReasonPayload;
  tool_use_id: string;
  type: "tool_execution_aborted";
} | {
  files_changed: number;
  target_message_id: string;
  type: "rewind_completed";
} | {
  args: string;
  name: string;
  text: string;
  type: "slash_command_result";
} | {
  body: string;
  title: string;
  type: "open_goal_status";
} | {
  result: ContextUsageResult;
  type: "open_context_usage";
} | {
  args: string;
  kind: SlashCommandStatusKind;
  name: string;
  type: "slash_command_status";
} | {
  type: "open_rewind_picker";
} | {
  entries: Array<MemoryDialogEntry>;
  type: "open_memory_dialog";
} | {
  payload: WorkflowDialogPayload;
  type: "open_workflow_picker";
} | {
  args: string;
  type: "copy_command_requested";
} | {
  path: string;
  type: "memory_file_opened";
} | {
  error: string;
  path: string;
  type: "memory_file_open_failed";
} | {
  path: string;
  type: "plan_file_opened";
} | {
  error: string;
  path: string;
  type: "plan_file_open_failed";
} | {
  request_id: string;
  type: "external_editor_prepare";
} | {
  content: string;
  modified: boolean;
  type: "prompt_editor_completed";
} | {
  error: string;
  type: "prompt_editor_failed";
} | {
  exit_code: number;
  output: string;
  type: "bash_command_completed";
  user_message_id: string;
} | {
  type: "open_model_picker";
} | {
  type: "open_settings";
} | {
  type: "open_theme_picker";
} | {
  payload: SkillsDialogPayload;
  type: "open_skills_dialog";
} | {
  payload: PluginDialogPayload;
  type: "open_plugin_dialog";
} | {
  payload: AgentsDialogPayload;
  type: "open_agents_dialog";
} | {
  payload: PermissionsEditorPayload;
  type: "open_permissions_editor";
} | {
  type: "open_add_directory";
} | {
  type: "open_export";
} | {
  result: SkillOverridesSaveResult;
  type: "skill_overrides_saved";
};

/**
 * Why a turn was aborted. Lets consumers distinguish user cancel,
 * submit interrupt, permission abort, and system pre-emption.
 */
export type TurnAbortReason = "user_cancel" | "submit_interrupt" | "system_preempt" | "permission_abort" | "background";

/**
 * Terminal event for one logical user-prompt cycle. The
 * `outcome` discriminator carries the per-variant payload —
 * `stop_reason` lives only on `Completed` where it is
 * semantically meaningful. Other variants self-describe.
 *
 * Pairing contract: every `TurnStartedParams` is followed by
 * **at least one** `TurnEndedParams` sharing the same `turn_id`.
 * A second `TurnEnded(Interrupted)` MAY follow a terminal outcome
 * when the user cancel was observed after the engine had already
 * returned — this is the late-cancel signal the TUI uses to fire
 * auto-restore. Consumers should treat the latest `Interrupted`
 * event as authoritative when it follows a same-id terminal.
 */
export interface TurnEndedParams {
  outcome: TurnOutcome;
  turn_id: TurnId;
  usage?: TokenUsage | null;
}

/**
 * Branded turn identifier. One per logical user-prompt cycle —
 * shared between the paired `TurnStarted` and `TurnEnded` events.
 * Generated by the runner layer (`tui_runner` / `sdk_runner`),
 * not the engine, so pre-engine hook blocks still emit a complete
 * lifecycle pair.
 */
export type TurnId = string;

/**
 * Discriminated terminal reason. The `Completed` variant is
 * the only one that carries `stop_reason`: the other variants'
 * names ARE the terminal reason.
 *
 * Wire format: `{"kind": "completed", "data": {"stop_reason": "end_turn"}}` etc.
 *
 * Per-variant payloads are **named structs** rather than inline
 * struct-variants. This is deliberate: schemars emits `$ref` to the
 * named struct, which the Python codegen turns into a real Pydantic
 * model. Inline struct-variants would degrade to `dict[str, Any]` on
 * the Python side — losing typed access for SDK consumers.
 */
export type TurnOutcome = {
  data: CompletedOutcome;
  kind: "completed";
} | {
  data: FailedOutcome;
  kind: "failed";
} | {
  data: InterruptedOutcome;
  kind: "interrupted";
} | {
  data: MaxTurnsReachedOutcome;
  kind: "max_turns_reached";
} | {
  data: BudgetExhaustedOutcome;
  kind: "budget_exhausted";
};

/**
 * Params for `turn/start`.
 */
export interface TurnStartParams {
  permission_mode?: PermissionMode | null;
  prompt: string;
  thinking_level?: ThinkingLevel | null;
}

/**
 * Response to `ClientRequest::TurnStart`.
 * `turn/start` is a fire-and-forget trigger — the server accepts the
 * request, spawns the turn as a detached task, and replies immediately
 * with a handle. Progress is delivered via `turn/started`, streaming
 * deltas, and the terminal `turn/ended` notification.
 */
export interface TurnStartResult {
  turn_id: string;
}

export interface TurnStartedParams {
  turn_id: TurnId;
}

/**
 * Unified finish reason for a completed LLM turn.
 *
 * Multi-LLM-stable: each `vercel-ai-<provider>` adapter maps its raw
 * stop_reason into one of these variants. See module docs for the
 * per-provider mapping table.
 */
export type UnifiedFinishReason = "end_turn" | "stop_sequence" | "tool_use" | "max_tokens" | "model_context_window_exceeded" | "content_filter" | "error" | "other";

/**
 * Params for `control/updateEnv`.
 */
export interface UpdateEnvParams {
  env: { [key: string]: string; };
}

/**
 * User message content parts.
 */
export type UserContentPart = TextPart | FilePart;

/**
 * Params for `input/resolveUserInput`.
 */
export interface UserInputResolveParams {
  answer: string;
  request_id: string;
}

export interface UserMessage {
  is_compact_summary?: boolean;
  is_virtual?: boolean;
  is_visible_in_transcript_only?: boolean;
  message: LanguageModelV4Message;
  origin?: MessageOrigin | null;
  parent_tool_use_id?: string | null;
  permission_mode?: PermissionMode | null;
  timestamp?: string;
  uuid: string;
}

/**
 * Input for UserPromptSubmit hooks.
 */
export interface UserPromptSubmitInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  prompt: string;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "UserPromptSubmit";
}

/**
 * Communication protocol (OpenAI has two APIs).
 */
export type WireApi = "chat" | "responses";

/**
 * Identity badge for a teammate whose tool needs the leader's approval.
 * Surfaced in the leader's permission prompt so a human reviewing a
 * cross-process worker's request can see WHO is asking. The `color` is
 * the worker's assigned per-teammate palette color (a coco-rs
 * improvement over TS's hardcoded `cyan`); text-surface renderers show
 * the name and carry the color for styled / SDK consumers.
 */
export interface WorkerBadge {
  color: AgentColorName;
  name: string;
}

/**
 * Per-agent lifecycle state (`start` → `progress` → `done`, or `error`).
 * A cache-replay hit is reported
 * as `Done` with the sibling `cached: true` flag, not a distinct state.
 */
export type WorkflowAgentState = "start" | "progress" | "done" | "error";

/**
 * One selectable workflow script in the `/workflow` picker.
 */
export interface WorkflowDialogEntry {
  description?: string;
  name: string;
  sourcePath: string;
}

/**
 * Payload for [`TuiOnlyEvent::OpenWorkflowPicker`].
 */
export interface WorkflowDialogPayload {
  entries: Array<WorkflowDialogEntry>;
}

/**
 * Typed workflow progress payload carried by `task/progress`.
 */
export type WorkflowProgressEvent = {
  agentId?: string | null;
  cached?: boolean;
  durationMs?: number | null;
  error?: string | null;
  index: number;
  label: string;
  model?: string | null;
  phaseIndex?: number | null;
  phaseTitle?: string | null;
  promptPreview?: string | null;
  resultPreview?: string | null;
  state: WorkflowAgentState;
  tokens?: number | null;
  toolCalls?: number | null;
  type: "workflow_agent";
} | {
  index: number;
  title: string;
  type: "workflow_phase";
} | {
  message: string;
  type: "workflow_log";
};

/**
 * Input for WorktreeCreate hooks.
 */
export interface WorktreeCreateInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  name: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  hook_event_name: "WorktreeCreate";
}

export interface WorktreeEnteredParams {
  branch: string;
  worktree_path: string;
}

export interface WorktreeExitedParams {
  action: string;
  worktree_path: string;
}

/**
 * Input for WorktreeRemove hooks.
 */
export interface WorktreeRemoveInput {
  agent_id?: string | null;
  agent_type?: string | null;
  cwd: string;
  permission_mode?: string | null;
  session_id: string;
  transcript_path?: string;
  worktree_path: string;
  hook_event_name: "WorktreeRemove";
}

export type InitializeRequest = {
  method: "initialize";
  params: InitializeParams;
};

export type SessionStartRequest = {
  method: "session/start";
  params: SessionStartParams;
};

export type SessionResumeRequest = {
  method: "session/resume";
  params: SessionResumeParams;
};

export type SessionListRequest = {
  method: "session/list";
};

export type SessionReadRequest = {
  method: "session/read";
  params: SessionReadParams;
};

export type SessionArchiveRequest = {
  method: "session/archive";
  params: SessionArchiveParams;
};

export type TurnStartRequest = {
  method: "turn/start";
  params: TurnStartParams;
};

export type TurnInterruptRequest = {
  method: "turn/interrupt";
};

export type ApprovalResolveRequest = {
  method: "approval/resolve";
  params: ApprovalResolveParams;
};

export type InputResolveUserInputRequest = {
  method: "input/resolveUserInput";
  params: UserInputResolveParams;
};

/**
 * Resolve a pending MCP elicitation request. Counterpart to the
 * `ServerRequest` the agent sends when an MCP server needs
 * structured user input (form values, OAuth tokens, etc.).
 * See `event-system-design.md` §5.4.
 */
export type ElicitationResolveRequest = {
  method: "elicitation/resolve";
  params: ElicitationResolveParams;
};

export type ControlSetModelRequest = {
  method: "control/setModel";
  params: SetModelParams;
};

export type ControlSetPermissionModeRequest = {
  method: "control/setPermissionMode";
  params: SetPermissionModeParams;
};

export type ControlSetThinkingRequest = {
  method: "control/setThinking";
  params: SetThinkingParams;
};

export type ControlStopTaskRequest = {
  method: "control/stopTask";
  params: StopTaskParams;
};

export type ControlRewindFilesRequest = {
  method: "control/rewindFiles";
  params: RewindFilesParams;
};

export type ControlUpdateEnvRequest = {
  method: "control/updateEnv";
  params: UpdateEnvParams;
};

export type ControlKeepAliveRequest = {
  method: "control/keepAlive";
};

export type ControlCancelRequestRequest = {
  method: "control/cancelRequest";
  params: CancelRequestParams;
};

/**
 * Interrupt one in-process teammate's active turn without
 * stopping the teammate lifecycle.
 */
export type AgentInterruptCurrentWorkRequest = {
  method: "agent/interruptCurrentWork";
  params: AgentInterruptCurrentWorkParams;
};

export type ConfigReadRequest = {
  method: "config/read";
};

export type ConfigValueWriteRequest = {
  method: "config/value/write";
  params: ConfigWriteParams;
};

/**
 * Query MCP server connection status.
 */
export type McpStatusRequest = {
  method: "mcp/status";
};

/**
 * Get context window usage breakdown.
 */
export type ContextUsageRequest = {
  method: "context/usage";
};

/**
 * Hot-reload MCP server configurations.
 */
export type McpSetServersRequest = {
  method: "mcp/setServers";
  params: McpSetServersParams;
};

/**
 * Reconnect a specific MCP server.
 */
export type McpReconnectRequest = {
  method: "mcp/reconnect";
  params: McpReconnectParams;
};

/**
 * Enable/disable a specific MCP server.
 */
export type McpToggleRequest = {
  method: "mcp/toggle";
  params: McpToggleParams;
};

/**
 * Reload all plugins from disk.
 */
export type PluginReloadRequest = {
  method: "plugin/reload";
};

/**
 * Apply feature flag settings at runtime.
 */
export type ConfigApplyFlagsRequest = {
  method: "config/applyFlags";
  params: ConfigApplyFlagsParams;
};

export type ClientRequest = InitializeRequest | SessionStartRequest | SessionResumeRequest | SessionListRequest | SessionReadRequest | SessionArchiveRequest | TurnStartRequest | TurnInterruptRequest | ApprovalResolveRequest | InputResolveUserInputRequest | ElicitationResolveRequest | ControlSetModelRequest | ControlSetPermissionModeRequest | ControlSetThinkingRequest | ControlStopTaskRequest | ControlRewindFilesRequest | ControlUpdateEnvRequest | ControlKeepAliveRequest | ControlCancelRequestRequest | AgentInterruptCurrentWorkRequest | ConfigReadRequest | ConfigValueWriteRequest | McpStatusRequest | ContextUsageRequest | McpSetServersRequest | McpReconnectRequest | McpToggleRequest | PluginReloadRequest | ConfigApplyFlagsRequest;

/**
 * New session started.
 */
export type SessionStartedNotification = {
  method: "session/started";
  params: SessionStartedParams;
};

/**
 * Session result (final usage, cost, stop reason).
 */
export type SessionResultNotification = {
  method: "session/result";
  params: SessionResultParams;
};

/**
 * Session ended.
 */
export type SessionEndedNotification = {
  method: "session/ended";
  params: SessionEndedParams;
};

/**
 * Session usage snapshot updated.
 */
export type SessionUsageUpdatedNotification = {
  method: "session/usageUpdated";
  params: SessionUsageSnapshot;
};

/**
 * One Message appended to engine MessageHistory.
 *
 * `session_id` + `agent_id` envelope (plan §11 F9): merged-timeline
 * consumers (AgentTeams) demux per session/agent off the same event
 * stream. `agent_id` is `None` for the main agent; `Some` for
 * teammates / subagents. Single-session SDK consumers may ignore
 * both fields (`#[serde(default)]` keeps the wire forward-compat).
 */
export type HistoryMessageAppendedNotification = {
  method: "history/messageAppended";
  params: {
  agent_id?: string | null;
  message: Message;
  session_id?: string;
};
};

/**
 * MessageHistory truncated to `keep_count` entries (indices
 * >= keep_count discarded). Emitted by explicit-rewind and
 * auto-restore both, so SDK + TUI converge on engine truncation
 * without separate private paths.
 */
export type HistoryMessageTruncatedNotification = {
  method: "history/messageTruncated";
  params: {
  agent_id?: string | null;
  keep_count: number;
  session_id?: string;
};
};

/**
 * Session reset for resume. TUI clears derived transcript view
 * in preparation for a burst of `MessageAppended` that replays
 * the loaded JSONL transcript.
 */
export type HistoryResetForResumeNotification = {
  method: "history/resetForResume";
  params: {
  agent_id?: string | null;
  session_id: string;
};
};

/**
 * Bulk snapshot for resume hydration. Consumers replace the
 * derived transcript view wholesale (one cache-rebuild pass)
 * instead of processing N `MessageAppended` events sequentially.
 * Used when loading large JSONL transcripts where the
 * per-message channel-bounded path would stall at the 256-msg
 * queue boundary and force the engine task to yield. Live
 * appends still use `MessageAppended` — this variant models
 * bulk replacement (a genuinely different operation).
 */
export type HistoryReplacedNotification = {
  method: "history/replaced";
  params: {
  agent_id?: string | null;
  messages: Array<Message>;
  session_id?: string;
};
};

/**
 * Reasoning aggregates attached to a specific assistant message.
 * Engine emits this after `TurnCompleted` (when usage is known)
 * so the TUI side-cache anchors `Thinking · <duration> · <tokens>`
 * by the message UUID rather than re-walking transcript cells.
 * Eliminates the prior I-2 exception in `TranscriptView`.
 */
export type HistoryReasoningMetadataAttachedNotification = {
  method: "history/reasoningMetadataAttached";
  params: ReasoningMetadataAttachedParams;
};

/**
 * Active `/goal` state changed.
 *
 * Mirrors the engine's `ToolAppState.active_goal` so consumers can
 * render live footer/status affordances without reverse-engineering
 * the silent `goal_status` transcript attachments.
 */
export type GoalActiveChangedNotification = {
  method: "goal/activeChanged";
  params: ActiveGoalChangedParams;
};

/**
 * Agent turn started. Emitted by the runner layer
 * (`tui_runner` / `sdk_runner`) at the start of every logical
 * user-prompt cycle. Pairs 1:1 with `TurnEnded` sharing the
 * same `turn_id`. Intra-turn loops (Stop-hook blocking,
 * reactive compact, max-tokens recovery, etc. — every
 * `ContinueReason::*`) do NOT re-emit this event.
 */
export type TurnStartedNotification = {
  method: "turn/started";
  params: TurnStartedParams;
};

/**
 * Agent turn ended. Discriminated by `outcome.kind` —
 * `completed` / `failed` / `interrupted` / `max_turns_reached` /
 * `budget_exhausted`. Paired 1:1 with the preceding
 * `TurnStarted` (same `turn_id`). See `TurnOutcome` for
 * variant payloads; `stop_reason` only appears on `Completed`
 * where it is semantically meaningful.
 */
export type TurnEndedNotification = {
  method: "turn/ended";
  params: TurnEndedParams;
};

/**
 * Thread item started (from StreamAccumulator).
 */
export type ItemStartedNotification = {
  method: "item/started";
  params: {
  item: ThreadItem;
};
};

/**
 * Thread item updated (e.g. tool execution began).
 */
export type ItemUpdatedNotification = {
  method: "item/updated";
  params: {
  item: ThreadItem;
};
};

/**
 * Thread item completed.
 */
export type ItemCompletedNotification = {
  method: "item/completed";
  params: {
  item: ThreadItem;
};
};

/**
 * Text content delta from assistant.
 */
export type AgentMessageDeltaNotification = {
  method: "agentMessage/delta";
  params: ContentDeltaParams;
};

/**
 * Reasoning/thinking delta.
 */
export type ReasoningDeltaNotification = {
  method: "reasoning/delta";
  params: ContentDeltaParams;
};

/**
 * MCP server startup status.
 */
export type McpStartupStatusNotification = {
  method: "mcp/startupStatus";
  params: McpStartupStatusParams;
};

/**
 * All MCP servers finished startup.
 */
export type McpStartupCompleteNotification = {
  method: "mcp/startupComplete";
  params: McpStartupCompleteParams;
};

/**
 * LSP server pool finished prewarm. Fired once per session
 * bootstrap (after `LspManagerAdapter::prewarm` completes), so the
 * TUI status bar can show a `LSP` badge with the running-server
 * count. Not emitted when `Feature::Lsp` is off — `started` /
 * `failed` are empty in that case.
 */
export type LspPrewarmCompleteNotification = {
  method: "lsp/prewarmComplete";
  params: LspPrewarmCompleteParams;
};

/**
 * Context compacted.
 */
export type ContextCompactedNotification = {
  method: "context/compacted";
  params: ContextCompactedParams;
};

/**
 * Context usage warning.
 */
export type ContextUsageWarningNotification = {
  method: "context/usageWarning";
  params: ContextUsageWarningParams;
};

/**
 * Compaction started.
 */
export type ContextCompactionStartedNotification = {
  method: "context/compactionStarted";
};

/**
 * Compaction phase progress (TS `onCompactProgress`).
 * Drives the spinner text in the TUI / SDK runner so the user
 * can see which sub-phase is active (PreCompact hooks → summarize
 * → PostCompact hooks → done).
 */
export type ContextCompactionPhaseNotification = {
  method: "context/compactionPhase";
  params: CompactionPhaseParams;
};

/**
 * Compaction failed.
 */
export type ContextCompactionFailedNotification = {
  method: "context/compactionFailed";
  params: CompactionFailedParams;
};

/**
 * Context cleared (e.g. new mode).
 */
export type ContextClearedNotification = {
  method: "context/cleared";
  params: ContextClearedParams;
};

/**
 * Background task started.
 */
export type TaskStartedNotification = {
  method: "task/started";
  params: TaskStartedParams;
};

/**
 * Background task completed.
 */
export type TaskCompletedNotification = {
  method: "task/completed";
  params: TaskCompletedParams;
};

/**
 * Background task progress.
 */
export type TaskProgressNotification = {
  method: "task/progress";
  params: TaskProgressParams;
};

/**
 * Durable plan-item / V1 todo snapshot — emitted after
 * `TaskCreate`/`TaskUpdate`/`TodoWrite` tools mutate state so
 * the TUI can refresh its panel without pulling the store
 * directly.
 */
export type TaskPanelChangedNotification = {
  method: "task_panel/changed";
  params: TaskPanelChangedParams;
};

/**
 * Team lead received a plan-approval request from a teammate
 * (via mailbox). The TUI surfaces this as a modal overlay.
 */
export type PlanApprovalRequestedNotification = {
  method: "plan_approval/requested";
  params: PlanApprovalRequestedParams;
};

/**
 * Agents killed.
 */
export type AgentsKilledNotification = {
  method: "agents/killed";
  params: AgentsKilledParams;
};

/**
 * Model fallback started.
 */
export type ModelFallbackStartedNotification = {
  method: "model/fallbackStarted";
  params: ModelFallbackParams;
};

/**
 * Model fallback completed.
 */
export type ModelFallbackCompletedNotification = {
  method: "model/fallbackCompleted";
};

/**
 * Fast mode state changed.
 */
export type ModelFastModeChangedNotification = {
  method: "model/fastModeChanged";
  params: {
  active: boolean;
};
};

/**
 * A role's binding (model + provider + effort) changed in-memory
 * via the picker or `Ctrl+T`. Carries the resolved fields the TUI
 * needs to refresh its `model_by_role` cache and, for `Main`,
 * status-bar fields (`model`, `provider`, `thinking_effort`).
 */
export type ModelRoleChangedNotification = {
  method: "model/roleChanged";
  params: ModelRoleChangedParams;
};

/**
 * Permission mode changed.
 */
export type PermissionModeChangedNotification = {
  method: "permission/modeChanged";
  params: PermissionModeChangedParams;
};

/**
 * Prompt suggestions.
 */
export type PromptSuggestionNotification = {
  method: "prompt/suggestion";
  params: {
  suggestions: Array<string>;
};
};

/**
 * Error notification.
 */
export type ErrorNotification = {
  method: "error";
  params: ErrorParams;
};

/**
 * Rate limit notification.
 */
export type RateLimitNotification = {
  method: "rateLimit";
  params: RateLimitParams;
};

/**
 * Keep-alive heartbeat.
 */
export type KeepAliveNotification = {
  method: "keepAlive";
  params: {
  timestamp: number;
};
};

/**
 * IDE selection changed.
 */
export type IdeSelectionChangedNotification = {
  method: "ide/selectionChanged";
  params: IdeSelectionChangedParams;
};

/**
 * IDE diagnostics updated.
 */
export type IdeDiagnosticsUpdatedNotification = {
  method: "ide/diagnosticsUpdated";
  params: IdeDiagnosticsUpdatedParams;
};

/**
 * Command queue state changed.
 */
export type QueueStateChangedNotification = {
  method: "queue/stateChanged";
  params: {
  queued: number;
};
};

/**
 * Command queued.
 */
export type QueueCommandQueuedNotification = {
  method: "queue/commandQueued";
  params: {
  editable: boolean;
  id: string;
  preview: string;
};
};

/**
 * Command dequeued.
 */
export type QueueCommandDequeuedNotification = {
  method: "queue/commandDequeued";
  params: {
  id: string;
};
};

/**
 * File rewind completed.
 */
export type RewindCompletedNotification = {
  method: "rewind/completed";
  params: RewindCompletedParams;
};

/**
 * File rewind failed.
 */
export type RewindFailedNotification = {
  method: "rewind/failed";
  params: {
  error: string;
};
};

/**
 * Cost threshold warning.
 */
export type CostWarningNotification = {
  method: "cost/warning";
  params: CostWarningParams;
};

/**
 * Sandbox state changed.
 */
export type SandboxStateChangedNotification = {
  method: "sandbox/stateChanged";
  params: SandboxStateChangedParams;
};

/**
 * Sandbox violations detected.
 */
export type SandboxViolationsDetectedNotification = {
  method: "sandbox/violationsDetected";
  params: {
  count: number;
};
};

/**
 * Agents registered.
 */
export type AgentsRegisteredNotification = {
  method: "agents/registered";
  params: {
  agents: Array<AgentInfo>;
};
};

/**
 * Hook execution started.
 */
export type HookStartedNotification = {
  method: "hook/started";
  params: HookStartedParams;
};

/**
 * Hook execution progress (TS gap P1 — stdout/stderr streaming).
 */
export type HookProgressNotification = {
  method: "hook/progress";
  params: HookProgressParams;
};

/**
 * Hook execution completed (TS gap P1).
 */
export type HookResponseNotification = {
  method: "hook/response";
  params: HookResponseParams;
};

/**
 * Entered a worktree.
 */
export type WorktreeEnteredNotification = {
  method: "worktree/entered";
  params: WorktreeEnteredParams;
};

/**
 * Exited a worktree.
 */
export type WorktreeExitedNotification = {
  method: "worktree/exited";
  params: WorktreeExitedParams;
};

/**
 * Summarization completed.
 */
export type SummarizeCompletedNotification = {
  method: "summarize/completed";
  params: SummarizeCompletedParams;
};

/**
 * Summarization failed.
 */
export type SummarizeFailedNotification = {
  method: "summarize/failed";
  params: {
  error: string;
};
};

/**
 * Stream stall detected.
 */
export type StreamStallDetectedNotification = {
  method: "stream/stallDetected";
  params: {
  turn_id?: string | null;
};
};

/**
 * Stream watchdog warning.
 */
export type StreamWatchdogWarningNotification = {
  method: "stream/watchdogWarning";
  params: {
  elapsed_secs: number;
};
};

/**
 * Stream request ended (with usage).
 */
export type StreamRequestEndNotification = {
  method: "stream/requestEnd";
  params: {
  usage: TokenUsage;
};
};

/**
 * Session state changed (idle/running/requires_action).
 */
export type SessionStateChangedNotification = {
  method: "session/stateChanged";
  params: {
  state: SessionState;
};
};

/**
 * Output from a user-executed local command (REPL `!` prefix).
 * Matches TS `SDKLocalCommandOutputMessage` (coreSchemas.ts:1590-1602).
 */
export type LocalCommandOutputNotification = {
  method: "localCommand/output";
  params: LocalCommandOutputParams;
};

/**
 * Files persisted to disk (file upload/snapshot completion).
 * Matches TS `SDKFilesPersistedEvent` (coreSchemas.ts:1672-1692).
 */
export type FilesPersistedNotification = {
  method: "files/persisted";
  params: FilesPersistedParams;
};

/**
 * MCP elicitation completed (form submission or cancellation).
 * Matches TS `SDKElicitationCompleteMessage` (coreSchemas.ts:1779-1792).
 */
export type ElicitationCompleteNotification = {
  method: "elicitation/complete";
  params: ElicitationCompleteParams;
};

/**
 * Tool use summary from background haiku summarization.
 * Matches TS `SDKToolUseSummaryMessage` (coreSchemas.ts:1769-1777).
 */
export type ToolUseSummaryNotification = {
  method: "tool/useSummary";
  params: ToolUseSummaryParams;
};

/**
 * Tool execution progress (bash/powershell long-running).
 * Matches TS `SDKToolProgressMessage` (coreSchemas.ts:1648-1659).
 * Sent at most once per 30 seconds per `parent_tool_use_id`.
 */
export type ToolProgressNotification = {
  method: "tool/progress";
  params: ToolProgressParams;
};

/**
 * Plugin state changed on disk (manifest added/removed/edited,
 * `installed_plugins.json` updated, or settings.json scope toggled).
 * Carries a short reason string the UI can surface as a banner.
 * Emits a "Plugins changed. Run /reload-plugins to activate." notification. Never
 * triggers an auto-reload — the explicit `/reload-plugins`
 * invocation is what applies the change.
 */
export type PluginsChangedNotification = {
  method: "plugins/changed";
  params: {
  reason: string;
};
};

export type ServerNotification = SessionStartedNotification | SessionResultNotification | SessionEndedNotification | SessionUsageUpdatedNotification | HistoryMessageAppendedNotification | HistoryMessageTruncatedNotification | HistoryResetForResumeNotification | HistoryReplacedNotification | HistoryReasoningMetadataAttachedNotification | GoalActiveChangedNotification | TurnStartedNotification | TurnEndedNotification | ItemStartedNotification | ItemUpdatedNotification | ItemCompletedNotification | AgentMessageDeltaNotification | ReasoningDeltaNotification | McpStartupStatusNotification | McpStartupCompleteNotification | LspPrewarmCompleteNotification | ContextCompactedNotification | ContextUsageWarningNotification | ContextCompactionStartedNotification | ContextCompactionPhaseNotification | ContextCompactionFailedNotification | ContextClearedNotification | TaskStartedNotification | TaskCompletedNotification | TaskProgressNotification | TaskPanelChangedNotification | PlanApprovalRequestedNotification | AgentsKilledNotification | ModelFallbackStartedNotification | ModelFallbackCompletedNotification | ModelFastModeChangedNotification | ModelRoleChangedNotification | PermissionModeChangedNotification | PromptSuggestionNotification | ErrorNotification | RateLimitNotification | KeepAliveNotification | IdeSelectionChangedNotification | IdeDiagnosticsUpdatedNotification | QueueStateChangedNotification | QueueCommandQueuedNotification | QueueCommandDequeuedNotification | RewindCompletedNotification | RewindFailedNotification | CostWarningNotification | SandboxStateChangedNotification | SandboxViolationsDetectedNotification | AgentsRegisteredNotification | HookStartedNotification | HookProgressNotification | HookResponseNotification | WorktreeEnteredNotification | WorktreeExitedNotification | SummarizeCompletedNotification | SummarizeFailedNotification | StreamStallDetectedNotification | StreamWatchdogWarningNotification | StreamRequestEndNotification | SessionStateChangedNotification | LocalCommandOutputNotification | FilesPersistedNotification | ElicitationCompleteNotification | ToolUseSummaryNotification | ToolProgressNotification | PluginsChangedNotification;

/**
 * Ask the SDK client to approve or deny a tool use.
 * Expected response: `ClientRequest::ApprovalResolve`.
 */
export type ApprovalAskForApprovalServerRequest = {
  method: "approval/askForApproval";
  params: AskForApprovalParams;
};

/**
 * Ask the user a question via the SDK client (e.g. multiple choice).
 * Expected response: `ClientRequest::UserInputResolve`.
 */
export type InputRequestUserInputServerRequest = {
  method: "input/requestUserInput";
  params: RequestUserInputParams;
};

/**
 * Route an MCP JSON-RPC message to the SDK-hosted MCP server.
 * Expected response: `ClientRequest::McpRouteMessageResponse`.
 */
export type McpRouteMessageServerRequest = {
  method: "mcp/routeMessage";
  params: McpRouteMessageParams;
};

/**
 * Invoke an SDK-registered hook callback.
 * Expected response: `ClientRequest::HookCallbackResponse`.
 */
export type HookCallbackServerRequest = {
  method: "hook/callback";
  params: HookCallbackParams;
};

/**
 * Notify the SDK that a previously-sent ServerRequest should be cancelled.
 */
export type ControlCancelRequestServerRequest = {
  method: "control/cancelRequest";
  params: ServerCancelRequestParams;
};

/**
 * Forward an MCP-server-initiated elicitation request to the SDK
 * client, which renders a form and replies with the user's
 * answer. Expected response: `ClientRequest::ElicitationResolve`
 * (delivered as the synchronous JSON-RPC response to this
 * request — the SDK reply payload matches
 * `ElicitationResolveParams`).
 */
export type McpRequestElicitationServerRequest = {
  method: "mcp/requestElicitation";
  params: RequestElicitationParams;
};

export type ServerRequest = ApprovalAskForApprovalServerRequest | InputRequestUserInputServerRequest | McpRouteMessageServerRequest | HookCallbackServerRequest | ControlCancelRequestServerRequest | McpRequestElicitationServerRequest;
