//! Foundation types shared across all coco-rs crates.
//!
//! **Source-level vercel-ai-free.** Provider DTOs (LlmMessage, content
//! parts, ProviderOptions, StopReason, FinishReason, …) come in through
//! `coco-llm-types`, the dedicated DTO seam. This crate names them but
//! never imports `vercel_ai_provider::*` directly. Upgrading the SDK
//! requires editing only `common/llm-types/src/lib.rs` plus the runtime
//! seam in `services/inference`; this crate stays unchanged. See
//! `scripts/check-vercel-ai-seam.sh`.

// === Modules ===
mod agent;
mod agent_ipc;
mod app_state;
mod apply_patch_preview;
mod attachment_kind;
mod cache;
mod client_request;
mod command;
mod composer;
pub mod context_usage;
mod event;
mod extended;
pub mod features;
mod fork_label;
mod goal;
mod hook;
mod id;
pub mod journey;
mod jsonrpc;
mod log;
mod mcp_exposure;
pub mod messages;
// Flat re-export at the crate root: `coco_types::Message` reads better
// than `coco_types::messages::Message`, and mirrors how every other
// coco-types module is surfaced. The submodule path
// (`coco_types::messages::*`) stays available for the operations-layer
// re-export in `coco-messages`.
pub use messages::*;
mod hook_callback_output;
mod permission;
pub mod persisted_output;
mod plugin;
mod provider;
mod provider_auth_status;
mod rate_limit;
mod sandbox;
mod server_request;
mod session_access;
pub mod side_query;
mod stream;
mod stream_accumulator;
mod task;
mod task_list;
mod thinking;
mod token;
mod tool;
mod tool_filter;
pub mod tool_summary;
mod tool_wire_name;
mod wire_tagged;

// === Re-exports ===

pub use composer::{
    PersistedComposer, PersistedComposerElement, QueuedCommandEditImage, SubmittedComposer,
    SubmittedComposerElement,
};

// App-state (cross-turn shared state carried on ToolUseContext)
pub use app_state::{
    ActiveWorktreeState, AppStatePatch, AppStateReadHandle, ElicitationGuard,
    LiveToolPermissionState, McpServerAnnouncementState, PendingPermissionGuard,
    PendingPlanVerificationState, ToolAppState,
};

// Per-provider rate-limit state (lives on `ToolAppState.rate_limits`).
pub use rate_limit::RateLimitEntry;

// Attachment taxonomy (full `AttachmentKind` catalog + coverage)
pub use attachment_kind::{
    AttachmentEvent, AttachmentKind, Coverage, SessionResultConsumption, coverage_of,
    session_result_consumption_of,
};

// Prompt-cache shared types (consumed by services/inference + app/query;
// adapter mirrors live in vercel-ai-anthropic — see prompt-cache-design.md §7)
pub use cache::{
    AccountKind, BetaCapability, CacheScope, CacheTtl, PromptCacheConfig, PromptCacheMode,
};

// Agent types
pub use agent::{
    AgentColorName, AgentDefinition, AgentIsolation, AgentMcpServerSpec, AgentSource, AgentTypeId,
    MemoryScope, ModelInheritance, ModelSource, SubagentType, ToolAllowList, WorkerBadge,
};

// Inter-agent IPC (mailbox protocol + sub-agent state snapshots)
pub use agent_ipc::{
    IdleReason, StandaloneAgentContext, SubAgentState, SubAgentStatus, SubagentRuntimeSnapshot,
    TaskEntry, TeammateProtocolContent, TeammateProtocolMessage,
};

// Apply-patch UI preview DTOs.
pub use apply_patch_preview::{
    ApplyPatchPreview, ApplyPatchPreviewAction, ApplyPatchPreviewRow, ApplyPatchPreviewSign,
    AskUserQuestionAnswered, AskUserQuestionResult, ExitPlanModeResult, ToolDisplayData,
};

// Event types (three-layer CoreEvent system; see event-system-design.md)
pub use event::{
    AgentInfo, AgentSkillLifecycleWire, AgentStreamEvent, AgentsDialogEntry, AgentsDialogPayload,
    AgentsKilledParams, CompactionFailedParams, CompactionHookType, CompactionPhase,
    CompactionPhaseParams, ContentDeltaParams, ContextClearedParams, ContextCompactedParams,
    ContextUsageWarningParams, CoreEvent, CostWarningParams, ElicitationCompleteParams, ErrorCode,
    ErrorParams, ErrorPayload, EventLayer, EventReplayPolicy, FastModeState, FileChangeInfo,
    FileChangeKind, FilesPersistedParams, HistoryReplaceReason, HookOutcomeStatus,
    HookProgressParams, HookResponseParams, HookStartedParams, IdeDiagnosticsUpdatedParams,
    IdeSelectionChangedParams, ItemStatus, JourneyBusiestDayWire, JourneyDialogPayload,
    JourneyMutationFailed, JourneyMutationKind, JourneyNodeBodyWire, JourneyNodeWire,
    JourneyStatsWire, LocalCommandOutputParams, McpServerInit, McpStartupCompleteParams,
    McpStartupStatusParams, MemoryDialogEntry, MemoryDialogRowKind, MemoryDialogScope,
    MoaAggregatingParams, MoaReferenceParams, ModelFallbackParams, ModelRoleChangedParams,
    NotificationMethod, PermissionDenialInfo, PermissionDisplayInput, PermissionModeChangedParams,
    PermissionsEditorDir, PermissionsEditorPayload, PermissionsEditorRule, PersistedFileError,
    PersistedFileInfo, PlanApprovalRequestedParams, PluginDialogAction, PluginDialogErrorRow,
    PluginDialogInstalledRow, PluginDialogMarketplaceRow, PluginDialogMcpServerRow,
    PluginDialogMcpToolRow, PluginDialogOptionRow, PluginDialogPayload, PluginDialogSkillRow,
    PluginDialogSkillUsage, PluginInit, RateLimitParams, RateLimitStatus,
    ReasoningMetadataAttachedParams, RewindCompletedParams, RewindDiffStatsPayload,
    RewindRowMetadata, SESSION_EVENT_METHOD, SESSION_LIFECYCLE_METHOD, SandboxStateChangedParams,
    ServerNotification, ServerNotificationIdentity, SessionEndedParams, SessionEnvelope,
    SessionModelUsage, SessionResultParams, SessionScopedEvent, SessionStartedParams, SessionState,
    SkillLock, SkillLockSource, SkillOverrideState, SkillOverridesSaveErrorKind,
    SkillOverridesSaveResult, SkillQuarantineWire, SkillTelemetryWire, SkillsDialogEntry,
    SkillsDialogPayload, SkillsDialogSource, SlashCommandStatusKind, SummarizeCompletedParams,
    TaskCompletedParams, TaskCompletionStatus, TaskPanelChangedParams, TaskProgressParams,
    TaskStartedParams, TaskUsage, ThreadItem, ThreadItemDetails, TimelineBucketWire,
    ToolAbortReasonPayload, ToolProgressParams, ToolUseSummaryParams, TuiOnlyEvent,
    TurnAbortReason, TurnEndedParams, TurnOutcome, TurnStartedParams, WorkflowDialogEntry,
    WorkflowDialogPayload, WorktreeEnteredParams, WorktreeExitedParams,
};
pub use stream_accumulator::StreamAccumulator;

// Session delivery DTOs shared by server routing and client adapters.
pub use session_access::{
    ServerRequestDelivery, SessionAccess, SessionDelivery, SessionLifecycleEffect,
    SessionLifecycleEffectKind,
};

// Client request types (Phase 2 — SDK control protocol, SDK → agent)
pub use client_request::{
    AgentInterruptCurrentWorkParams, ApplyPermissionUpdateParams, ApprovalDecision,
    ApprovalResolveParams, BackgroundAllTasksResult, CancelRequestParams, ClientAgentDefinition,
    ClientRequest, ClientRequestMethod, ConfigApplyFlagsParams, ConfigReadParams, ConfigReadTarget,
    ConfigWriteParams, ConfigWriteTarget, ConnectionProfile, ConnectionProfileError,
    ElicitationResolveParams, HookCallbackMatcher, InitializeParams, McpReconnectParams,
    McpSetServersParams, McpToggleParams, RequestScope, ResetSessionPermissionRulesResult,
    RewindFilesParams, SessionCloseParams, SessionCostResult, SessionDeleteParams,
    SessionReadParams, SessionRenameParams, SessionRenameResult, SessionReplaceParams,
    SessionReplacement, SessionResumeParams, SessionStartParams, SessionStatusResult,
    SessionSubscribeParams, SessionTarget, SessionToggleTagParams, SessionToggleTagResult,
    SessionTurnsListParams, SetAgentColorParams, SetModelParams, SetModelRoleParams,
    SetPermissionModeParams, SetThinkingParams, StopTaskParams, TaskDetailParams, TaskDetailResult,
    TaskListResult, TurnStartParams, UpdateEnvParams, UserInputResolveParams, request_scope,
};

// Hook callback output (stable wire format; mirrors
// `hookJSONOutputSchema`). Single source of truth for the hook callback
// boundary and for hook orchestration's stdout parser.
pub use hook_callback_output::{
    ElicitationAction, HookCallbackOutput, HookCallbackResult, HookDecision, HookSpecificOutput,
    McpRouteMessageResult, PermissionRequestDecision,
};

// Server request types (Phase 2 — SDK control protocol, agent → SDK)
pub use context_usage::{
    ContextCategoryKind, ContextSuggestion, GridCell, GridCellKind, SourceGroup,
    SuggestionSeverity, build_grid, build_suggestions, fmt_token_compact, group_by_source,
    source_group,
};
pub use server_request::{
    AskForApprovalParams as ServerAskForApprovalParams, AttachmentTypeBreakdown, ConfigReadResult,
    ContextAgent, ContextMcpTool, ContextMemoryFile, ContextSkill, ContextUsageCategory,
    ContextUsageResult, EffortLevel, HookCallbackParams as ServerHookCallbackParams,
    HookReloadResult, InitializeAccountInfo, InitializeAgentInfo, InitializeApiProvider,
    InitializeModelInfo, InitializeResult, InitializeSlashCommand, McpConnectionStatus,
    McpRouteMessageParams as ServerMcpRouteMessageParams, McpServerStatus, McpSetServersResult,
    McpSkippedToolStatus, McpStatusResult, MessageBreakdown, PluginReloadResult,
    RequestElicitationParams as ServerRequestElicitationParams,
    RequestUserInputParams as ServerRequestUserInputParams, RewindFilesResult,
    ServerCancelRequestParams, ServerRequest, ServerRequestMethod, SessionListResult,
    SessionReadResult, SessionReplaceResult, SessionResumeResult, SessionSearchHit,
    SessionStartResult, SessionSubscribeEnvelope, SessionSubscribeResult, SessionSummary,
    SessionTurnSummary, SessionTurnsListResult, SetModelRoleResult, ToolTypeBreakdown,
    TurnStartResult,
};

// JSON-RPC envelope types (Phase 2 — wire format)
pub use jsonrpc::{
    JSONRPC_VERSION, JsonRpcError, JsonRpcErrorObject, JsonRpcMessage, JsonRpcNotification,
    JsonRpcRequest, JsonRpcResponse, RequestId, error_codes,
};

// Command types
pub use command::{
    CommandArgumentKind, CommandAvailability, CommandBase, CommandContext, CommandSafety,
    CommandSource, CommandType, CommandTypeTag, LocalCommandData, PromptCommandData,
    SkillProvenanceBadge, SlashCommandInfo, SlashCommandSessionScope,
};

// Hook types
pub use hook::{HookEventType, HookOutcome, HookScope};

// ID types
pub use id::{AgentId, SessionId, TaskId, TurnId};

// Journey (learning-timeline) event schema + node addressing
pub use journey::{JourneyAction, JourneyEvent, JourneyNodeId, JourneyRecord, SkillRetireReason};

// Log types
pub use log::{Entrypoint, LogOption, UserType};

// Tool selection / identity types
pub use tool::{ActiveShellTool, ModelShellToolType};

/// How compaction was triggered.
/// Stays in `coco-types` (rather than `coco-messages`) because
/// `event::CompactionPhaseParams` references it; the rest of the message
/// family lives in `coco-messages`.
/// Variants: manual `/compact`, threshold-based auto, PTL-413
/// reactive recovery, gap-based time-based microcompact, session-memory
/// short-circuit (no LLM), and staged context-collapse commit.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactTrigger {
    Manual,
    Auto,
    Reactive,
    TimeBased,
    SessionMemory,
    ContextCollapse,
}

// Permission types
pub use permission::{
    AdditionalWorkingDir, ClassifierBehavior, ClassifierMode, ClassifierUsage, ExitPlanChoice,
    ExitPlanModeAllowedPrompt, ExitPlanModeOutcome, MAX_PERMISSION_FEEDBACK_BYTES,
    PendingClassifierCheck, PermissionAbortReason, PermissionAskChoice, PermissionBehavior,
    PermissionDecision, PermissionDecisionReason, PermissionRequestDetail,
    PermissionResolutionDetail, PermissionRule, PermissionRuleSource, PermissionRuleValue,
    PermissionRulesBySource, PermissionUpdate, PermissionUpdateDestination, ToolCheckResult,
    ToolPermissionContext, WorkingDirectorySource, content_matches, matches_rule,
    parse_rule_pattern, tool_matches_pattern,
};

// Plugin types
pub use plugin::BuiltinPluginDefinition;

// Provider & model types
pub use provider::{
    ApplyPatchToolType, Capability, CapabilitySet, LlmModelSelection, LoginEntryInfo,
    ModelCatalogInfo, ModelRole, ModelSpec, OAuthFlowId, ProviderApi, ProviderModelSelection,
    ProviderStatusInfo, ProviderUnavailableReason, WireApi,
};
pub use provider_auth_status::{AuthReadinessLevel, AuthRefreshSupport, AuthState};

// Sandbox types
pub use sandbox::SandboxMode;

// Feature gates
pub use features::{
    Feature, FeatureSpec, Features, Stage as FeatureStage, all_features, feature_for_key,
    is_known_feature_key,
};

// Fork-label discriminator (used by logs / telemetry / transcripts to
// identify framework-spawned, cache-shared side-channel queries).
pub use fork_label::ForkLabel;
pub use goal::{
    GoalCommandResult, GoalCreateParams, GoalEditParams, GoalSetStatusParams,
    GoalSnapshotChangedParams, GoalSnapshotView, GoalStatusKind, GoalStatusRequest,
};

// Tool filter pipeline (Layers 2 + 4)
pub use tool_filter::{ToolFilter, ToolOverrides};

// Side-query types (data only; async trait in coco-tool-runtime)
pub use side_query::{
    CacheSafeParams, SideQueryMessage, SideQueryOutputFormat, SideQueryRequest, SideQueryResponse,
    SideQueryRole, SideQueryStopReason, SideQueryToolDef, SideQueryToolUse, SideQueryUsage,
};

// Stream types
pub use stream::{RequestStartEvent, StreamEvent, StreamingThinking, StreamingToolUse, TaskBudget};

// Task types
pub use task::{
    BackendType, BgAgentExtras, DreamExtras, FieldUpdate, MessageRole, RemoteTeammateExtras,
    ShellExtras, TaskActivity, TaskExtras, TaskIdentity, TaskKilledBy, TaskProgress, TaskStateBase,
    TaskStatus, TaskType, TeammateExtras, TeammateRef, TeammateTaskMessage, WorkflowAgentState,
    WorkflowProgressEvent, generate_bg_agent_id, generate_task_id, task_type_wire,
};
pub use task_list::{
    ExpandedView, TaskClaimOutcome, TaskListStatus, TaskRecord, TaskRecordUpdate, TodoRecord,
};

// Thinking types
pub use thinking::{ReasoningEffort, ThinkingLevel};

// Token types
pub use token::{
    InputTokens, ModelUsage, OutputTokens, SessionModelUsageEntry, SessionUsageSnapshot,
    SessionUsageSourceEntry, SessionUsageTotals, TokenUsage, UsageAttribution, UsageSource,
    UsageSourceGroup,
};

// Tool types (ToolResult moved to coco-messages because new_messages: Vec<Message>)
pub use mcp_exposure::McpToolExposure;
pub use tool::{
    AGENT_WORKTREE_BRANCH_PREFIX, MCP_TOOL_PREFIX, MCP_TOOL_SEPARATOR, ToolId, ToolName,
    ToolProgress, legacy_tool_name_aliases_of, normalize_legacy_tool_name,
};
pub use tool_wire_name::{MAX_WIRE_TOOL_NAME_BYTES, WireToolName};

// Extended types (ported from TS hooks.ts, command.ts, permissions.ts, logs.ts)
pub use extended::{
    // Log / transcript extended
    AgentColorEntry,
    AgentNameEntry,
    AgentSettingEntry,
    // Hook extended
    AiTitleEntry,
    AttributionSnapshotEntry,
    CommandBaseExt,
    CommandKind,
    CustomTitleEntry,
    FileAttributionState,
    HookBlockingError,
    HookProgress,
    // Permission extended
    PermissionCommandMetadata,
    PermissionDecisionReasonExt,
    PermissionExplanation,
    PermissionRequestResult,
    PermissionResult,
    PersistedWorktreeSession,
    PrLinkEntry,
    PromptCommandDataExt,
    PromptOption,
    PromptRequest,
    PromptResponse,
    ResumeEntrypoint,
    RiskLevel,
    SandboxOverrideReason,
    SessionMode,
    SummaryEntry,
    TagEntry,
    TaskSummaryEntry,
    ToolPermissionContextExt,
};

/// Permission mode (top-level because it's used by both message and permission modules).
/// Wire format is camelCase. The serde aliases on the drifting variants accept
/// legacy snake_case input so old session transcripts deserialize cleanly.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    #[default]
    Default,
    Plan,
    #[serde(alias = "bypass_permissions")]
    BypassPermissions,
    #[serde(alias = "dont_ask")]
    DontAsk,
    #[serde(alias = "accept_edits")]
    AcceptEdits,
    /// Feature-gated auto mode.
    Auto,
    /// Internal: escalate to parent agent.
    Bubble,
}

impl std::str::FromStr for PermissionMode {
    type Err = ();

    /// Parse the wire string into a mode. Mirrors the `#[serde(rename_all
    /// = "camelCase")]` representation plus the legacy snake_case aliases
    /// accepted on deserialize, so a `mode` field carried as a raw string
    /// (e.g. `AgentSpawnRequest.mode`) round-trips identically to serde.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "default" => Self::Default,
            "plan" => Self::Plan,
            "bypassPermissions" | "bypass_permissions" => Self::BypassPermissions,
            "dontAsk" | "dont_ask" => Self::DontAsk,
            "acceptEdits" | "accept_edits" => Self::AcceptEdits,
            "auto" => Self::Auto,
            "bubble" => Self::Bubble,
            _ => return Err(()),
        })
    }
}

impl PermissionMode {
    /// Next mode when the user presses Shift+Tab.
    /// Cycle: `Default → AcceptEdits → [Plan] → [BypassPermissions] → [Auto] → Default`.
    /// Optional modes are skipped when their gate flag is false — `Plan` is
    /// skipped when the `plan_mode` feature is off (`plan_available == false`).
    pub fn next_in_cycle(
        self,
        plan_available: bool,
        bypass_available: bool,
        auto_available: bool,
    ) -> Self {
        // Shared tail after the (optional) Plan step.
        let after_plan = if bypass_available {
            Self::BypassPermissions
        } else if auto_available {
            Self::Auto
        } else {
            Self::Default
        };
        match self {
            Self::Default => Self::AcceptEdits,
            Self::AcceptEdits => {
                if plan_available {
                    Self::Plan
                } else {
                    after_plan
                }
            }
            Self::Plan => after_plan,
            Self::BypassPermissions => {
                if auto_available {
                    Self::Auto
                } else {
                    Self::Default
                }
            }
            // Auto, DontAsk, Bubble, and any future mode fall back to Default.
            Self::Auto | Self::DontAsk | Self::Bubble => Self::Default,
        }
    }
}

/// Session capabilities for optional permission modes.
///
/// This is not a mode enum: these capabilities are independent. A session can
/// expose bypass permissions, Auto mode, both, or neither.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct PermissionModeAvailability {
    pub bypass_permissions: bool,
    pub auto: bool,
}

impl PermissionModeAvailability {
    pub const fn new(bypass_permissions: bool, auto: bool) -> Self {
        Self {
            bypass_permissions,
            auto,
        }
    }
}
