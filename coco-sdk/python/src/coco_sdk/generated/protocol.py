"""Generated protocol types for the coco SDK.

These types mirror the Rust `coco-app-server-protocol` crate.
Regenerate with: `scripts/generate_python.sh`

Source schemas: coco-rs/app-server-protocol/schema/json/

DO NOT EDIT MANUALLY — changes will be overwritten by the generator.
"""

from __future__ import annotations

from enum import Enum
from typing import Annotated, Any, Literal, Union

from pydantic import BaseModel, Field

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Scalar newtype aliases (transparent Rust newtypes)
# ---------------------------------------------------------------------------

TurnId = str


# ---------------------------------------------------------------------------
# Enums
# ---------------------------------------------------------------------------

class AgentColorName(str, Enum):
    red = 'red'
    blue = 'blue'
    green = 'green'
    yellow = 'yellow'
    purple = 'purple'
    orange = 'orange'
    pink = 'pink'
    cyan = 'cyan'

class AgentSource(str, Enum):
    built_in = 'built-in'
    plugin = 'plugin'
    userSettings = 'userSettings'
    projectSettings = 'projectSettings'
    flagSettings = 'flagSettings'
    policySettings = 'policySettings'

class ApiProvider(str, Enum):
    firstParty = 'firstParty'
    bedrock = 'bedrock'
    vertex = 'vertex'
    foundry = 'foundry'

class ApplyPatchPreviewAction(str, Enum):
    add = 'add'
    delete = 'delete'
    update = 'update'

class ApplyPatchPreviewSign(str, Enum):
    added = 'added'
    removed = 'removed'
    context = 'context'

class ApplyPatchToolType(str, Enum):
    freeform = 'freeform'

class ApprovalDecision(str, Enum):
    allow = 'allow'
    deny = 'deny'

class AttachmentKind(str, Enum):
    plan_mode = 'plan_mode'
    plan_mode_reentry = 'plan_mode_reentry'
    plan_mode_exit = 'plan_mode_exit'
    auto_mode = 'auto_mode'
    auto_mode_exit = 'auto_mode_exit'
    todo_reminder = 'todo_reminder'
    task_reminder = 'task_reminder'
    compaction_reminder = 'compaction_reminder'
    date_change = 'date_change'
    verify_plan_reminder = 'verify_plan_reminder'
    ultrathink_effort = 'ultrathink_effort'
    token_usage = 'token_usage'
    budget_usd = 'budget_usd'
    output_token_usage = 'output_token_usage'
    companion_intro = 'companion_intro'
    deferred_tools_delta = 'deferred_tools_delta'
    agent_listing_delta = 'agent_listing_delta'
    mcp_instructions_delta = 'mcp_instructions_delta'
    hook_success = 'hook_success'
    hook_blocking_error = 'hook_blocking_error'
    hook_additional_context = 'hook_additional_context'
    hook_stopped_continuation = 'hook_stopped_continuation'
    async_hook_response = 'async_hook_response'
    diagnostics = 'diagnostics'
    output_style = 'output_style'
    queued_command = 'queued_command'
    task_status = 'task_status'
    skill_listing = 'skill_listing'
    invoked_skills = 'invoked_skills'
    teammate_mailbox = 'teammate_mailbox'
    team_context = 'team_context'
    mcp_resource = 'mcp_resource'
    agent_mention = 'agent_mention'
    selected_lines_in_ide = 'selected_lines_in_ide'
    opened_file_in_ide = 'opened_file_in_ide'
    nested_memory = 'nested_memory'
    relevant_memories = 'relevant_memories'
    already_read_file = 'already_read_file'
    edited_image_file = 'edited_image_file'
    file = 'file'
    directory = 'directory'
    pdf_reference = 'pdf_reference'
    compact_file_reference = 'compact_file_reference'
    plan_file_reference = 'plan_file_reference'
    edited_text_file = 'edited_text_file'
    command_permissions = 'command_permissions'
    hook_cancelled = 'hook_cancelled'
    hook_error_during_execution = 'hook_error_during_execution'
    hook_non_blocking_error = 'hook_non_blocking_error'
    hook_permission_decision = 'hook_permission_decision'
    hook_system_message = 'hook_system_message'
    structured_output = 'structured_output'
    dynamic_skill = 'dynamic_skill'
    skill_discovery = 'skill_discovery'
    context_efficiency = 'context_efficiency'
    max_turns_reached = 'max_turns_reached'
    current_session_memory = 'current_session_memory'
    teammate_shutdown_batch = 'teammate_shutdown_batch'
    bagel_console = 'bagel_console'
    critical_system_reminder = 'critical_system_reminder'
    slash_command_metadata = 'slash_command_metadata'
    user_context = 'user_context'

class Capability(str, Enum):
    text_generation = 'text_generation'
    streaming = 'streaming'
    vision = 'vision'
    audio = 'audio'
    tool_calling = 'tool_calling'
    embedding = 'embedding'
    extended_thinking = 'extended_thinking'
    structured_output = 'structured_output'
    reasoning_summaries = 'reasoning_summaries'
    parallel_tool_calls = 'parallel_tool_calls'
    fast_mode = 'fast_mode'
    prompt_cache = 'prompt_cache'
    context_1m = 'context_1m'
    interleaved_thinking = 'interleaved_thinking'
    context_management = 'context_management'
    adaptive_thinking = 'adaptive_thinking'
    token_efficient_tools = 'token_efficient_tools'
    server_side_tool_reference = 'server_side_tool_reference'
    client_side_tool_search = 'client_side_tool_search'

class CommandArgumentKind(str, Enum):
    none = 'none'
    free_text = 'free_text'
    file_path = 'file_path'
    directory_path = 'directory_path'
    session_id = 'session_id'

class CommandTypeTag(str, Enum):
    prompt = 'prompt'
    local = 'local'
    local_overlay = 'local_overlay'

class CompactTrigger(str, Enum):
    manual = 'manual'
    auto = 'auto'

class CompactionHookType(str, Enum):
    pre_compact = 'pre_compact'
    post_compact = 'post_compact'
    session_start = 'session_start'

class CompactionPhase(str, Enum):
    hooks_start = 'hooks_start'
    summarizing = 'summarizing'
    done = 'done'

class ConfigChangeSource(str, Enum):
    user_settings = 'user_settings'
    project_settings = 'project_settings'
    local_settings = 'local_settings'
    policy_settings = 'policy_settings'
    skills = 'skills'

class ContextCategoryKind(str, Enum):
    system_prompt = 'system_prompt'
    tools = 'tools'
    mcp_tools = 'mcp_tools'
    agents = 'agents'
    memory_files = 'memory_files'
    skills = 'skills'
    messages = 'messages'
    free = 'free'

class EffortLevel(str, Enum):
    low = 'low'
    medium = 'medium'
    high = 'high'
    max = 'max'

class ElicitationAction(str, Enum):
    accept = 'accept'
    decline = 'decline'
    cancel = 'cancel'

class ElicitationMode(str, Enum):
    form = 'form'
    url = 'url'

class ErrorCode(str, Enum):
    common = 'common'
    input = 'input'
    io = 'io'
    network = 'network'
    auth = 'auth'
    config = 'config'
    provider = 'provider'
    resource = 'resource'
    system_reminder = 'system_reminder'
    hook_blocked = 'hook_blocked'
    unknown = 'unknown'

class ExitPlanModeOutcome(str, Enum):
    implementation_plan = 'implementation_plan'
    no_implementation_plan = 'no_implementation_plan'

class ExitReason(str, Enum):
    clear = 'clear'
    resume = 'resume'
    logout = 'logout'
    prompt_input_exit = 'prompt_input_exit'
    other = 'other'
    bypass_permissions_disabled = 'bypass_permissions_disabled'

class ExpandedView(str, Enum):
    none = 'none'
    tasks = 'tasks'
    teammates = 'teammates'

class FastModeState(str, Enum):
    off = 'off'
    cooldown = 'cooldown'
    on = 'on'

class FileChangeEvent(str, Enum):
    change = 'change'
    add = 'add'
    unlink = 'unlink'

class FileChangeKind(str, Enum):
    create = 'create'
    modify = 'modify'
    delete = 'delete'

class HookDecision(str, Enum):
    approve = 'approve'
    block = 'block'

class HookEventType(str, Enum):
    PreToolUse = 'PreToolUse'
    PostToolUse = 'PostToolUse'
    PostToolUseFailure = 'PostToolUseFailure'
    SessionStart = 'SessionStart'
    SessionEnd = 'SessionEnd'
    Setup = 'Setup'
    Stop = 'Stop'
    StopFailure = 'StopFailure'
    SubagentStart = 'SubagentStart'
    SubagentStop = 'SubagentStop'
    UserPromptSubmit = 'UserPromptSubmit'
    PermissionRequest = 'PermissionRequest'
    PermissionDenied = 'PermissionDenied'
    Notification = 'Notification'
    Elicitation = 'Elicitation'
    ElicitationResult = 'ElicitationResult'
    PreCompact = 'PreCompact'
    PostCompact = 'PostCompact'
    TeammateIdle = 'TeammateIdle'
    TaskCreated = 'TaskCreated'
    TaskCompleted = 'TaskCompleted'
    ConfigChange = 'ConfigChange'
    InstructionsLoaded = 'InstructionsLoaded'
    CwdChanged = 'CwdChanged'
    FileChanged = 'FileChanged'
    WorktreeCreate = 'WorktreeCreate'
    WorktreeRemove = 'WorktreeRemove'

class HookOutcomeStatus(str, Enum):
    success = 'success'
    error = 'error'
    cancelled = 'cancelled'

class HookPermissionDecision(str, Enum):
    allow = 'allow'
    deny = 'deny'
    ask = 'ask'

class InstructionsLoadReason(str, Enum):
    session_start = 'session_start'
    nested_traversal = 'nested_traversal'
    path_glob_match = 'path_glob_match'
    include = 'include'
    compact = 'compact'

class ItemStatus(str, Enum):
    in_progress = 'in_progress'
    completed = 'completed'
    failed = 'failed'
    declined = 'declined'

class McpConnectionStatus(str, Enum):
    connected = 'connected'
    pending = 'pending'
    failed = 'failed'
    needs_auth = 'needs-auth'
    disabled = 'disabled'
    disconnected = 'disconnected'

class MemoryDialogScope(str, Enum):
    managed = 'managed'
    user = 'user'
    project = 'project'
    project_local = 'project_local'
    project_config = 'project_config'
    subdir = 'subdir'
    imported = 'imported'
    auto_mem_folder = 'auto_mem_folder'
    team_mem_folder = 'team_mem_folder'
    agent_mem_folder = 'agent_mem_folder'

class MemoryScope(str, Enum):
    user = 'user'
    project = 'project'
    local = 'local'

class MemoryType(str, Enum):
    User = 'User'
    Project = 'Project'
    Local = 'Local'
    Managed = 'Managed'

class MentionItemKind(str, Enum):
    file = 'file'
    already_read = 'already_read'
    directory = 'directory'
    image = 'image'
    pdf = 'pdf'

class MessageKind(str, Enum):
    user = 'user'
    assistant = 'assistant'
    system = 'system'
    attachment = 'attachment'
    tool_result = 'tool_result'
    progress = 'progress'
    tombstone = 'tombstone'

class MessageOrigin(str, Enum):
    user_input = 'user_input'
    system_injected = 'system_injected'
    tool_result = 'tool_result'
    compact_summary = 'compact_summary'
    subagent_reply = 'subagent_reply'
    slash_command = 'slash_command'
    plan_implementation = 'plan_implementation'
    queued_steering = 'queued_steering'

class ModelRole(str, Enum):
    main = 'main'
    fast = 'fast'
    explore = 'explore'
    review = 'review'
    memory = 'memory'
    hook_agent = 'hook_agent'
    plan = 'plan'
    subagent = 'subagent'

class PermissionBehavior(str, Enum):
    allow = 'allow'
    deny = 'deny'
    ask = 'ask'

class PermissionMode(str, Enum):
    default = 'default'
    plan = 'plan'
    bypassPermissions = 'bypassPermissions'
    dontAsk = 'dontAsk'
    acceptEdits = 'acceptEdits'
    auto = 'auto'
    bubble = 'bubble'

class PermissionRuleSource(str, Enum):
    user_settings = 'user_settings'
    project_settings = 'project_settings'
    local_settings = 'local_settings'
    flag_settings = 'flag_settings'
    policy_settings = 'policy_settings'
    cli_arg = 'cli_arg'
    command = 'command'
    session = 'session'

class PermissionUpdateDestination(str, Enum):
    user_settings = 'user_settings'
    project_settings = 'project_settings'
    local_settings = 'local_settings'
    session = 'session'
    cli_arg = 'cli_arg'
    command = 'command'

class ProviderApi(str, Enum):
    anthropic = 'anthropic'
    openai = 'openai'
    gemini = 'gemini'
    volcengine = 'volcengine'
    zai = 'zai'
    openai_compat = 'openai_compat'

class RateLimitStatus(str, Enum):
    allowed = 'allowed'
    allowed_warning = 'allowed_warning'
    rejected = 'rejected'

class ReasoningEffort(str, Enum):
    minimal = 'minimal'
    low = 'low'
    medium = 'medium'
    high = 'high'
    x_high = 'x_high'
    off = 'off'
    auto = 'auto'

class RiskLevel(str, Enum):
    LOW = 'LOW'
    MEDIUM = 'MEDIUM'
    HIGH = 'HIGH'

class SessionStartSource(str, Enum):
    startup = 'startup'
    resume = 'resume'
    clear = 'clear'
    compact = 'compact'

class SessionState(str, Enum):
    idle = 'idle'
    running = 'running'
    requires_action = 'requires_action'

class SetupTrigger(str, Enum):
    init = 'init'
    maintenance = 'maintenance'

class SkillDiscoverySource(str, Enum):
    native = 'native'
    aki = 'aki'
    both = 'both'

class SkillLockSource(str, Enum):
    policy = 'policy'
    flag = 'flag'
    author = 'author'
    plugin = 'plugin'

class SkillOverrideState(str, Enum):
    on = 'on'
    name_only = 'name-only'
    user_invocable_only = 'user-invocable-only'
    off = 'off'

class SkillOverridesSaveErrorKind(str, Enum):
    io = 'io'
    parse = 'parse'
    rebuild = 'rebuild'
    no_publisher = 'no_publisher'

class SkillsDialogSource(str, Enum):
    built_in = 'built_in'
    project = 'project'
    user = 'user'
    policy = 'policy'
    plugin = 'plugin'
    mcp = 'mcp'

class SourceType(str, Enum):
    url = 'url'
    document = 'document'

class SuggestionSeverity(str, Enum):
    warning = 'warning'
    info = 'info'

class SystemMessageLevel(str, Enum):
    info = 'info'
    warning = 'warning'
    error = 'error'

class TaskCompletionStatus(str, Enum):
    completed = 'completed'
    failed = 'failed'
    stopped = 'stopped'

class TaskListStatus(str, Enum):
    pending = 'pending'
    in_progress = 'in_progress'
    completed = 'completed'

class TurnAbortReason(str, Enum):
    user_cancel = 'user_cancel'
    submit_interrupt = 'submit_interrupt'
    system_preempt = 'system_preempt'
    permission_abort = 'permission_abort'
    background = 'background'

class UnifiedFinishReason(str, Enum):
    end_turn = 'end_turn'
    stop_sequence = 'stop_sequence'
    tool_use = 'tool_use'
    max_tokens = 'max_tokens'
    model_context_window_exceeded = 'model_context_window_exceeded'
    content_filter = 'content_filter'
    error = 'error'
    other = 'other'

class WireApi(str, Enum):
    chat = 'chat'
    responses = 'responses'


# ---------------------------------------------------------------------------
# Union type aliases
# ---------------------------------------------------------------------------

# One entry in `AgentDefinition.mcp_servers`. Mirrors the TS union:
AgentMcpServerSpec = str | dict[str, Any]

# Assistant message content parts.
AssistantContentPart = Union["TextPart", "FilePart", "ReasoningPart", "ReasoningFilePart", "CustomPart", "ToolCallPart", "ToolResultPart", "SourcePart", "ToolApprovalRequestPart"]

# Typed payload for an [`AttachmentMessage`](super::AttachmentMessage).
AttachmentBody = Union["LanguageModelV4Message", "SilentPayload", "dict[str, Any]"]

# Typed structured extras carried alongside an [`AttachmentBody::Api`] body.
AttachmentExtras = Union["SkillDiscoveryPayload", "CompactFileReferencePayload", "MentionSummaryPayload"]

# Top-level wire message. SDK clients send these over stdin; coco-rs
JsonRpcMessage = Union["JsonRpcRequest", "JsonRpcResponse", "JsonRpcNotification", "JsonRpcError"]

# Top-level message enum.
Message = Union["UserMessage", "AssistantMessage", "SystemMessage", "AttachmentMessage", "ToolResultMessage", "ProgressMessage", "TombstoneMessage"]

# Tool-specific payload for permission UIs.
PermissionRequestDetail = dict[str, Any]

# Request identifier. Can be a string or integer per JSON-RPC 2.0.
RequestId = int | str

# Typed payload for silent attachment kinds.
SilentPayload = Union["HookCancelledPayload", "HookErrorDuringExecutionPayload", "HookNonBlockingErrorPayload", "HookSystemMessagePayload", "HookPermissionDecisionPayload", "CommandPermissionsPayload", "StructuredOutputPayload", "DynamicSkillPayload", "MaxTurnsReachedPayload", "AlreadyReadFilePayload", "EditedImageFilePayload"]

# System messages have sub-types for different notification kinds.
SystemMessage = Union["SystemInformationalMessage", "SystemApiErrorMessage", "SystemCompactBoundaryMessage", "SystemMicrocompactBoundaryMessage", "SystemLocalCommandMessage", "SystemPermissionRetryMessage", "SystemBridgeStatusMessage", "SystemMemorySavedMessage", "SystemAwaySummaryMessage", "SystemAgentsKilledMessage", "SystemApiMetricsMessage", "SystemStopHookSummaryMessage", "SystemTurnDurationMessage", "SystemScheduledTaskFireMessage", "SystemContextUsageMessage", "SystemUserInterruptionMessage"]

# Tool message content parts.
ToolContentPart = Union["ToolResultPart", "ToolApprovalResponsePart"]

# User message content parts.
UserContentPart = Union["TextPart", "FilePart"]


# ---------------------------------------------------------------------------
# Tagged discriminated unions
# ---------------------------------------------------------------------------

class AgentStreamEventTextDelta(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['text_delta'] = Field(default='text_delta', alias='type')
    delta: str
    turn_id: str

class AgentStreamEventThinkingDelta(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['thinking_delta'] = Field(default='thinking_delta', alias='type')
    delta: str
    turn_id: str

class AgentStreamEventToolUseQueued(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_use_queued'] = Field(default='tool_use_queued', alias='type')
    call_id: str
    input: Any
    name: str

class AgentStreamEventToolUseStarted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_use_started'] = Field(default='tool_use_started', alias='type')
    call_id: str
    name: str
    batch_id: str | None = None

class AgentStreamEventToolUseCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_use_completed'] = Field(default='tool_use_completed', alias='type')
    call_id: str
    is_error: bool
    name: str
    output: str

class AgentStreamEventMcpToolCallBegin(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['mcp_tool_call_begin'] = Field(default='mcp_tool_call_begin', alias='type')
    call_id: str
    server: str
    tool: str

class AgentStreamEventMcpToolCallEnd(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['mcp_tool_call_end'] = Field(default='mcp_tool_call_end', alias='type')
    call_id: str
    is_error: bool
    server: str
    tool: str

AgentStreamEvent = Annotated[
    Union[AgentStreamEventTextDelta, AgentStreamEventThinkingDelta, AgentStreamEventToolUseQueued, AgentStreamEventToolUseStarted, AgentStreamEventToolUseCompleted, AgentStreamEventMcpToolCallBegin, AgentStreamEventMcpToolCallEnd],
    Field(discriminator='type_'),
]

class ApplyPatchPreviewRowHeader(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['header'] = Field(default='header', alias='kind')
    action: ApplyPatchPreviewAction
    target: str

class ApplyPatchPreviewRowLine(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['line'] = Field(default='line', alias='kind')
    content: str
    sign: ApplyPatchPreviewSign

class ApplyPatchPreviewRowRaw(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['raw'] = Field(default='raw', alias='kind')
    content: str

class ApplyPatchPreviewRowOmitted(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['omitted'] = Field(default='omitted', alias='kind')
    rows: int

ApplyPatchPreviewRow = Annotated[
    Union[ApplyPatchPreviewRowHeader, ApplyPatchPreviewRowLine, ApplyPatchPreviewRowRaw, ApplyPatchPreviewRowOmitted],
    Field(discriminator='kind'),
]

class ClientRequestInitialize(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['initialize'] = Field(default='initialize', alias='method')
    params: InitializeParams

class ClientRequestSessionStart(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/start'] = Field(default='session/start', alias='method')
    params: SessionStartParams

class ClientRequestSessionResume(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/resume'] = Field(default='session/resume', alias='method')
    params: SessionResumeParams

class ClientRequestSessionList(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/list'] = Field(default='session/list', alias='method')

class ClientRequestSessionRead(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/read'] = Field(default='session/read', alias='method')
    params: SessionReadParams

class ClientRequestSessionArchive(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/archive'] = Field(default='session/archive', alias='method')
    params: SessionArchiveParams

class ClientRequestTurnStart(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['turn/start'] = Field(default='turn/start', alias='method')
    params: TurnStartParams

class ClientRequestTurnInterrupt(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['turn/interrupt'] = Field(default='turn/interrupt', alias='method')

class ClientRequestApprovalResolve(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['approval/resolve'] = Field(default='approval/resolve', alias='method')
    params: ApprovalResolveParams

class ClientRequestInputResolveUserInput(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['input/resolveUserInput'] = Field(default='input/resolveUserInput', alias='method')
    params: UserInputResolveParams

class ClientRequestElicitationResolve(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['elicitation/resolve'] = Field(default='elicitation/resolve', alias='method')
    params: ElicitationResolveParams

class ClientRequestControlSetModel(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/setModel'] = Field(default='control/setModel', alias='method')
    params: SetModelParams

class ClientRequestControlSetPermissionMode(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/setPermissionMode'] = Field(default='control/setPermissionMode', alias='method')
    params: SetPermissionModeParams

class ClientRequestControlSetThinking(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/setThinking'] = Field(default='control/setThinking', alias='method')
    params: SetThinkingParams

class ClientRequestControlStopTask(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/stopTask'] = Field(default='control/stopTask', alias='method')
    params: StopTaskParams

class ClientRequestControlRewindFiles(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/rewindFiles'] = Field(default='control/rewindFiles', alias='method')
    params: RewindFilesParams

class ClientRequestControlUpdateEnv(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/updateEnv'] = Field(default='control/updateEnv', alias='method')
    params: UpdateEnvParams

class ClientRequestControlKeepAlive(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/keepAlive'] = Field(default='control/keepAlive', alias='method')

class ClientRequestControlCancelRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/cancelRequest'] = Field(default='control/cancelRequest', alias='method')
    params: CancelRequestParams

class ClientRequestAgentInterruptCurrentWork(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['agent/interruptCurrentWork'] = Field(default='agent/interruptCurrentWork', alias='method')
    params: AgentInterruptCurrentWorkParams

class ClientRequestConfigRead(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['config/read'] = Field(default='config/read', alias='method')

class ClientRequestConfigValueWrite(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['config/value/write'] = Field(default='config/value/write', alias='method')
    params: ConfigWriteParams

class ClientRequestMcpStatus(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/status'] = Field(default='mcp/status', alias='method')

class ClientRequestContextUsage(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/usage'] = Field(default='context/usage', alias='method')

class ClientRequestMcpSetServers(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/setServers'] = Field(default='mcp/setServers', alias='method')
    params: McpSetServersParams

class ClientRequestMcpReconnect(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/reconnect'] = Field(default='mcp/reconnect', alias='method')
    params: McpReconnectParams

class ClientRequestMcpToggle(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/toggle'] = Field(default='mcp/toggle', alias='method')
    params: McpToggleParams

class ClientRequestPluginReload(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['plugin/reload'] = Field(default='plugin/reload', alias='method')

class ClientRequestConfigApplyFlags(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['config/applyFlags'] = Field(default='config/applyFlags', alias='method')
    params: ConfigApplyFlagsParams

ClientRequest = Annotated[
    Union[ClientRequestInitialize, ClientRequestSessionStart, ClientRequestSessionResume, ClientRequestSessionList, ClientRequestSessionRead, ClientRequestSessionArchive, ClientRequestTurnStart, ClientRequestTurnInterrupt, ClientRequestApprovalResolve, ClientRequestInputResolveUserInput, ClientRequestElicitationResolve, ClientRequestControlSetModel, ClientRequestControlSetPermissionMode, ClientRequestControlSetThinking, ClientRequestControlStopTask, ClientRequestControlRewindFiles, ClientRequestControlUpdateEnv, ClientRequestControlKeepAlive, ClientRequestControlCancelRequest, ClientRequestAgentInterruptCurrentWork, ClientRequestConfigRead, ClientRequestConfigValueWrite, ClientRequestMcpStatus, ClientRequestContextUsage, ClientRequestMcpSetServers, ClientRequestMcpReconnect, ClientRequestMcpToggle, ClientRequestPluginReload, ClientRequestConfigApplyFlags],
    Field(discriminator='method'),
]

class CommandSourceBuiltin(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['builtin'] = Field(default='builtin', alias='kind')

class CommandSourceBundled(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['bundled'] = Field(default='bundled', alias='kind')

class CommandSourceUser(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['user'] = Field(default='user', alias='kind')

class CommandSourceProject(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['project'] = Field(default='project', alias='kind')

class CommandSourceManaged(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['managed'] = Field(default='managed', alias='kind')

class CommandSourceSkills(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['skills'] = Field(default='skills', alias='kind')

class CommandSourceCommandsDeprecated(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['commands_deprecated'] = Field(default='commands_deprecated', alias='kind')

class CommandSourcePlugin(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['plugin'] = Field(default='plugin', alias='kind')
    name: str

class CommandSourceMcp(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['mcp'] = Field(default='mcp', alias='kind')
    server_name: str

CommandSource = Annotated[
    Union[CommandSourceBuiltin, CommandSourceBundled, CommandSourceUser, CommandSourceProject, CommandSourceManaged, CommandSourceSkills, CommandSourceCommandsDeprecated, CommandSourcePlugin, CommandSourceMcp],
    Field(discriminator='kind'),
]

class HookSpecificOutputPreToolUse(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['PreToolUse'] = Field(default='PreToolUse', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')
    permission_decision: HookPermissionDecision | None = Field(default=None, alias='permissionDecision')
    permission_decision_reason: str | None = Field(default=None, alias='permissionDecisionReason')
    updated_input: dict[str, Any] = Field(default=None, alias='updatedInput')

class HookSpecificOutputPostToolUse(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['PostToolUse'] = Field(default='PostToolUse', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')
    updated_mcp_tool_output: dict[str, Any] = Field(default=None, alias='updatedMCPToolOutput')

class HookSpecificOutputPostToolUseFailure(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['PostToolUseFailure'] = Field(default='PostToolUseFailure', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')

class HookSpecificOutputUserPromptSubmit(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['UserPromptSubmit'] = Field(default='UserPromptSubmit', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')

class HookSpecificOutputSessionStart(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['SessionStart'] = Field(default='SessionStart', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')
    initial_user_message: str | None = Field(default=None, alias='initialUserMessage')
    watch_paths: list[str] | None = Field(default=None, alias='watchPaths')

class HookSpecificOutputSetup(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['Setup'] = Field(default='Setup', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')

class HookSpecificOutputSubagentStart(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['SubagentStart'] = Field(default='SubagentStart', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')

class HookSpecificOutputPermissionDenied(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['PermissionDenied'] = Field(default='PermissionDenied', alias='hookEventName')
    retry: bool | None = None

class HookSpecificOutputNotification(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['Notification'] = Field(default='Notification', alias='hookEventName')
    additional_context: str | None = Field(default=None, alias='additionalContext')

class HookSpecificOutputPermissionRequest(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['PermissionRequest'] = Field(default='PermissionRequest', alias='hookEventName')
    decision: PermissionRequestDecision | None = None

class HookSpecificOutputElicitation(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['Elicitation'] = Field(default='Elicitation', alias='hookEventName')
    action: ElicitationAction | None = None
    content: dict[str, Any] | None = None

class HookSpecificOutputElicitationResult(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['ElicitationResult'] = Field(default='ElicitationResult', alias='hookEventName')
    action: ElicitationAction | None = None
    content: dict[str, Any] | None = None

class HookSpecificOutputCwdChanged(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['CwdChanged'] = Field(default='CwdChanged', alias='hookEventName')
    watch_paths: list[str] | None = Field(default=None, alias='watchPaths')

class HookSpecificOutputFileChanged(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['FileChanged'] = Field(default='FileChanged', alias='hookEventName')
    watch_paths: list[str] | None = Field(default=None, alias='watchPaths')

class HookSpecificOutputWorktreeCreate(BaseModel):
    model_config = {"populate_by_name": True}
    hook_event_name: Literal['WorktreeCreate'] = Field(default='WorktreeCreate', alias='hookEventName')
    worktree_path: str | None = Field(default=None, alias='worktreePath')

HookSpecificOutput = Annotated[
    Union[HookSpecificOutputPreToolUse, HookSpecificOutputPostToolUse, HookSpecificOutputPostToolUseFailure, HookSpecificOutputUserPromptSubmit, HookSpecificOutputSessionStart, HookSpecificOutputSetup, HookSpecificOutputSubagentStart, HookSpecificOutputPermissionDenied, HookSpecificOutputNotification, HookSpecificOutputPermissionRequest, HookSpecificOutputElicitation, HookSpecificOutputElicitationResult, HookSpecificOutputCwdChanged, HookSpecificOutputFileChanged, HookSpecificOutputWorktreeCreate],
    Field(discriminator='hook_event_name'),
]

class LanguageModelV4FileDataData(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['data'] = Field(default='data', alias='type')
    data: str

class LanguageModelV4FileDataUrl(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['url'] = Field(default='url', alias='type')
    url: str

LanguageModelV4FileData = Annotated[
    Union[LanguageModelV4FileDataData, LanguageModelV4FileDataUrl],
    Field(discriminator='type_'),
]

class LanguageModelV4MessageSystem(BaseModel):
    model_config = {"populate_by_name": True}
    role: Literal['system'] = Field(default='system', alias='role')
    content: list[UserContentPart]
    provider_options: ProviderOptions | None = None

class LanguageModelV4MessageDeveloper(BaseModel):
    model_config = {"populate_by_name": True}
    role: Literal['developer'] = Field(default='developer', alias='role')
    content: list[UserContentPart]
    provider_options: ProviderOptions | None = None

class LanguageModelV4MessageUser(BaseModel):
    model_config = {"populate_by_name": True}
    role: Literal['user'] = Field(default='user', alias='role')
    content: list[UserContentPart]
    provider_options: ProviderOptions | None = None

class LanguageModelV4MessageAssistant(BaseModel):
    model_config = {"populate_by_name": True}
    role: Literal['assistant'] = Field(default='assistant', alias='role')
    content: list[AssistantContentPart]
    provider_options: ProviderOptions | None = None

class LanguageModelV4MessageTool(BaseModel):
    model_config = {"populate_by_name": True}
    role: Literal['tool'] = Field(default='tool', alias='role')
    content: list[ToolContentPart]
    provider_options: ProviderOptions | None = None

LanguageModelV4Message = Annotated[
    Union[LanguageModelV4MessageSystem, LanguageModelV4MessageDeveloper, LanguageModelV4MessageUser, LanguageModelV4MessageAssistant, LanguageModelV4MessageTool],
    Field(discriminator='role'),
]

class MemoryDialogRowKindFile(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['file'] = Field(default='file', alias='kind')
    exists: bool = False
    read_only: bool = False

class MemoryDialogRowKindFolder(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['folder'] = Field(default='folder', alias='kind')
    enabled: bool = False

class MemoryDialogRowKindToggle(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['toggle'] = Field(default='toggle', alias='kind')
    enabled: bool = False

MemoryDialogRowKind = Annotated[
    Union[MemoryDialogRowKindFile, MemoryDialogRowKindFolder, MemoryDialogRowKindToggle],
    Field(discriminator='kind'),
]

class PermissionDisplayInputCommand(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['command'] = Field(default='command', alias='kind')
    value: str

class PermissionDisplayInputJson(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['json'] = Field(default='json', alias='kind')
    value: str

class PermissionDisplayInputText(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['text'] = Field(default='text', alias='kind')
    value: str

class PermissionDisplayInputEmpty(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['empty'] = Field(default='empty', alias='kind')

PermissionDisplayInput = Annotated[
    Union[PermissionDisplayInputCommand, PermissionDisplayInputJson, PermissionDisplayInputText, PermissionDisplayInputEmpty],
    Field(discriminator='kind'),
]

class PermissionRequestDecisionAllow(BaseModel):
    model_config = {"populate_by_name": True}
    behavior: Literal['allow'] = Field(default='allow', alias='behavior')
    updated_input: dict[str, Any] = Field(default=None, alias='updatedInput')

class PermissionRequestDecisionDeny(BaseModel):
    model_config = {"populate_by_name": True}
    behavior: Literal['deny'] = Field(default='deny', alias='behavior')
    interrupt: bool | None = None
    message: str | None = None

PermissionRequestDecision = Annotated[
    Union[PermissionRequestDecisionAllow, PermissionRequestDecisionDeny],
    Field(discriminator='behavior'),
]

class PermissionUpdateAddRules(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['add_rules'] = Field(default='add_rules', alias='type')
    destination: PermissionUpdateDestination
    rules: list[PermissionRule]

class PermissionUpdateReplaceRules(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['replace_rules'] = Field(default='replace_rules', alias='type')
    destination: PermissionUpdateDestination
    rules: list[PermissionRule]

class PermissionUpdateRemoveRules(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['remove_rules'] = Field(default='remove_rules', alias='type')
    destination: PermissionUpdateDestination
    rules: list[PermissionRule]

class PermissionUpdateSetMode(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['set_mode'] = Field(default='set_mode', alias='type')
    mode: PermissionMode

class PermissionUpdateAddDirectories(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['add_directories'] = Field(default='add_directories', alias='type')
    destination: PermissionUpdateDestination
    directories: list[str]

class PermissionUpdateRemoveDirectories(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['remove_directories'] = Field(default='remove_directories', alias='type')
    destination: PermissionUpdateDestination
    directories: list[str]

PermissionUpdate = Annotated[
    Union[PermissionUpdateAddRules, PermissionUpdateReplaceRules, PermissionUpdateRemoveRules, PermissionUpdateSetMode, PermissionUpdateAddDirectories, PermissionUpdateRemoveDirectories],
    Field(discriminator='type_'),
]

class ServerNotificationSessionStarted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/started'] = Field(default='session/started', alias='method')
    params: SessionStartedParams

class ServerNotificationSessionResult(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/result'] = Field(default='session/result', alias='method')
    params: SessionResultParams

class ServerNotificationSessionEnded(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/ended'] = Field(default='session/ended', alias='method')
    params: SessionEndedParams

class ServerNotificationSessionUsageUpdated(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/usageUpdated'] = Field(default='session/usageUpdated', alias='method')
    params: SessionUsageSnapshot

class ServerNotificationHistoryMessageAppended(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['history/messageAppended'] = Field(default='history/messageAppended', alias='method')
    params: dict[str, Any]

class ServerNotificationHistoryMessageTruncated(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['history/messageTruncated'] = Field(default='history/messageTruncated', alias='method')
    params: dict[str, Any]

class ServerNotificationHistoryResetForResume(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['history/resetForResume'] = Field(default='history/resetForResume', alias='method')
    params: dict[str, Any]

class ServerNotificationHistoryReplaced(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['history/replaced'] = Field(default='history/replaced', alias='method')
    params: dict[str, Any]

class ServerNotificationHistoryReasoningMetadataAttached(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['history/reasoningMetadataAttached'] = Field(default='history/reasoningMetadataAttached', alias='method')
    params: ReasoningMetadataAttachedParams

class ServerNotificationTurnStarted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['turn/started'] = Field(default='turn/started', alias='method')
    params: TurnStartedParams

class ServerNotificationTurnEnded(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['turn/ended'] = Field(default='turn/ended', alias='method')
    params: TurnEndedParams

class ServerNotificationItemStarted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['item/started'] = Field(default='item/started', alias='method')
    params: dict[str, Any]

class ServerNotificationItemUpdated(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['item/updated'] = Field(default='item/updated', alias='method')
    params: dict[str, Any]

class ServerNotificationItemCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['item/completed'] = Field(default='item/completed', alias='method')
    params: dict[str, Any]

class ServerNotificationAgentMessageDelta(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['agentMessage/delta'] = Field(default='agentMessage/delta', alias='method')
    params: ContentDeltaParams

class ServerNotificationReasoningDelta(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['reasoning/delta'] = Field(default='reasoning/delta', alias='method')
    params: ContentDeltaParams

class ServerNotificationMcpStartupStatus(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/startupStatus'] = Field(default='mcp/startupStatus', alias='method')
    params: McpStartupStatusParams

class ServerNotificationMcpStartupComplete(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/startupComplete'] = Field(default='mcp/startupComplete', alias='method')
    params: McpStartupCompleteParams

class ServerNotificationLspPrewarmComplete(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['lsp/prewarmComplete'] = Field(default='lsp/prewarmComplete', alias='method')
    params: LspPrewarmCompleteParams

class ServerNotificationContextCompacted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/compacted'] = Field(default='context/compacted', alias='method')
    params: ContextCompactedParams

class ServerNotificationContextUsageWarning(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/usageWarning'] = Field(default='context/usageWarning', alias='method')
    params: ContextUsageWarningParams

class ServerNotificationContextCompactionStarted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/compactionStarted'] = Field(default='context/compactionStarted', alias='method')

class ServerNotificationContextCompactionPhase(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/compactionPhase'] = Field(default='context/compactionPhase', alias='method')
    params: CompactionPhaseParams

class ServerNotificationContextCompactionFailed(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/compactionFailed'] = Field(default='context/compactionFailed', alias='method')
    params: CompactionFailedParams

class ServerNotificationContextCleared(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/cleared'] = Field(default='context/cleared', alias='method')
    params: ContextClearedParams

class ServerNotificationTaskStarted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['task/started'] = Field(default='task/started', alias='method')
    params: TaskStartedParams

class ServerNotificationTaskCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['task/completed'] = Field(default='task/completed', alias='method')
    params: TaskCompletedParams

class ServerNotificationTaskProgress(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['task/progress'] = Field(default='task/progress', alias='method')
    params: TaskProgressParams

class ServerNotificationTaskPanelChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['task_panel/changed'] = Field(default='task_panel/changed', alias='method')
    params: TaskPanelChangedParams

class ServerNotificationPlanApprovalRequested(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['plan_approval/requested'] = Field(default='plan_approval/requested', alias='method')
    params: PlanApprovalRequestedParams

class ServerNotificationAgentsKilled(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['agents/killed'] = Field(default='agents/killed', alias='method')
    params: AgentsKilledParams

class ServerNotificationModelFallbackStarted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['model/fallbackStarted'] = Field(default='model/fallbackStarted', alias='method')
    params: ModelFallbackParams

class ServerNotificationModelFallbackCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['model/fallbackCompleted'] = Field(default='model/fallbackCompleted', alias='method')

class ServerNotificationModelFastModeChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['model/fastModeChanged'] = Field(default='model/fastModeChanged', alias='method')
    params: dict[str, Any]

class ServerNotificationModelRoleChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['model/roleChanged'] = Field(default='model/roleChanged', alias='method')
    params: ModelRoleChangedParams

class ServerNotificationPermissionModeChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['permission/modeChanged'] = Field(default='permission/modeChanged', alias='method')
    params: PermissionModeChangedParams

class ServerNotificationPromptSuggestion(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['prompt/suggestion'] = Field(default='prompt/suggestion', alias='method')
    params: dict[str, Any]

class ServerNotificationError(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['error'] = Field(default='error', alias='method')
    params: ErrorParams

class ServerNotificationRateLimit(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['rateLimit'] = Field(default='rateLimit', alias='method')
    params: RateLimitParams

class ServerNotificationKeepAlive(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['keepAlive'] = Field(default='keepAlive', alias='method')
    params: dict[str, Any]

class ServerNotificationIdeSelectionChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['ide/selectionChanged'] = Field(default='ide/selectionChanged', alias='method')
    params: IdeSelectionChangedParams

class ServerNotificationIdeDiagnosticsUpdated(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['ide/diagnosticsUpdated'] = Field(default='ide/diagnosticsUpdated', alias='method')
    params: IdeDiagnosticsUpdatedParams

class ServerNotificationQueueStateChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['queue/stateChanged'] = Field(default='queue/stateChanged', alias='method')
    params: dict[str, Any]

class ServerNotificationQueueCommandQueued(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['queue/commandQueued'] = Field(default='queue/commandQueued', alias='method')
    params: dict[str, Any]

class ServerNotificationQueueCommandDequeued(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['queue/commandDequeued'] = Field(default='queue/commandDequeued', alias='method')
    params: dict[str, Any]

class ServerNotificationRewindCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['rewind/completed'] = Field(default='rewind/completed', alias='method')
    params: RewindCompletedParams

class ServerNotificationRewindFailed(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['rewind/failed'] = Field(default='rewind/failed', alias='method')
    params: dict[str, Any]

class ServerNotificationCostWarning(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['cost/warning'] = Field(default='cost/warning', alias='method')
    params: CostWarningParams

class ServerNotificationSandboxStateChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['sandbox/stateChanged'] = Field(default='sandbox/stateChanged', alias='method')
    params: SandboxStateChangedParams

class ServerNotificationSandboxViolationsDetected(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['sandbox/violationsDetected'] = Field(default='sandbox/violationsDetected', alias='method')
    params: dict[str, Any]

class ServerNotificationAgentsRegistered(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['agents/registered'] = Field(default='agents/registered', alias='method')
    params: dict[str, Any]

class ServerNotificationHookStarted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['hook/started'] = Field(default='hook/started', alias='method')
    params: HookStartedParams

class ServerNotificationHookProgress(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['hook/progress'] = Field(default='hook/progress', alias='method')
    params: HookProgressParams

class ServerNotificationHookResponse(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['hook/response'] = Field(default='hook/response', alias='method')
    params: HookResponseParams

class ServerNotificationWorktreeEntered(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['worktree/entered'] = Field(default='worktree/entered', alias='method')
    params: WorktreeEnteredParams

class ServerNotificationWorktreeExited(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['worktree/exited'] = Field(default='worktree/exited', alias='method')
    params: WorktreeExitedParams

class ServerNotificationSummarizeCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['summarize/completed'] = Field(default='summarize/completed', alias='method')
    params: SummarizeCompletedParams

class ServerNotificationSummarizeFailed(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['summarize/failed'] = Field(default='summarize/failed', alias='method')
    params: dict[str, Any]

class ServerNotificationStreamStallDetected(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['stream/stallDetected'] = Field(default='stream/stallDetected', alias='method')
    params: dict[str, Any]

class ServerNotificationStreamWatchdogWarning(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['stream/watchdogWarning'] = Field(default='stream/watchdogWarning', alias='method')
    params: dict[str, Any]

class ServerNotificationStreamRequestEnd(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['stream/requestEnd'] = Field(default='stream/requestEnd', alias='method')
    params: dict[str, Any]

class ServerNotificationSessionStateChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/stateChanged'] = Field(default='session/stateChanged', alias='method')
    params: dict[str, Any]

class ServerNotificationLocalCommandOutput(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['localCommand/output'] = Field(default='localCommand/output', alias='method')
    params: LocalCommandOutputParams

class ServerNotificationFilesPersisted(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['files/persisted'] = Field(default='files/persisted', alias='method')
    params: FilesPersistedParams

class ServerNotificationElicitationComplete(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['elicitation/complete'] = Field(default='elicitation/complete', alias='method')
    params: ElicitationCompleteParams

class ServerNotificationToolUseSummary(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['tool/useSummary'] = Field(default='tool/useSummary', alias='method')
    params: ToolUseSummaryParams

class ServerNotificationToolProgress(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['tool/progress'] = Field(default='tool/progress', alias='method')
    params: ToolProgressParams

class ServerNotificationPluginsChanged(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['plugins/changed'] = Field(default='plugins/changed', alias='method')
    params: dict[str, Any]

ServerNotification = Annotated[
    Union[ServerNotificationSessionStarted, ServerNotificationSessionResult, ServerNotificationSessionEnded, ServerNotificationSessionUsageUpdated, ServerNotificationHistoryMessageAppended, ServerNotificationHistoryMessageTruncated, ServerNotificationHistoryResetForResume, ServerNotificationHistoryReplaced, ServerNotificationHistoryReasoningMetadataAttached, ServerNotificationTurnStarted, ServerNotificationTurnEnded, ServerNotificationItemStarted, ServerNotificationItemUpdated, ServerNotificationItemCompleted, ServerNotificationAgentMessageDelta, ServerNotificationReasoningDelta, ServerNotificationMcpStartupStatus, ServerNotificationMcpStartupComplete, ServerNotificationLspPrewarmComplete, ServerNotificationContextCompacted, ServerNotificationContextUsageWarning, ServerNotificationContextCompactionStarted, ServerNotificationContextCompactionPhase, ServerNotificationContextCompactionFailed, ServerNotificationContextCleared, ServerNotificationTaskStarted, ServerNotificationTaskCompleted, ServerNotificationTaskProgress, ServerNotificationTaskPanelChanged, ServerNotificationPlanApprovalRequested, ServerNotificationAgentsKilled, ServerNotificationModelFallbackStarted, ServerNotificationModelFallbackCompleted, ServerNotificationModelFastModeChanged, ServerNotificationModelRoleChanged, ServerNotificationPermissionModeChanged, ServerNotificationPromptSuggestion, ServerNotificationError, ServerNotificationRateLimit, ServerNotificationKeepAlive, ServerNotificationIdeSelectionChanged, ServerNotificationIdeDiagnosticsUpdated, ServerNotificationQueueStateChanged, ServerNotificationQueueCommandQueued, ServerNotificationQueueCommandDequeued, ServerNotificationRewindCompleted, ServerNotificationRewindFailed, ServerNotificationCostWarning, ServerNotificationSandboxStateChanged, ServerNotificationSandboxViolationsDetected, ServerNotificationAgentsRegistered, ServerNotificationHookStarted, ServerNotificationHookProgress, ServerNotificationHookResponse, ServerNotificationWorktreeEntered, ServerNotificationWorktreeExited, ServerNotificationSummarizeCompleted, ServerNotificationSummarizeFailed, ServerNotificationStreamStallDetected, ServerNotificationStreamWatchdogWarning, ServerNotificationStreamRequestEnd, ServerNotificationSessionStateChanged, ServerNotificationLocalCommandOutput, ServerNotificationFilesPersisted, ServerNotificationElicitationComplete, ServerNotificationToolUseSummary, ServerNotificationToolProgress, ServerNotificationPluginsChanged],
    Field(discriminator='method'),
]

class ServerRequestApprovalAskForApproval(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['approval/askForApproval'] = Field(default='approval/askForApproval', alias='method')
    params: AskForApprovalParams

class ServerRequestInputRequestUserInput(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['input/requestUserInput'] = Field(default='input/requestUserInput', alias='method')
    params: RequestUserInputParams

class ServerRequestMcpRouteMessage(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/routeMessage'] = Field(default='mcp/routeMessage', alias='method')
    params: McpRouteMessageParams

class ServerRequestHookCallback(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['hook/callback'] = Field(default='hook/callback', alias='method')
    params: HookCallbackParams

class ServerRequestControlCancelRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/cancelRequest'] = Field(default='control/cancelRequest', alias='method')
    params: ServerCancelRequestParams

class ServerRequestMcpRequestElicitation(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/requestElicitation'] = Field(default='mcp/requestElicitation', alias='method')
    params: RequestElicitationParams

ServerRequest = Annotated[
    Union[ServerRequestApprovalAskForApproval, ServerRequestInputRequestUserInput, ServerRequestMcpRouteMessage, ServerRequestHookCallback, ServerRequestControlCancelRequest, ServerRequestMcpRequestElicitation],
    Field(discriminator='method'),
]

class SharedV4FileDataData(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['data'] = Field(default='data', alias='type')
    data: str

class SharedV4FileDataUrl(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['url'] = Field(default='url', alias='type')
    url: str

class SharedV4FileDataReference(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['reference'] = Field(default='reference', alias='type')
    reference: dict[str, str]

class SharedV4FileDataText(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['text'] = Field(default='text', alias='type')
    text: str

SharedV4FileData = Annotated[
    Union[SharedV4FileDataData, SharedV4FileDataUrl, SharedV4FileDataReference, SharedV4FileDataText],
    Field(discriminator='type_'),
]

class SkillOverridesSaveResultOk(BaseModel):
    model_config = {"populate_by_name": True}
    outcome: Literal['ok'] = Field(default='ok', alias='outcome')

class SkillOverridesSaveResultErr(BaseModel):
    model_config = {"populate_by_name": True}
    outcome: Literal['err'] = Field(default='err', alias='outcome')
    kind: SkillOverridesSaveErrorKind
    message: str

SkillOverridesSaveResult = Annotated[
    Union[SkillOverridesSaveResultOk, SkillOverridesSaveResultErr],
    Field(discriminator='outcome'),
]

class SlashCommandStatusKindNoHandler(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['no_handler'] = Field(default='no_handler', alias='kind')

class SlashCommandStatusKindFailed(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['failed'] = Field(default='failed', alias='kind')
    error: str

class SlashCommandStatusKindEmptyPrompt(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['empty_prompt'] = Field(default='empty_prompt', alias='kind')

class SlashCommandStatusKindDialogPending(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['dialog_pending'] = Field(default='dialog_pending', alias='kind')
    dialog_kind: str

class SlashCommandStatusKindPermissionsUsageAllow(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['permissions_usage_allow'] = Field(default='permissions_usage_allow', alias='kind')

class SlashCommandStatusKindPermissionsUsageDeny(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['permissions_usage_deny'] = Field(default='permissions_usage_deny', alias='kind')

SlashCommandStatusKind = Annotated[
    Union[SlashCommandStatusKindNoHandler, SlashCommandStatusKindFailed, SlashCommandStatusKindEmptyPrompt, SlashCommandStatusKindDialogPending, SlashCommandStatusKindPermissionsUsageAllow, SlashCommandStatusKindPermissionsUsageDeny],
    Field(discriminator='kind'),
]

class ToolAbortReasonPayloadTurn(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['turn'] = Field(default='turn', alias='kind')
    reason: TurnAbortReason

class ToolAbortReasonPayloadSelfAbort(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['self_abort'] = Field(default='self_abort', alias='kind')
    message: str

class ToolAbortReasonPayloadSiblingError(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['sibling_error'] = Field(default='sibling_error', alias='kind')
    failed_tool: str

ToolAbortReasonPayload = Annotated[
    Union[ToolAbortReasonPayloadTurn, ToolAbortReasonPayloadSelfAbort, ToolAbortReasonPayloadSiblingError],
    Field(discriminator='kind'),
]

class ToolDisplayDataApplyPatchPreview(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['apply_patch_preview'] = Field(default='apply_patch_preview', alias='kind')
    data: ApplyPatchPreview

class ToolDisplayDataAskUserQuestionResult(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['ask_user_question_result'] = Field(default='ask_user_question_result', alias='kind')
    data: AskUserQuestionResult

class ToolDisplayDataExitPlanModeResult(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['exit_plan_mode_result'] = Field(default='exit_plan_mode_result', alias='kind')
    data: ExitPlanModeResult

ToolDisplayData = Annotated[
    Union[ToolDisplayDataApplyPatchPreview, ToolDisplayDataAskUserQuestionResult, ToolDisplayDataExitPlanModeResult],
    Field(discriminator='kind'),
]

class ToolInputInvalidReasonJsonParseFailed(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['json_parse_failed'] = Field(default='json_parse_failed', alias='kind')
    error: str
    raw: str

class ToolInputInvalidReasonSchemaViolation(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['schema_violation'] = Field(default='schema_violation', alias='kind')
    message: str

class ToolInputInvalidReasonNoSuchTool(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['no_such_tool'] = Field(default='no_such_tool', alias='kind')
    tool_name: str

ToolInputInvalidReason = Annotated[
    Union[ToolInputInvalidReasonJsonParseFailed, ToolInputInvalidReasonSchemaViolation, ToolInputInvalidReasonNoSuchTool],
    Field(discriminator='kind'),
]

class ToolResultContentText(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['text'] = Field(default='text', alias='type')
    value: str
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentJson(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['json'] = Field(default='json', alias='type')
    value: Any
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentExecutionDenied(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['execution-denied'] = Field(default='execution-denied', alias='type')
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')
    reason: str | None = None

class ToolResultContentErrorText(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['error-text'] = Field(default='error-text', alias='type')
    value: str
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentErrorJson(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['error-json'] = Field(default='error-json', alias='type')
    value: Any
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentContent(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['content'] = Field(default='content', alias='type')
    value: list[ToolResultContentPart]
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

ToolResultContent = Annotated[
    Union[ToolResultContentText, ToolResultContentJson, ToolResultContentExecutionDenied, ToolResultContentErrorText, ToolResultContentErrorJson, ToolResultContentContent],
    Field(discriminator='type_'),
]

class ToolResultContentPartText(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['text'] = Field(default='text', alias='type')
    text: str
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentPartFileData(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['file-data'] = Field(default='file-data', alias='type')
    data: str
    media_type: str = Field(alias='mediaType')
    filename: str | None = None
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentPartFileUrl(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['file-url'] = Field(default='file-url', alias='type')
    media_type: str = Field(alias='mediaType')
    url: str
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentPartFileReference(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['file-reference'] = Field(default='file-reference', alias='type')
    provider_reference: dict[str, str] = Field(alias='providerReference')
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class ToolResultContentPartCustom(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['custom'] = Field(default='custom', alias='type')
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

ToolResultContentPart = Annotated[
    Union[ToolResultContentPartText, ToolResultContentPartFileData, ToolResultContentPartFileUrl, ToolResultContentPartFileReference, ToolResultContentPartCustom],
    Field(discriminator='type_'),
]

class TuiOnlyEventApprovalRequired(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['approval_required'] = Field(default='approval_required', alias='type')
    description: str
    display_input: PermissionDisplayInput
    request_id: str
    tool_name: str
    choices: list[PermissionAskChoice] | None = None
    cwd: str | None = None
    detail: PermissionRequestDetail | None = None
    original_input: dict[str, Any] | None = None
    permission_suggestions: list[PermissionUpdate] | None = None
    show_always_allow: bool = False
    worker_badge: WorkerBadge | None = None

class TuiOnlyEventQuestionAsked(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['question_asked'] = Field(default='question_asked', alias='type')
    input: Any
    request_id: str

class TuiOnlyEventElicitationRequested(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['elicitation_requested'] = Field(default='elicitation_requested', alias='type')
    request_id: str
    schema_: Any = Field(alias='schema')
    server: str

class TuiOnlyEventSandboxApprovalRequired(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['sandbox_approval_required'] = Field(default='sandbox_approval_required', alias='type')
    operation: str
    request_id: str

class TuiOnlyEventPermissionExplanationReady(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['permission_explanation_ready'] = Field(default='permission_explanation_ready', alias='type')
    request_id: str
    explanation: PermissionExplanation | None = None

class TuiOnlyEventPluginDataReady(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['plugin_data_ready'] = Field(default='plugin_data_ready', alias='type')
    plugins: list[Any]

class TuiOnlyEventOutputStylesReady(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['output_styles_ready'] = Field(default='output_styles_ready', alias='type')
    styles: list[str]

class TuiOnlyEventAvailableCommandsRefreshed(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['available_commands_refreshed'] = Field(default='available_commands_refreshed', alias='type')
    commands: list[SlashCommandInfo]

class TuiOnlyEventQueuedCommandEditReady(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['queued_command_edit_ready'] = Field(default='queued_command_edit_ready', alias='type')
    id: str
    prompt: str
    images: list[QueuedCommandEditImage] | None = None

class TuiOnlyEventQueuedCommandsEditReady(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['queued_commands_edit_ready'] = Field(default='queued_commands_edit_ready', alias='type')
    cursor: int
    ids: list[str]
    prompt: str
    images: list[QueuedCommandEditImage] | None = None

class TuiOnlyEventQueuedCommandEditUnavailable(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['queued_command_edit_unavailable'] = Field(default='queued_command_edit_unavailable', alias='type')
    id: str
    reason: str

class TuiOnlyEventOpenSessionBrowser(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_session_browser'] = Field(default='open_session_browser', alias='type')
    sessions: list[SdkSessionSummary]

class TuiOnlyEventRewindRowMetadataReady(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['rewind_row_metadata_ready'] = Field(default='rewind_row_metadata_ready', alias='type')
    rows: list[RewindRowMetadata]

class TuiOnlyEventRewindRestorePreviewReady(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['rewind_restore_preview_ready'] = Field(default='rewind_restore_preview_ready', alias='type')
    message_id: str
    stats: RewindDiffStatsPayload | None = None

class TuiOnlyEventCompactionCircuitBreakerOpen(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['compaction_circuit_breaker_open'] = Field(default='compaction_circuit_breaker_open', alias='type')
    failures: int

class TuiOnlyEventMicroCompactionApplied(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['micro_compaction_applied'] = Field(default='micro_compaction_applied', alias='type')
    removed: int

class TuiOnlyEventSessionMemoryCompactApplied(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['session_memory_compact_applied'] = Field(default='session_memory_compact_applied', alias='type')
    summary_tokens: int

class TuiOnlyEventSpeculativeRolledBack(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['speculative_rolled_back'] = Field(default='speculative_rolled_back', alias='type')
    reason: str

class TuiOnlyEventSessionMemoryExtractionStarted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['session_memory_extraction_started'] = Field(default='session_memory_extraction_started', alias='type')

class TuiOnlyEventSessionMemoryExtractionCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['session_memory_extraction_completed'] = Field(default='session_memory_extraction_completed', alias='type')
    extracted: int

class TuiOnlyEventSessionMemoryExtractionFailed(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['session_memory_extraction_failed'] = Field(default='session_memory_extraction_failed', alias='type')
    error: str

class TuiOnlyEventCronJobDisabled(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['cron_job_disabled'] = Field(default='cron_job_disabled', alias='type')
    job_id: str
    reason: str

class TuiOnlyEventCronJobsMissed(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['cron_jobs_missed'] = Field(default='cron_jobs_missed', alias='type')
    count: int

class TuiOnlyEventToolCallStreamStart(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_call_stream_start'] = Field(default='tool_call_stream_start', alias='type')
    call_id: str
    name: str

class TuiOnlyEventToolCallDelta(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_call_delta'] = Field(default='tool_call_delta', alias='type')
    call_id: str
    delta: str

class TuiOnlyEventToolProgress(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_progress'] = Field(default='tool_progress', alias='type')
    data: Any
    tool_use_id: str

class TuiOnlyEventToolInterruptibilityChanged(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_interruptibility_changed'] = Field(default='tool_interruptibility_changed', alias='type')
    interruptible: bool

class TuiOnlyEventToolExecutionAborted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['tool_execution_aborted'] = Field(default='tool_execution_aborted', alias='type')
    reason: ToolAbortReasonPayload
    tool_use_id: str

class TuiOnlyEventRewindCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['rewind_completed'] = Field(default='rewind_completed', alias='type')
    files_changed: int
    target_message_id: str

class TuiOnlyEventSlashCommandResult(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['slash_command_result'] = Field(default='slash_command_result', alias='type')
    args: str
    name: str
    text: str

class TuiOnlyEventOpenContextUsage(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_context_usage'] = Field(default='open_context_usage', alias='type')
    result: ContextUsageResult

class TuiOnlyEventSlashCommandStatus(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['slash_command_status'] = Field(default='slash_command_status', alias='type')
    args: str
    kind: SlashCommandStatusKind
    name: str

class TuiOnlyEventOpenRewindPicker(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_rewind_picker'] = Field(default='open_rewind_picker', alias='type')

class TuiOnlyEventOpenMemoryDialog(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_memory_dialog'] = Field(default='open_memory_dialog', alias='type')
    entries: list[MemoryDialogEntry]

class TuiOnlyEventCopyCommandRequested(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['copy_command_requested'] = Field(default='copy_command_requested', alias='type')
    args: str

class TuiOnlyEventMemoryFileOpened(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['memory_file_opened'] = Field(default='memory_file_opened', alias='type')
    path: str

class TuiOnlyEventMemoryFileOpenFailed(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['memory_file_open_failed'] = Field(default='memory_file_open_failed', alias='type')
    error: str
    path: str

class TuiOnlyEventPlanFileOpened(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['plan_file_opened'] = Field(default='plan_file_opened', alias='type')
    path: str

class TuiOnlyEventPlanFileOpenFailed(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['plan_file_open_failed'] = Field(default='plan_file_open_failed', alias='type')
    error: str
    path: str

class TuiOnlyEventExternalEditorPrepare(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['external_editor_prepare'] = Field(default='external_editor_prepare', alias='type')
    request_id: str

class TuiOnlyEventPromptEditorCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['prompt_editor_completed'] = Field(default='prompt_editor_completed', alias='type')
    content: str
    modified: bool

class TuiOnlyEventPromptEditorFailed(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['prompt_editor_failed'] = Field(default='prompt_editor_failed', alias='type')
    error: str

class TuiOnlyEventBashCommandCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['bash_command_completed'] = Field(default='bash_command_completed', alias='type')
    exit_code: int
    output: str
    user_message_id: str

class TuiOnlyEventOpenModelPicker(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_model_picker'] = Field(default='open_model_picker', alias='type')

class TuiOnlyEventOpenSettings(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_settings'] = Field(default='open_settings', alias='type')

class TuiOnlyEventOpenThemePicker(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_theme_picker'] = Field(default='open_theme_picker', alias='type')

class TuiOnlyEventOpenSkillsDialog(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_skills_dialog'] = Field(default='open_skills_dialog', alias='type')
    payload: SkillsDialogPayload

class TuiOnlyEventOpenPluginDialog(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_plugin_dialog'] = Field(default='open_plugin_dialog', alias='type')
    payload: PluginDialogPayload

class TuiOnlyEventOpenAgentsDialog(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_agents_dialog'] = Field(default='open_agents_dialog', alias='type')
    payload: AgentsDialogPayload

class TuiOnlyEventOpenPermissionsEditor(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_permissions_editor'] = Field(default='open_permissions_editor', alias='type')
    payload: PermissionsEditorPayload

class TuiOnlyEventOpenAddDirectory(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_add_directory'] = Field(default='open_add_directory', alias='type')

class TuiOnlyEventOpenExport(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['open_export'] = Field(default='open_export', alias='type')

class TuiOnlyEventSkillOverridesSaved(BaseModel):
    model_config = {"populate_by_name": True}
    type_: Literal['skill_overrides_saved'] = Field(default='skill_overrides_saved', alias='type')
    result: SkillOverridesSaveResult

TuiOnlyEvent = Annotated[
    Union[TuiOnlyEventApprovalRequired, TuiOnlyEventQuestionAsked, TuiOnlyEventElicitationRequested, TuiOnlyEventSandboxApprovalRequired, TuiOnlyEventPermissionExplanationReady, TuiOnlyEventPluginDataReady, TuiOnlyEventOutputStylesReady, TuiOnlyEventAvailableCommandsRefreshed, TuiOnlyEventQueuedCommandEditReady, TuiOnlyEventQueuedCommandsEditReady, TuiOnlyEventQueuedCommandEditUnavailable, TuiOnlyEventOpenSessionBrowser, TuiOnlyEventRewindRowMetadataReady, TuiOnlyEventRewindRestorePreviewReady, TuiOnlyEventCompactionCircuitBreakerOpen, TuiOnlyEventMicroCompactionApplied, TuiOnlyEventSessionMemoryCompactApplied, TuiOnlyEventSpeculativeRolledBack, TuiOnlyEventSessionMemoryExtractionStarted, TuiOnlyEventSessionMemoryExtractionCompleted, TuiOnlyEventSessionMemoryExtractionFailed, TuiOnlyEventCronJobDisabled, TuiOnlyEventCronJobsMissed, TuiOnlyEventToolCallStreamStart, TuiOnlyEventToolCallDelta, TuiOnlyEventToolProgress, TuiOnlyEventToolInterruptibilityChanged, TuiOnlyEventToolExecutionAborted, TuiOnlyEventRewindCompleted, TuiOnlyEventSlashCommandResult, TuiOnlyEventOpenContextUsage, TuiOnlyEventSlashCommandStatus, TuiOnlyEventOpenRewindPicker, TuiOnlyEventOpenMemoryDialog, TuiOnlyEventCopyCommandRequested, TuiOnlyEventMemoryFileOpened, TuiOnlyEventMemoryFileOpenFailed, TuiOnlyEventPlanFileOpened, TuiOnlyEventPlanFileOpenFailed, TuiOnlyEventExternalEditorPrepare, TuiOnlyEventPromptEditorCompleted, TuiOnlyEventPromptEditorFailed, TuiOnlyEventBashCommandCompleted, TuiOnlyEventOpenModelPicker, TuiOnlyEventOpenSettings, TuiOnlyEventOpenThemePicker, TuiOnlyEventOpenSkillsDialog, TuiOnlyEventOpenPluginDialog, TuiOnlyEventOpenAgentsDialog, TuiOnlyEventOpenPermissionsEditor, TuiOnlyEventOpenAddDirectory, TuiOnlyEventOpenExport, TuiOnlyEventSkillOverridesSaved],
    Field(discriminator='type_'),
]

class TurnOutcomeCompleted(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['completed'] = Field(default='completed', alias='kind')
    data: CompletedOutcome

class TurnOutcomeFailed(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['failed'] = Field(default='failed', alias='kind')
    data: FailedOutcome

class TurnOutcomeInterrupted(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['interrupted'] = Field(default='interrupted', alias='kind')
    data: InterruptedOutcome

class TurnOutcomeMaxTurnsReached(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['max_turns_reached'] = Field(default='max_turns_reached', alias='kind')
    data: MaxTurnsReachedOutcome

class TurnOutcomeBudgetExhausted(BaseModel):
    model_config = {"populate_by_name": True}
    kind: Literal['budget_exhausted'] = Field(default='budget_exhausted', alias='kind')
    data: BudgetExhaustedOutcome

TurnOutcome = Annotated[
    Union[TurnOutcomeCompleted, TurnOutcomeFailed, TurnOutcomeInterrupted, TurnOutcomeMaxTurnsReached, TurnOutcomeBudgetExhausted],
    Field(discriminator='kind'),
]


# ---------------------------------------------------------------------------
# Item types
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# ThreadItem (tagged union with extra fields)
# ---------------------------------------------------------------------------

class ThreadItem(BaseModel):
    """A discrete operation within a turn."""

    id: str
    type: str
    model_config = {"extra": "allow"}

    def as_agent_message(self) -> AgentMessageItem | None:
        if self.type == 'agent_message':
            return AgentMessageItem.model_validate(self.model_extra or {})
        return None

    def as_reasoning(self) -> ReasoningItem | None:
        if self.type == 'reasoning':
            return ReasoningItem.model_validate(self.model_extra or {})
        return None

    def as_command_execution(self) -> CommandExecutionItem | None:
        if self.type == 'command_execution':
            return CommandExecutionItem.model_validate(self.model_extra or {})
        return None

    def as_file_change(self) -> FileChangeItem | None:
        if self.type == 'file_change':
            return FileChangeItem.model_validate(self.model_extra or {})
        return None

    def as_mcp_tool_call(self) -> McpToolCallItem | None:
        if self.type == 'mcp_tool_call':
            return McpToolCallItem.model_validate(self.model_extra or {})
        return None

    def as_web_search(self) -> WebSearchItem | None:
        if self.type == 'web_search':
            return WebSearchItem.model_validate(self.model_extra or {})
        return None

    def as_subagent(self) -> SubagentItem | None:
        if self.type == 'subagent':
            return SubagentItem.model_validate(self.model_extra or {})
        return None

    def as_tool_call(self) -> GenericToolCallItem | None:
        if self.type == 'tool_call':
            return GenericToolCallItem.model_validate(self.model_extra or {})
        return None

    def as_error_item(self) -> ErrorItem | None:
        if self.type == 'error':
            return ErrorItem.model_validate(self.model_extra or {})
        return None


# ---------------------------------------------------------------------------
# Server notification params
# ---------------------------------------------------------------------------

class AgentsKilledParams(BaseModel):
    count: int
    agent_ids: list[str] = []

class CompactionFailedParams(BaseModel):
    error: str
    attempts: int = 0

class CompactionPhaseParams(BaseModel):
    phase: CompactionPhase
    hook_type: CompactionHookType | None = None

class ContentDeltaParams(BaseModel):
    delta: str
    item_id: str | None = None
    turn_id: str | None = None

class ContextClearedParams(BaseModel):
    new_mode: str | None = None

class ContextCompactedParams(BaseModel):
    removed_messages: int
    summary_tokens: int
    post_tokens: int | None = None
    pre_tokens: int | None = None
    trigger: CompactTrigger = 'auto'

class ContextUsageWarningParams(BaseModel):
    estimated_tokens: int
    percent_left: float
    warning_threshold: int

class CostWarningParams(BaseModel):
    current_cost_cents: int
    threshold_cents: int
    budget_cents: int | None = None

class ElicitationCompleteParams(BaseModel):
    elicitation_id: str
    mcp_server_name: str

class ErrorParams(BaseModel):
    message: str
    category: str | None = None
    retryable: bool = False

class FilesPersistedParams(BaseModel):
    files: list[PersistedFileInfo]
    processed_at: str
    failed: list[PersistedFileError] = []

class HookProgressParams(BaseModel):
    hook_event: str
    hook_id: str
    hook_name: str
    output: str = ''
    stderr: str = ''
    stdout: str = ''

class HookResponseParams(BaseModel):
    hook_event: str
    hook_id: str
    hook_name: str
    outcome: HookOutcomeStatus
    output: str
    exit_code: int | None = None
    stderr: str = ''
    stdout: str = ''

class HookStartedParams(BaseModel):
    hook_event: str
    hook_id: str
    hook_name: str

class IdeDiagnosticsUpdatedParams(BaseModel):
    file_path: str
    new_count: int
    diagnostics: list[Any] = []

class IdeSelectionChangedParams(BaseModel):
    end_line: int
    file_path: str
    selected_text: str
    start_line: int

class LocalCommandOutputParams(BaseModel):
    content: dict[str, Any]

class LspPrewarmCompleteParams(BaseModel):
    root: str
    started: list[str]

class McpStartupCompleteParams(BaseModel):
    servers: list[str]
    failed: list[str] = []

class McpStartupStatusParams(BaseModel):
    server: str
    status: McpConnectionStatus

class ModelFallbackParams(BaseModel):
    from_model: str
    reason: str
    to_model: str

class ModelRoleChangedParams(BaseModel):
    model_id: str
    provider: str
    role: ModelRole
    context_window: int | None = None
    effort: ReasoningEffort | None = None

class PermissionModeChangedParams(BaseModel):
    mode: PermissionMode
    bypass_available: bool = False

class PlanApprovalRequestedParams(BaseModel):
    from_: str = Field(alias='from')
    plan_content: str
    request_id: str
    plan_file_path: str | None = None

class RateLimitParams(BaseModel):
    limit: int | None = None
    provider: str | None = None
    rate_limit_type: str | None = None
    remaining: int | None = None
    reset_at: int | None = None
    status: RateLimitStatus | None = None
    utilization: float | None = None

class ReasoningMetadataAttachedParams(BaseModel):
    message_uuid: str
    reasoning_tokens: int
    duration_ms: int | None = None

class RewindCompletedParams(BaseModel):
    messages_removed: int
    restored_files: int
    rewound_turn: int

class SandboxStateChangedParams(BaseModel):
    active: bool
    enforcement: str

class SessionEndedParams(BaseModel):
    reason: str

class SessionResultParams(BaseModel):
    duration_api_ms: int
    duration_ms: int
    session_id: str
    stop_reason: str
    total_cost_usd: float
    total_turns: int
    usage: TokenUsage
    errors: list[str] | None = None
    fast_mode_state: FastModeState | None = None
    is_error: bool = False
    model_usage: dict[str, SessionModelUsage] = {}
    num_api_calls: int | None = None
    permission_denials: list[PermissionDenialInfo] = []
    result: str | None = None
    structured_output: Any = None

class SessionStartedParams(BaseModel):
    cwd: str
    model: str
    permission_mode: str
    protocol_version: str
    session_id: str
    version: str
    agents: list[str] = []
    api_key_source: str | None = None
    betas: list[str] | None = None
    fast_mode_state: FastModeState | None = None
    lsp_active: bool = False
    mcp_servers: list[McpServerInit] = []
    output_style: str | None = None
    plugins: list[PluginInit] = []
    provider: str | None = None
    skills: list[str] = []
    slash_commands: list[str] = []
    tools: list[str] = []

class SessionUsageSnapshot(BaseModel):
    session_id: str
    totals: SessionUsageTotals
    updated_at_ms: int
    version: int
    models: list[SessionModelUsageEntry] | None = None
    unpriced_models: list[ProviderModelSelection] | None = None

class SummarizeCompletedParams(BaseModel):
    from_turn: int
    summary_tokens: int

class TaskCompletedParams(BaseModel):
    output_file: str
    status: TaskCompletionStatus
    summary: str
    task_id: str
    tool_use_id: str | None = None
    usage: TaskUsage | None = None

class TaskPanelChangedParams(BaseModel):
    expanded_view: ExpandedView
    plan_tasks: list[TaskRecord]
    verification_nudge_pending: bool
    todos_by_agent: dict[str, list[TodoRecord]] = {}

class TaskProgressParams(BaseModel):
    description: str
    task_id: str
    usage: TaskUsage
    agent_type: str | None = None
    last_tool_name: str | None = None
    recent_activities: list[TaskActivity] | None = None
    summary: str | None = None
    tool_use_id: str | None = None
    workflow_progress: list[Any] | None = None

class TaskStartedParams(BaseModel):
    description: str
    task_id: str
    agent_name: str | None = None
    backend_kind: str | None = None
    color: str | None = None
    prompt: str | None = None
    task_type: str | None = None
    team_name: str | None = None
    tool_use_id: str | None = None
    workflow_name: str | None = None

class ToolProgressParams(BaseModel):
    elapsed_time_seconds: float
    tool_name: str
    tool_use_id: str
    parent_tool_use_id: str | None = None
    task_id: str | None = None

class ToolUseSummaryParams(BaseModel):
    preceding_tool_use_ids: list[str]
    summary: str

class TurnEndedParams(BaseModel):
    outcome: TurnOutcome
    turn_id: TurnId
    usage: TokenUsage | None = None

class TurnStartedParams(BaseModel):
    turn_id: TurnId

class WorktreeEnteredParams(BaseModel):
    branch: str
    worktree_path: str

class WorktreeExitedParams(BaseModel):
    action: str
    worktree_path: str

class HistoryMessageAppendedParams(BaseModel):
    message: Message
    agent_id: str | None = None
    session_id: str = ''

class HistoryMessageTruncatedParams(BaseModel):
    keep_count: int
    agent_id: str | None = None
    session_id: str = ''

class HistoryResetForResumeParams(BaseModel):
    session_id: str
    agent_id: str | None = None

class HistoryReplacedParams(BaseModel):
    messages: list[Message]
    agent_id: str | None = None
    session_id: str = ''

class ItemStartedParams(BaseModel):
    item: ThreadItem

class ItemUpdatedParams(BaseModel):
    item: ThreadItem

class ItemCompletedParams(BaseModel):
    item: ThreadItem

class ContextCompactionStartedParams(BaseModel):
    """Empty params for the wire-method `context/compactionStarted`."""

    model_config = {"extra": "allow"}

class ModelFallbackCompletedParams(BaseModel):
    """Empty params for the wire-method `model/fallbackCompleted`."""

    model_config = {"extra": "allow"}

class ModelFastModeChangedParams(BaseModel):
    active: bool

class PromptSuggestionParams(BaseModel):
    suggestions: list[str]

class KeepAliveNotifParams(BaseModel):
    timestamp: int

class QueueStateChangedParams(BaseModel):
    queued: int

class QueueCommandQueuedParams(BaseModel):
    editable: bool
    id: str
    preview: str

class QueueCommandDequeuedParams(BaseModel):
    id: str

class RewindFailedParams(BaseModel):
    error: str

class SandboxViolationsDetectedParams(BaseModel):
    count: int

class AgentsRegisteredParams(BaseModel):
    agents: list[AgentInfo]

class SummarizeFailedParams(BaseModel):
    error: str

class StreamStallDetectedParams(BaseModel):
    turn_id: str | None = None

class StreamWatchdogWarningParams(BaseModel):
    elapsed_secs: float

class StreamRequestEndParams(BaseModel):
    usage: TokenUsage

class SessionStateChangedParams(BaseModel):
    state: SessionState

class PluginsChangedParams(BaseModel):
    reason: str


# ---------------------------------------------------------------------------
# Notification wire-method constants
# ---------------------------------------------------------------------------

class NotificationMethod(str, Enum):
    """Wire-method identifier for every `ServerNotification` variant. Mirrors the Rust `NotificationMethod` enum. Members inherit from `str`, so equality with raw wire strings Just Works."""

    SESSION_STARTED = 'session/started'
    SESSION_RESULT = 'session/result'
    SESSION_ENDED = 'session/ended'
    SESSION_USAGE_UPDATED = 'session/usageUpdated'
    HISTORY_MESSAGE_APPENDED = 'history/messageAppended'
    HISTORY_MESSAGE_TRUNCATED = 'history/messageTruncated'
    HISTORY_RESET_FOR_RESUME = 'history/resetForResume'
    HISTORY_REPLACED = 'history/replaced'
    HISTORY_REASONING_METADATA_ATTACHED = 'history/reasoningMetadataAttached'
    TURN_STARTED = 'turn/started'
    TURN_ENDED = 'turn/ended'
    ITEM_STARTED = 'item/started'
    ITEM_UPDATED = 'item/updated'
    ITEM_COMPLETED = 'item/completed'
    AGENT_MESSAGE_DELTA = 'agentMessage/delta'
    REASONING_DELTA = 'reasoning/delta'
    MCP_STARTUP_STATUS = 'mcp/startupStatus'
    MCP_STARTUP_COMPLETE = 'mcp/startupComplete'
    LSP_PREWARM_COMPLETE = 'lsp/prewarmComplete'
    CONTEXT_COMPACTED = 'context/compacted'
    CONTEXT_USAGE_WARNING = 'context/usageWarning'
    CONTEXT_COMPACTION_STARTED = 'context/compactionStarted'
    CONTEXT_COMPACTION_PHASE = 'context/compactionPhase'
    CONTEXT_COMPACTION_FAILED = 'context/compactionFailed'
    CONTEXT_CLEARED = 'context/cleared'
    TASK_STARTED = 'task/started'
    TASK_COMPLETED = 'task/completed'
    TASK_PROGRESS = 'task/progress'
    TASK_PANEL_CHANGED = 'task_panel/changed'
    PLAN_APPROVAL_REQUESTED = 'plan_approval/requested'
    AGENTS_KILLED = 'agents/killed'
    MODEL_FALLBACK_STARTED = 'model/fallbackStarted'
    MODEL_FALLBACK_COMPLETED = 'model/fallbackCompleted'
    MODEL_FAST_MODE_CHANGED = 'model/fastModeChanged'
    MODEL_ROLE_CHANGED = 'model/roleChanged'
    PERMISSION_MODE_CHANGED = 'permission/modeChanged'
    PROMPT_SUGGESTION = 'prompt/suggestion'
    ERROR = 'error'
    RATE_LIMIT = 'rateLimit'
    KEEP_ALIVE = 'keepAlive'
    IDE_SELECTION_CHANGED = 'ide/selectionChanged'
    IDE_DIAGNOSTICS_UPDATED = 'ide/diagnosticsUpdated'
    QUEUE_STATE_CHANGED = 'queue/stateChanged'
    QUEUE_COMMAND_QUEUED = 'queue/commandQueued'
    QUEUE_COMMAND_DEQUEUED = 'queue/commandDequeued'
    REWIND_COMPLETED = 'rewind/completed'
    REWIND_FAILED = 'rewind/failed'
    COST_WARNING = 'cost/warning'
    SANDBOX_STATE_CHANGED = 'sandbox/stateChanged'
    SANDBOX_VIOLATIONS_DETECTED = 'sandbox/violationsDetected'
    AGENTS_REGISTERED = 'agents/registered'
    HOOK_STARTED = 'hook/started'
    HOOK_PROGRESS = 'hook/progress'
    HOOK_RESPONSE = 'hook/response'
    WORKTREE_ENTERED = 'worktree/entered'
    WORKTREE_EXITED = 'worktree/exited'
    SUMMARIZE_COMPLETED = 'summarize/completed'
    SUMMARIZE_FAILED = 'summarize/failed'
    STREAM_STALL_DETECTED = 'stream/stallDetected'
    STREAM_WATCHDOG_WARNING = 'stream/watchdogWarning'
    STREAM_REQUEST_END = 'stream/requestEnd'
    SESSION_STATE_CHANGED = 'session/stateChanged'
    LOCAL_COMMAND_OUTPUT = 'localCommand/output'
    FILES_PERSISTED = 'files/persisted'
    ELICITATION_COMPLETE = 'elicitation/complete'
    TOOL_USE_SUMMARY = 'tool/useSummary'
    TOOL_PROGRESS = 'tool/progress'
    PLUGINS_CHANGED = 'plugins/changed'


# ---------------------------------------------------------------------------
# Server request param types
# ---------------------------------------------------------------------------

class AskForApprovalParams(BaseModel):
    input: Any
    request_id: str
    tool_name: str
    tool_use_id: str
    agent_id: str | None = None
    blocked_path: str | None = None
    cwd: str | None = None
    decision_reason: str | None = None
    description: str | None = None
    display_name: str | None = None
    permission_suggestions: list[Any] | None = None
    title: str | None = None

class HookCallbackParams(BaseModel):
    callback_id: str
    event_type: HookEventType
    input: HookInput
    tool_use_id: str | None = None

class McpRouteMessageParams(BaseModel):
    message: Any
    server_name: str

class RequestElicitationParams(BaseModel):
    elicitation: Any
    mcp_server_name: str
    request_id: str

class RequestUserInputParams(BaseModel):
    prompt: str
    request_id: str
    choices: list[str] | None = None
    default: str | None = None
    description: str | None = None

class ServerCancelRequestParams(BaseModel):
    request_id: str
    reason: str | None = None


class ServerRequestMethod(str, Enum):
    """Wire-method identifier for every `ServerRequest` variant. Mirrors the Rust `ServerRequestMethod` enum."""

    APPROVAL_ASK_FOR_APPROVAL = 'approval/askForApproval'
    INPUT_REQUEST_USER_INPUT = 'input/requestUserInput'
    MCP_ROUTE_MESSAGE = 'mcp/routeMessage'
    HOOK_CALLBACK = 'hook/callback'
    CONTROL_CANCEL_REQUEST = 'control/cancelRequest'
    MCP_REQUEST_ELICITATION = 'mcp/requestElicitation'


# ---------------------------------------------------------------------------
# MCP server config types
# ---------------------------------------------------------------------------

class StdioMcpServerConfig(BaseModel):
    """Subprocess-based MCP server (stdio transport)."""

    type: str = "stdio"
    command: str
    args: list[str] = []
    env: dict[str, str] | None = None


class SseMcpServerConfig(BaseModel):
    """SSE-based MCP server."""

    type: str = "sse"
    url: str


class HttpMcpServerConfig(BaseModel):
    """HTTP-based MCP server."""

    type: str = "http"
    url: str


McpServerConfig = StdioMcpServerConfig | SseMcpServerConfig | HttpMcpServerConfig

# ---------------------------------------------------------------------------
# Config types
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Hook input/output types
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# Client request params
# ---------------------------------------------------------------------------

class AgentInterruptCurrentWorkParams(BaseModel):
    agent_id: str

class ApprovalResolveParams(BaseModel):
    decision: ApprovalDecision
    request_id: str
    content_blocks: list[Any] | None = None
    feedback: str | None = None
    permission_update: PermissionUpdate | None = None
    updated_input: dict[str, Any] | None = None

class CancelRequestParams(BaseModel):
    request_id: str
    reason: str | None = None

class ConfigApplyFlagsParams(BaseModel):
    settings: dict[str, Any]

class ConfigWriteParams(BaseModel):
    key: str
    value: Any
    scope: str | None = None

class ElicitationResolveParams(BaseModel):
    approved: bool
    mcp_server_name: str
    request_id: str
    values: dict[str, Any] = {}

class InitializeParams(BaseModel):
    agent_progress_summaries: bool | None = None
    agents: dict[str, SdkAgentDefinition] | None = None
    append_system_prompt: str | None = None
    hooks: dict[str, list[HookCallbackMatcher]] | None = None
    json_schema: Any = None
    prompt_suggestions: bool | None = None
    sdk_mcp_servers: list[str] | None = None
    system_prompt: str | None = None

class McpReconnectParams(BaseModel):
    server_name: str

class McpSetServersParams(BaseModel):
    servers: dict[str, Any]

class McpToggleParams(BaseModel):
    enabled: bool
    server_name: str

class RewindFilesParams(BaseModel):
    user_message_id: str
    dry_run: bool = False

class SessionArchiveParams(BaseModel):
    session_id: str

class SessionReadParams(BaseModel):
    session_id: str
    cursor: str | None = None
    limit: int | None = None

class SessionResumeParams(BaseModel):
    session_id: str

class SessionStartParams(BaseModel):
    append_system_prompt: str | None = None
    cwd: str | None = None
    initial_prompt: str | None = None
    max_budget_usd: float | None = None
    max_turns: int | None = None
    model: str | None = None
    permission_mode: PermissionMode | None = None
    system_prompt: str | None = None

class SetModelParams(BaseModel):
    model: str | None = None

class SetPermissionModeParams(BaseModel):
    mode: PermissionMode

class SetThinkingParams(BaseModel):
    thinking_level: ThinkingLevel | None = None

class StopTaskParams(BaseModel):
    task_id: str

class TurnStartParams(BaseModel):
    prompt: str
    permission_mode: PermissionMode | None = None
    thinking_level: ThinkingLevel | None = None

class UpdateEnvParams(BaseModel):
    env: dict[str, str]

class UserInputResolveParams(BaseModel):
    answer: str
    request_id: str


# ---------------------------------------------------------------------------
# Client request wire-method constants
# ---------------------------------------------------------------------------

class ClientRequestMethod(str, Enum):
    """Wire-method identifier for every `ClientRequest` variant. Mirrors the Rust `ClientRequestMethod` enum."""

    INITIALIZE = 'initialize'
    SESSION_START = 'session/start'
    SESSION_RESUME = 'session/resume'
    SESSION_LIST = 'session/list'
    SESSION_READ = 'session/read'
    SESSION_ARCHIVE = 'session/archive'
    TURN_START = 'turn/start'
    TURN_INTERRUPT = 'turn/interrupt'
    APPROVAL_RESOLVE = 'approval/resolve'
    INPUT_RESOLVE_USER_INPUT = 'input/resolveUserInput'
    ELICITATION_RESOLVE = 'elicitation/resolve'
    CONTROL_SET_MODEL = 'control/setModel'
    CONTROL_SET_PERMISSION_MODE = 'control/setPermissionMode'
    CONTROL_SET_THINKING = 'control/setThinking'
    CONTROL_STOP_TASK = 'control/stopTask'
    CONTROL_REWIND_FILES = 'control/rewindFiles'
    CONTROL_UPDATE_ENV = 'control/updateEnv'
    CONTROL_KEEP_ALIVE = 'control/keepAlive'
    CONTROL_CANCEL_REQUEST = 'control/cancelRequest'
    AGENT_INTERRUPT_CURRENT_WORK = 'agent/interruptCurrentWork'
    CONFIG_READ = 'config/read'
    CONFIG_VALUE_WRITE = 'config/value/write'
    MCP_STATUS = 'mcp/status'
    CONTEXT_USAGE = 'context/usage'
    MCP_SET_SERVERS = 'mcp/setServers'
    MCP_RECONNECT = 'mcp/reconnect'
    MCP_TOGGLE = 'mcp/toggle'
    PLUGIN_RELOAD = 'plugin/reload'
    CONFIG_APPLY_FLAGS = 'config/applyFlags'


# ---------------------------------------------------------------------------
# Client request wrappers (one Pydantic class per variant)
# ---------------------------------------------------------------------------

class InitializeRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['initialize'] = Field(default='initialize')
    params: InitializeRequestParams

    class InitializeRequestParams(InitializeParams):
        pass

InitializeRequestParams = InitializeRequest.InitializeRequestParams

class SessionStartRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/start'] = Field(default='session/start')
    params: SessionStartRequestParams

    class SessionStartRequestParams(SessionStartParams):
        pass

SessionStartRequestParams = SessionStartRequest.SessionStartRequestParams

class SessionResumeRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/resume'] = Field(default='session/resume')
    params: SessionResumeRequestParams

    class SessionResumeRequestParams(SessionResumeParams):
        pass

SessionResumeRequestParams = SessionResumeRequest.SessionResumeRequestParams

class SessionListRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/list'] = Field(default='session/list')
    params: SessionListRequestParams

    class SessionListRequestParams(BaseModel):
        model_config = {"extra": "allow"}

SessionListRequestParams = SessionListRequest.SessionListRequestParams

class SessionReadRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/read'] = Field(default='session/read')
    params: SessionReadRequestParams

    class SessionReadRequestParams(SessionReadParams):
        pass

SessionReadRequestParams = SessionReadRequest.SessionReadRequestParams

class SessionArchiveRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['session/archive'] = Field(default='session/archive')
    params: SessionArchiveRequestParams

    class SessionArchiveRequestParams(SessionArchiveParams):
        pass

SessionArchiveRequestParams = SessionArchiveRequest.SessionArchiveRequestParams

class TurnStartRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['turn/start'] = Field(default='turn/start')
    params: TurnStartRequestParams

    class TurnStartRequestParams(TurnStartParams):
        pass

TurnStartRequestParams = TurnStartRequest.TurnStartRequestParams

class TurnInterruptRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['turn/interrupt'] = Field(default='turn/interrupt')
    params: TurnInterruptRequestParams

    class TurnInterruptRequestParams(BaseModel):
        model_config = {"extra": "allow"}

TurnInterruptRequestParams = TurnInterruptRequest.TurnInterruptRequestParams

class ApprovalResolveRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['approval/resolve'] = Field(default='approval/resolve')
    params: ApprovalResolveRequestParams

    class ApprovalResolveRequestParams(ApprovalResolveParams):
        pass

ApprovalResolveRequestParams = ApprovalResolveRequest.ApprovalResolveRequestParams

class UserInputResolveRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['input/resolveUserInput'] = Field(default='input/resolveUserInput')
    params: UserInputResolveRequestParams

    class UserInputResolveRequestParams(UserInputResolveParams):
        pass

UserInputResolveRequestParams = UserInputResolveRequest.UserInputResolveRequestParams

class ElicitationResolveRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['elicitation/resolve'] = Field(default='elicitation/resolve')
    params: ElicitationResolveRequestParams

    class ElicitationResolveRequestParams(ElicitationResolveParams):
        pass

ElicitationResolveRequestParams = ElicitationResolveRequest.ElicitationResolveRequestParams

class SetModelRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/setModel'] = Field(default='control/setModel')
    params: SetModelRequestParams

    class SetModelRequestParams(SetModelParams):
        pass

SetModelRequestParams = SetModelRequest.SetModelRequestParams

class SetPermissionModeRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/setPermissionMode'] = Field(default='control/setPermissionMode')
    params: SetPermissionModeRequestParams

    class SetPermissionModeRequestParams(SetPermissionModeParams):
        pass

SetPermissionModeRequestParams = SetPermissionModeRequest.SetPermissionModeRequestParams

class SetThinkingRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/setThinking'] = Field(default='control/setThinking')
    params: SetThinkingRequestParams

    class SetThinkingRequestParams(SetThinkingParams):
        pass

SetThinkingRequestParams = SetThinkingRequest.SetThinkingRequestParams

class StopTaskRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/stopTask'] = Field(default='control/stopTask')
    params: StopTaskRequestParams

    class StopTaskRequestParams(StopTaskParams):
        pass

StopTaskRequestParams = StopTaskRequest.StopTaskRequestParams

class RewindFilesRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/rewindFiles'] = Field(default='control/rewindFiles')
    params: RewindFilesRequestParams

    class RewindFilesRequestParams(RewindFilesParams):
        pass

RewindFilesRequestParams = RewindFilesRequest.RewindFilesRequestParams

class UpdateEnvRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/updateEnv'] = Field(default='control/updateEnv')
    params: UpdateEnvRequestParams

    class UpdateEnvRequestParams(UpdateEnvParams):
        pass

UpdateEnvRequestParams = UpdateEnvRequest.UpdateEnvRequestParams

class KeepAliveRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/keepAlive'] = Field(default='control/keepAlive')
    params: KeepAliveRequestParams

    class KeepAliveRequestParams(BaseModel):
        model_config = {"extra": "allow"}

KeepAliveRequestParams = KeepAliveRequest.KeepAliveRequestParams

class CancelRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['control/cancelRequest'] = Field(default='control/cancelRequest')
    params: CancelRequestParams

    class CancelRequestParams(CancelRequestParams):
        pass

CancelRequestParams = CancelRequest.CancelRequestParams

class AgentInterruptCurrentWorkRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['agent/interruptCurrentWork'] = Field(default='agent/interruptCurrentWork')
    params: AgentInterruptCurrentWorkRequestParams

    class AgentInterruptCurrentWorkRequestParams(AgentInterruptCurrentWorkParams):
        pass

AgentInterruptCurrentWorkRequestParams = AgentInterruptCurrentWorkRequest.AgentInterruptCurrentWorkRequestParams

class ConfigReadRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['config/read'] = Field(default='config/read')
    params: ConfigReadRequestParams

    class ConfigReadRequestParams(BaseModel):
        model_config = {"extra": "allow"}

ConfigReadRequestParams = ConfigReadRequest.ConfigReadRequestParams

class ConfigWriteRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['config/value/write'] = Field(default='config/value/write')
    params: ConfigWriteRequestParams

    class ConfigWriteRequestParams(ConfigWriteParams):
        pass

ConfigWriteRequestParams = ConfigWriteRequest.ConfigWriteRequestParams

class McpStatusRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/status'] = Field(default='mcp/status')
    params: McpStatusRequestParams

    class McpStatusRequestParams(BaseModel):
        model_config = {"extra": "allow"}

McpStatusRequestParams = McpStatusRequest.McpStatusRequestParams

class ContextUsageRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['context/usage'] = Field(default='context/usage')
    params: ContextUsageRequestParams

    class ContextUsageRequestParams(BaseModel):
        model_config = {"extra": "allow"}

ContextUsageRequestParams = ContextUsageRequest.ContextUsageRequestParams

class McpSetServersRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/setServers'] = Field(default='mcp/setServers')
    params: McpSetServersRequestParams

    class McpSetServersRequestParams(McpSetServersParams):
        pass

McpSetServersRequestParams = McpSetServersRequest.McpSetServersRequestParams

class McpReconnectRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/reconnect'] = Field(default='mcp/reconnect')
    params: McpReconnectRequestParams

    class McpReconnectRequestParams(McpReconnectParams):
        pass

McpReconnectRequestParams = McpReconnectRequest.McpReconnectRequestParams

class McpToggleRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['mcp/toggle'] = Field(default='mcp/toggle')
    params: McpToggleRequestParams

    class McpToggleRequestParams(McpToggleParams):
        pass

McpToggleRequestParams = McpToggleRequest.McpToggleRequestParams

class PluginReloadRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['plugin/reload'] = Field(default='plugin/reload')
    params: PluginReloadRequestParams

    class PluginReloadRequestParams(BaseModel):
        model_config = {"extra": "allow"}

PluginReloadRequestParams = PluginReloadRequest.PluginReloadRequestParams

class ConfigApplyFlagsRequest(BaseModel):
    model_config = {"populate_by_name": True}
    method: Literal['config/applyFlags'] = Field(default='config/applyFlags')
    params: ConfigApplyFlagsRequestParams

    class ConfigApplyFlagsRequestParams(ConfigApplyFlagsParams):
        pass

ConfigApplyFlagsRequestParams = ConfigApplyFlagsRequest.ConfigApplyFlagsRequestParams

ClientRequest = Annotated[
    Union[InitializeRequest, SessionStartRequest, SessionResumeRequest, SessionListRequest, SessionReadRequest, SessionArchiveRequest, TurnStartRequest, TurnInterruptRequest, ApprovalResolveRequest, UserInputResolveRequest, ElicitationResolveRequest, SetModelRequest, SetPermissionModeRequest, SetThinkingRequest, StopTaskRequest, RewindFilesRequest, UpdateEnvRequest, KeepAliveRequest, CancelRequest, AgentInterruptCurrentWorkRequest, ConfigReadRequest, ConfigWriteRequest, McpStatusRequest, ContextUsageRequest, McpSetServersRequest, McpReconnectRequest, McpToggleRequest, PluginReloadRequest, ConfigApplyFlagsRequest],
    Field(discriminator='method'),
]

# ---------------------------------------------------------------------------
# Additional types
# ---------------------------------------------------------------------------

class AgentInfo(BaseModel):
    name: str
    description: str | None = None

class AgentsDialogEntry(BaseModel):
    description: str
    name: str
    source: AgentSource
    color: AgentColorName | None = None
    is_overridden: bool = False
    source_path: str | None = None

class AgentsDialogPayload(BaseModel):
    entries: list[AgentsDialogEntry]

class AlreadyReadFilePayload(BaseModel):
    display_path: str
    filename: str
    content: str = ''
    truncated: bool = False

class ApiError(BaseModel):
    message: str
    error_type: str | None = None
    status_code: int | None = None

class ApplyPatchPreview(BaseModel):
    rows: list[ApplyPatchPreviewRow]

class AskUserQuestionAnswered(BaseModel):
    answers: list[str]
    question: str
    note: str | None = None

class AskUserQuestionResult(BaseModel):
    questions: list[AskUserQuestionAnswered]

class AssistantMessage(BaseModel):
    message: LanguageModelV4Message
    uuid: str
    api_error: ApiError | None = None
    cost_usd: float | None = None
    model: str = ''
    request_id: str | None = None
    stop_reason: UnifiedFinishReason | None = None
    usage: TokenUsage | None = None

class AttachmentMessage(BaseModel):
    body: AttachmentBody
    kind: AttachmentKind
    uuid: str
    extras: AttachmentExtras | None = None

class AttachmentTypeBreakdown(BaseModel):
    name: str
    tokens: int

class BudgetExhaustedOutcome(BaseModel):
    used_tokens: int
    budget_tokens: int | None = None

class CommandPermissionsPayload(BaseModel):
    allowed_tools: list[str] = Field(alias='allowedTools')
    model: str | None = None

class CompactFileReferencePayload(BaseModel):
    display_path: str
    filename: str

class CompletedOutcome(BaseModel):
    stop_reason: UnifiedFinishReason | None = None

class ConfigChangeInput(BaseModel):
    cwd: str
    session_id: str
    source: ConfigChangeSource
    hook_event_name: Literal['ConfigChange']
    agent_id: str | None = None
    agent_type: str | None = None
    file_path: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class ConfigReadResult(BaseModel):
    config: Any
    sources: dict[str, Any] = {}

class ContextAgent(BaseModel):
    agent_type: str
    source: str
    tokens: int

class ContextMcpTool(BaseModel):
    is_loaded: bool
    name: str
    server_name: str
    tokens: int

class ContextMemoryFile(BaseModel):
    path: str
    source: str
    tokens: int

class ContextSkill(BaseModel):
    name: str
    source: str
    tokens: int

class ContextSuggestion(BaseModel):
    detail: str
    severity: SuggestionSeverity
    title: str
    savings_tokens: int | None = None

class ContextUsageCategory(BaseModel):
    kind: ContextCategoryKind
    tokens: int

class ContextUsageResult(BaseModel):
    categories: list[ContextUsageCategory]
    is_auto_compact_enabled: bool
    max_tokens: int
    model: str
    percentage: float
    raw_max_tokens: int
    total_tokens: int
    agents: list[ContextAgent] | None = None
    auto_compact_threshold: int | None = None
    mcp_tools: list[ContextMcpTool] | None = None
    memory_files: list[ContextMemoryFile] | None = None
    message_breakdown: MessageBreakdown | None = None
    skills: list[ContextSkill] | None = None
    suggestions: list[ContextSuggestion] | None = None

class CustomPart(BaseModel):
    kind: str
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')
    provider_options: ProviderOptions | None = Field(default=None, alias='providerOptions')

class CwdChangedInput(BaseModel):
    cwd: str
    new_cwd: str
    old_cwd: str
    session_id: str
    hook_event_name: Literal['CwdChanged']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class DynamicSkillPayload(BaseModel):
    display_path: str = Field(alias='displayPath')
    skill_dir: str = Field(alias='skillDir')
    skill_names: list[str] = Field(alias='skillNames')

class EditedImageFilePayload(BaseModel):
    display_path: str
    filename: str

class ElicitationInput(BaseModel):
    cwd: str
    mcp_server_name: str
    message: str
    session_id: str
    hook_event_name: Literal['Elicitation']
    agent_id: str | None = None
    agent_type: str | None = None
    elicitation_id: str | None = None
    mode: ElicitationMode | None = None
    permission_mode: str | None = None
    requested_schema: Any = None
    transcript_path: str = ''
    url: str | None = None

class ElicitationResultInput(BaseModel):
    action: ElicitationAction
    cwd: str
    mcp_server_name: str
    session_id: str
    hook_event_name: Literal['ElicitationResult']
    agent_id: str | None = None
    agent_type: str | None = None
    content: dict[str, Any] | None = None
    elicitation_id: str | None = None
    mode: ElicitationMode | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class ErrorPayload(BaseModel):
    code: ErrorCode
    message: str

class ExitPlanModeAllowedPrompt(BaseModel):
    prompt: str
    tool: str

class ExitPlanModeResult(BaseModel):
    awaiting_leader_approval: bool = Field(alias='awaitingLeaderApproval')
    is_agent: bool = Field(alias='isAgent')
    outcome: ExitPlanModeOutcome
    plan: str
    plan_was_edited: bool = Field(alias='planWasEdited')
    file_path: str | None = Field(default=None, alias='filePath')

class FailedOutcome(BaseModel):
    error: ErrorPayload

class FileChangeInfo(BaseModel):
    kind: FileChangeKind
    path: str

class FileChangedInput(BaseModel):
    cwd: str
    event: FileChangeEvent
    file_path: str
    session_id: str
    hook_event_name: Literal['FileChanged']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class FilePart(BaseModel):
    data: SharedV4FileData
    media_type: str = Field(alias='mediaType')
    filename: str | None = None
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')

class HookCallbackMatcher(BaseModel):
    hook_callback_ids: list[str]
    matcher: str | None = None
    timeout: int | None = None

class HookCallbackResult(BaseModel):
    output: SdkHookOutput

class HookCancelledPayload(BaseModel):
    hook_event: HookEventType
    hook_name: str
    tool_use_id: str
    command: str | None = None
    duration_ms: int | None = None

class HookErrorDuringExecutionPayload(BaseModel):
    content: str
    hook_event: HookEventType
    hook_name: str
    tool_use_id: str

class HookNonBlockingErrorPayload(BaseModel):
    error: str
    hook_event: HookEventType
    hook_name: str
    tool_use_id: str

class HookPermissionDecisionPayload(BaseModel):
    decision: HookPermissionDecision
    hook_event: HookEventType
    tool_use_id: str

class HookSystemMessagePayload(BaseModel):
    content: str
    hook_event: HookEventType
    hook_name: str
    tool_use_id: str

class InitializeResult(BaseModel):
    output_style: str
    coco_rs_protocol_version: str = Field(default=None, alias='_cocoRsProtocolVersion')
    coco_rs_version: str = Field(default=None, alias='_cocoRsVersion')
    account: SdkAccountInfo = {}
    agents: list[SdkAgentInfo] = []
    available_output_styles: list[str] = []
    commands: list[SdkSlashCommand] = []
    fast_mode_state: FastModeState | None = None
    models: list[SdkModelInfo] = []
    pid: int | None = None

class InputTokens(BaseModel):
    cache_read: int = 0
    cache_write: int = 0
    no_cache: int = 0
    total: int = 0

class InstructionsLoadedInput(BaseModel):
    cwd: str
    file_path: str
    load_reason: InstructionsLoadReason
    memory_type: MemoryType
    session_id: str
    hook_event_name: Literal['InstructionsLoaded']
    agent_id: str | None = None
    agent_type: str | None = None
    globs: list[str] | None = None
    parent_file_path: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''
    trigger_file_path: str | None = None

class InterruptedOutcome(BaseModel):
    abort_reason: TurnAbortReason

class JsonRpcError(BaseModel):
    code: int
    message: str
    request_id: RequestId
    data: Any = None

class JsonRpcNotification(BaseModel):
    method: str
    params: Any = None

class JsonRpcRequest(BaseModel):
    method: str
    request_id: RequestId
    params: Any = None

class JsonRpcResponse(BaseModel):
    request_id: RequestId
    result: Any = None

class MaxTurnsReachedOutcome(BaseModel):
    max_turns: int

class MaxTurnsReachedPayload(BaseModel):
    max_turns: int = Field(alias='maxTurns')
    turn_count: int = Field(alias='turnCount')

class McpRouteMessageResult(BaseModel):
    message: Any

class McpServerInit(BaseModel):
    name: str
    status: McpConnectionStatus

class McpServerStatus(BaseModel):
    name: str
    status: McpConnectionStatus
    error: str | None = None
    skipped_tools: list[McpSkippedToolStatus] | None = None
    tombstoned_tools: list[str] | None = None
    tool_count: int = 0

class McpSetServersResult(BaseModel):
    added: list[str]
    errors: dict[str, str]
    removed: list[str]

class McpSkippedToolStatus(BaseModel):
    error: str
    tool_name: str

class McpStatusResult(BaseModel):
    mcp_servers: list[McpServerStatus] = Field(alias='mcpServers')

class MemoryDialogEntry(BaseModel):
    label: str
    path: str
    scope: MemoryDialogScope
    row_kind: MemoryDialogRowKind = {}

class MentionSummaryItem(BaseModel):
    display_path: str
    kind: MentionItemKind
    count: int | None = None
    truncated: bool = False

class MentionSummaryPayload(BaseModel):
    items: list[MentionSummaryItem]

class MessageBreakdown(BaseModel):
    assistant_message_tokens: int
    attachment_tokens: int
    tool_call_tokens: int
    tool_result_tokens: int
    user_message_tokens: int
    attachments_by_type: list[AttachmentTypeBreakdown] | None = None
    tool_calls_by_type: list[ToolTypeBreakdown] | None = None

class ModelSpec(BaseModel):
    api: ProviderApi
    display_name: str
    model_id: str
    provider: str

class NotificationInput(BaseModel):
    cwd: str
    message: str
    notification_type: str
    session_id: str
    hook_event_name: Literal['Notification']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    title: str | None = None
    transcript_path: str = ''

class OutputTokens(BaseModel):
    reasoning: int = 0
    text: int = 0
    total: int = 0

class PermissionAskChoice(BaseModel):
    label: str
    value: str
    description: str | None = None

class PermissionDenialInfo(BaseModel):
    tool_input: Any
    tool_name: str
    tool_use_id: str

class PermissionDeniedInput(BaseModel):
    cwd: str
    reason: str
    session_id: str
    tool_input: Any
    tool_name: str
    tool_use_id: str
    hook_event_name: Literal['PermissionDenied']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class PermissionExplanation(BaseModel):
    explanation: str
    reasoning: str
    risk: str
    risk_level: RiskLevel

class PermissionRequestInput(BaseModel):
    cwd: str
    session_id: str
    tool_input: Any
    tool_name: str
    hook_event_name: Literal['PermissionRequest']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    permission_suggestions: Any = None
    transcript_path: str = ''

class PermissionRule(BaseModel):
    behavior: PermissionBehavior
    source: PermissionRuleSource
    value: PermissionRuleValue

class PermissionRuleValue(BaseModel):
    tool_pattern: str
    rule_content: str | None = None

class PermissionsEditorDir(BaseModel):
    path: str
    source: PermissionRuleSource

class PermissionsEditorPayload(BaseModel):
    cwd: str
    directories: list[PermissionsEditorDir]
    rules: list[PermissionsEditorRule]
    managed_only: bool = False

class PermissionsEditorRule(BaseModel):
    behavior: PermissionBehavior
    source: PermissionRuleSource
    tool_pattern: str
    rule_content: str | None = None

class PersistedFileError(BaseModel):
    error: str
    filename: str

class PersistedFileInfo(BaseModel):
    file_id: str
    filename: str

class PluginDialogAction(BaseModel):
    label: str
    plugin_args: str

class PluginDialogErrorRow(BaseModel):
    message: str
    plugin_id: str

class PluginDialogInstalledRow(BaseModel):
    blocked_by_policy: bool
    enabled: bool
    id: str
    name: str
    path: str
    source: str
    actions: list[PluginDialogAction] = []
    description: str | None = None
    mcp_servers: list[PluginDialogMcpServerRow] = []
    options: list[PluginDialogOptionRow] = []
    version: str | None = None

class PluginDialogMarketplaceRow(BaseModel):
    name: str
    official: bool
    plugin_count: int
    actions: list[PluginDialogAction] = []
    source: str | None = None

class PluginDialogMcpServerRow(BaseModel):
    display_name: str
    enabled: bool
    name: str
    needs_config: bool
    actions: list[PluginDialogAction] = []
    tools: list[PluginDialogMcpToolRow] = []

class PluginDialogMcpToolRow(BaseModel):
    name: str
    description: str | None = None

class PluginDialogOptionRow(BaseModel):
    description: str
    key: str
    required: bool
    title: str
    value_type: str
    current_value: Any = None

class PluginDialogPayload(BaseModel):
    errors: list[PluginDialogErrorRow]
    installed: list[PluginDialogInstalledRow]
    marketplaces: list[PluginDialogMarketplaceRow]

class PluginInit(BaseModel):
    name: str
    path: str
    source: str | None = None

class PluginReloadResult(BaseModel):
    agents: list[str]
    commands: list[str]
    error_count: int
    plugins: list[str]

class PostCompactInput(BaseModel):
    compact_summary: str
    cwd: str
    session_id: str
    trigger: CompactTrigger
    hook_event_name: Literal['PostCompact']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class PostToolUseFailureInput(BaseModel):
    cwd: str
    error: str
    session_id: str
    tool_input: Any
    tool_name: str
    tool_use_id: str
    hook_event_name: Literal['PostToolUseFailure']
    agent_id: str | None = None
    agent_type: str | None = None
    is_interrupt: bool | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class PostToolUseInput(BaseModel):
    cwd: str
    session_id: str
    tool_input: Any
    tool_name: str
    tool_response: Any
    tool_use_id: str
    hook_event_name: Literal['PostToolUse']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class PreCompactInput(BaseModel):
    cwd: str
    session_id: str
    trigger: CompactTrigger
    hook_event_name: Literal['PreCompact']
    agent_id: str | None = None
    agent_type: str | None = None
    custom_instructions: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class PreToolUseInput(BaseModel):
    cwd: str
    session_id: str
    tool_input: Any
    tool_name: str
    tool_use_id: str
    hook_event_name: Literal['PreToolUse']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class PreservedSegment(BaseModel):
    anchor_uuid: str
    head_uuid: str
    tail_uuid: str

class ProgressMessage(BaseModel):
    data: Any
    tool_use_id: str
    parent_message_uuid: str | None = None

class ProviderMetadata(BaseModel):
    pass

class ProviderModelSelection(BaseModel):
    model_id: str
    provider: str

class ProviderOptions(BaseModel):
    pass

class QueuedCommandEditImage(BaseModel):
    data_base64: str
    media_type: str

class ReasoningFilePart(BaseModel):
    data: LanguageModelV4FileData
    media_type: str = Field(alias='mediaType')
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')

class ReasoningPart(BaseModel):
    text: str
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')

class RewindDiffStatsPayload(BaseModel):
    deletions: int
    file_paths: list[str]
    insertions: int

class RewindFilesResult(BaseModel):
    deletions: int = 0
    dry_run: bool = False
    files_changed: list[str] = []
    insertions: int = 0

class RewindRowMetadata(BaseModel):
    message_id: str
    metadata: RewindDiffStatsPayload | None = None

class SdkAccountInfo(BaseModel):
    api_key_source: str | None = Field(default=None, alias='apiKeySource')
    api_provider: ApiProvider | None = Field(default=None, alias='apiProvider')
    email: str | None = None
    organization: str | None = None
    subscription_type: str | None = Field(default=None, alias='subscriptionType')
    token_source: str | None = Field(default=None, alias='tokenSource')

class SdkAgentDefinition(BaseModel):
    description: str
    prompt: str
    background: bool | None = None
    critical_system_reminder_experimental: str | None = Field(default=None, alias='criticalSystemReminder_EXPERIMENTAL')
    disallowed_tools: list[str] | None = None
    effort: ReasoningEffort | None = None
    initial_prompt: str | None = None
    max_turns: int | None = None
    mcp_servers: list[AgentMcpServerSpec] | None = None
    memory: MemoryScope | None = None
    model: str | None = None
    permission_mode: PermissionMode | None = None
    skills: list[str] | None = None
    tools: list[str] | None = None

class SdkAgentInfo(BaseModel):
    description: str
    name: str
    model: str | None = None

class SdkHookOutput(BaseModel):
    async_: bool | None = Field(default=None, alias='async')
    async_timeout: int | None = Field(default=None, alias='asyncTimeout')
    continue_: bool | None = Field(default=None, alias='continue')
    decision: HookDecision | None = None
    hook_specific_output: HookSpecificOutput | None = Field(default=None, alias='hookSpecificOutput')
    reason: str | None = None
    stop_reason: str | None = Field(default=None, alias='stopReason')
    suppress_output: bool | None = Field(default=None, alias='suppressOutput')
    system_message: str | None = Field(default=None, alias='systemMessage')

class SdkModelInfo(BaseModel):
    description: str
    display_name: str = Field(alias='displayName')
    value: str
    supported_effort_levels: list[EffortLevel] = Field(default=None, alias='supportedEffortLevels')
    supports_adaptive_thinking: bool | None = Field(default=None, alias='supportsAdaptiveThinking')
    supports_auto_mode: bool | None = Field(default=None, alias='supportsAutoMode')
    supports_effort: bool | None = Field(default=None, alias='supportsEffort')
    supports_fast_mode: bool | None = Field(default=None, alias='supportsFastMode')

class SdkSessionSummary(BaseModel):
    created_at: str
    cwd: str
    model: str
    session_id: str
    message_count: int = 0
    title: str | None = None
    total_tokens: int = 0
    updated_at: str | None = None

class SdkSlashCommand(BaseModel):
    argument_hint: str = Field(alias='argumentHint')
    description: str
    name: str

class SessionEndInput(BaseModel):
    cwd: str
    reason: ExitReason
    session_id: str
    hook_event_name: Literal['SessionEnd']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class SessionListResult(BaseModel):
    sessions: list[SdkSessionSummary]

class SessionModelUsage(BaseModel):
    cache_creation_input_tokens: int
    cache_read_input_tokens: int
    context_window: int
    cost_usd: float
    input_tokens: int
    max_output_tokens: int
    output_tokens: int
    web_search_requests: int

class SessionModelUsageEntry(BaseModel):
    cache_creation_cost_usd: float
    cache_creation_input_tokens: int
    cache_read_cost_usd: float
    cache_read_input_tokens: int
    input_cost_usd: float
    input_tokens: int
    model_id: str
    output_cost_usd: float
    output_tokens: int
    priced: bool
    provider: str
    request_count: int
    total_cost_usd: float
    unpriced_input_tokens: int = 0
    unpriced_output_tokens: int = 0
    unpriced_request_count: int = 0
    web_search_requests: int = 0

class SessionReadResult(BaseModel):
    session: SdkSessionSummary
    has_more: bool = False
    messages: list[Any] = []
    next_cursor: str | None = None

class SessionResumeResult(BaseModel):
    session: SdkSessionSummary

class SessionStartInput(BaseModel):
    cwd: str
    session_id: str
    source: SessionStartSource
    hook_event_name: Literal['SessionStart']
    agent_id: str | None = None
    agent_type: str | None = None
    model: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class SessionStartResult(BaseModel):
    session_id: str

class SessionUsageTotals(BaseModel):
    cache_creation_cost_usd: float
    cache_creation_input_tokens: int
    cache_read_cost_usd: float
    cache_read_input_tokens: int
    input_cost_usd: float
    input_tokens: int
    output_cost_usd: float
    output_tokens: int
    request_count: int
    total_cost_usd: float
    unpriced_input_tokens: int = 0
    unpriced_output_tokens: int = 0
    unpriced_request_count: int = 0
    web_search_requests: int = 0

class SetupInput(BaseModel):
    cwd: str
    session_id: str
    trigger: SetupTrigger
    hook_event_name: Literal['Setup']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class SkillDiscoveryPayload(BaseModel):
    signal: str
    skills: list[SkillDiscoverySkill]
    source: SkillDiscoverySource

class SkillDiscoverySkill(BaseModel):
    description: str
    name: str
    short_id: str | None = Field(default=None, alias='shortId')

class SkillLock(BaseModel):
    forced_value: SkillOverrideState
    source: SkillLockSource

class SkillsDialogEntry(BaseModel):
    baseline: SkillOverrideState
    description: str
    frontmatter_bytes: int
    name: str
    source: SkillsDialogSource
    current_local: SkillOverrideState | None = None
    lock: SkillLock | None = None
    plugin_name: str | None = None

class SkillsDialogPayload(BaseModel):
    bytes_per_token: int
    entries: list[SkillsDialogEntry]

class SlashCommandInfo(BaseModel):
    name: str
    aliases: list[str] | None = None
    argument_hint: str | None = None
    argument_kind: CommandArgumentKind = 'none'
    description: str | None = None
    kind: CommandTypeTag = 'local'
    source: CommandSource | None = None
    usage_score: float = 0.0

class SourcePart(BaseModel):
    id: str
    source_type: SourceType = Field(alias='sourceType')
    filename: str | None = None
    media_type: str | None = Field(default=None, alias='mediaType')
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')
    title: str | None = None
    url: str | None = None

class StopFailureInput(BaseModel):
    cwd: str
    error: str
    session_id: str
    hook_event_name: Literal['StopFailure']
    agent_id: str | None = None
    agent_type: str | None = None
    error_details: str | None = None
    last_assistant_message: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class StopInput(BaseModel):
    cwd: str
    session_id: str
    stop_hook_active: bool
    hook_event_name: Literal['Stop']
    agent_id: str | None = None
    agent_type: str | None = None
    last_assistant_message: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class StructuredOutputPayload(BaseModel):
    data: Any

class SubagentStartInput(BaseModel):
    agent_id: str
    agent_type: str
    cwd: str
    session_id: str
    hook_event_name: Literal['SubagentStart']
    permission_mode: str | None = None
    transcript_path: str = ''

class SubagentStopInput(BaseModel):
    agent_id: str
    agent_transcript_path: str
    agent_type: str
    cwd: str
    session_id: str
    stop_hook_active: bool
    hook_event_name: Literal['SubagentStop']
    last_assistant_message: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class SystemAgentsKilledMessage(BaseModel):
    count: int
    uuid: str

class SystemApiErrorMessage(BaseModel):
    error: str
    uuid: str
    status_code: int | None = None

class SystemApiMetricsMessage(BaseModel):
    model: str
    usage: TokenUsage
    uuid: str
    cost_usd: float | None = None

class SystemAwaySummaryMessage(BaseModel):
    summary: str
    uuid: str

class SystemBridgeStatusMessage(BaseModel):
    connected: bool
    uuid: str
    message: str | None = None

class SystemCompactBoundaryMessage(BaseModel):
    tokens_after: int
    tokens_before: int
    uuid: str
    messages_summarized: int | None = None
    pre_compact_discovered_tools: list[str] | None = None
    preserved_segment: PreservedSegment | None = None
    trigger: CompactTrigger = 'auto'
    user_context: str | None = None

class SystemContextUsageMessage(BaseModel):
    result: ContextUsageResult
    uuid: str

class SystemInformationalMessage(BaseModel):
    level: SystemMessageLevel
    message: str
    title: str
    uuid: str

class SystemLocalCommandMessage(BaseModel):
    command: str
    output: str
    uuid: str

class SystemMemorySavedMessage(BaseModel):
    uuid: str
    verb: str = 'Saved'
    written_paths: list[str] = []

class SystemMicrocompactBoundaryMessage(BaseModel):
    uuid: str

class SystemPermissionRetryMessage(BaseModel):
    message: str
    tool_name: str
    uuid: str

class SystemScheduledTaskFireMessage(BaseModel):
    schedule: str
    task_id: str
    uuid: str

class SystemStopHookSummaryMessage(BaseModel):
    hook_name: str
    outcome: str
    uuid: str

class SystemTurnDurationMessage(BaseModel):
    duration_ms: int
    uuid: str

class SystemUserInterruptionMessage(BaseModel):
    for_tool_use: bool
    uuid: str

class TaskActivity(BaseModel):
    tool_name: str
    summary: str | None = None

class TaskCompletedInput(BaseModel):
    cwd: str
    session_id: str
    task_id: str
    task_subject: str
    hook_event_name: Literal['TaskCompleted']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    task_description: str | None = None
    team_name: str | None = None
    teammate_name: str | None = None
    transcript_path: str = ''

class TaskCreatedInput(BaseModel):
    cwd: str
    session_id: str
    task_id: str
    task_subject: str
    hook_event_name: Literal['TaskCreated']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    task_description: str | None = None
    team_name: str | None = None
    teammate_name: str | None = None
    transcript_path: str = ''

class TaskRecord(BaseModel):
    id: str
    status: TaskListStatus
    subject: str
    active_form: str | None = Field(default=None, alias='activeForm')
    blocked_by: list[str] = Field(default=None, alias='blockedBy')
    blocks: list[str] = []
    description: str = ''
    metadata: dict[str, Any] | None = None
    owner: str | None = None

class TaskUsage(BaseModel):
    duration_ms: int
    tool_uses: int
    total_tokens: int
    cache_read_tokens: int = 0
    cost_usd: float = 0.0
    input_tokens: int = 0
    output_tokens: int = 0

class TeammateIdleInput(BaseModel):
    cwd: str
    session_id: str
    team_name: str
    teammate_name: str
    hook_event_name: Literal['TeammateIdle']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class TextPart(BaseModel):
    text: str
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')

class ThinkingLevel(BaseModel):
    effort: ReasoningEffort
    budget_tokens: int | None = None
    options: dict[str, Any] | None = None

class TodoRecord(BaseModel):
    active_form: str = Field(alias='activeForm')
    content: str
    status: str

class TokenUsage(BaseModel):
    input_tokens: InputTokens = {}
    output_tokens: OutputTokens = {}

class TombstoneMessage(BaseModel):
    original_kind: MessageKind
    uuid: str

class ToolApprovalRequestPart(BaseModel):
    approval_id: str = Field(alias='approvalId')
    tool_call_id: str = Field(alias='toolCallId')
    context: str | None = None
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')
    tool_name: str | None = Field(default=None, alias='toolName')

class ToolApprovalResponsePart(BaseModel):
    approval_id: str = Field(alias='approvalId')
    approved: bool
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')
    reason: str | None = None

class ToolCallPart(BaseModel):
    input: Any
    tool_call_id: str = Field(alias='toolCallId')
    tool_name: str = Field(alias='toolName')
    invalid: bool = False
    invalid_reason: ToolInputInvalidReason | None = Field(default=None, alias='invalidReason')
    provider_executed: bool | None = Field(default=None, alias='providerExecuted')
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')

class ToolResultMessage(BaseModel):
    message: LanguageModelV4Message
    tool_id: str
    tool_use_id: str
    uuid: str
    display_data: ToolDisplayData | None = None
    is_error: bool = False
    source_assistant_uuid: str | None = None

class ToolResultPart(BaseModel):
    output: ToolResultContent
    tool_call_id: str = Field(alias='toolCallId')
    tool_name: str = Field(alias='toolName')
    is_error: bool = Field(default=None, alias='isError')
    provider_metadata: ProviderMetadata | None = Field(default=None, alias='providerMetadata')

class ToolTypeBreakdown(BaseModel):
    call_tokens: int
    name: str
    result_tokens: int

class TurnStartResult(BaseModel):
    turn_id: str

class UserMessage(BaseModel):
    message: LanguageModelV4Message
    uuid: str
    is_compact_summary: bool = False
    is_virtual: bool = False
    is_visible_in_transcript_only: bool = False
    origin: MessageOrigin | None = None
    parent_tool_use_id: str | None = None
    permission_mode: PermissionMode | None = None
    timestamp: str = ''

class UserPromptSubmitInput(BaseModel):
    cwd: str
    prompt: str
    session_id: str
    hook_event_name: Literal['UserPromptSubmit']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class WorkerBadge(BaseModel):
    color: AgentColorName
    name: str

class WorktreeCreateInput(BaseModel):
    cwd: str
    name: str
    session_id: str
    hook_event_name: Literal['WorktreeCreate']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

class WorktreeRemoveInput(BaseModel):
    cwd: str
    session_id: str
    worktree_path: str
    hook_event_name: Literal['WorktreeRemove']
    agent_id: str | None = None
    agent_type: str | None = None
    permission_mode: str | None = None
    transcript_path: str = ''

# ---------------------------------------------------------------------------
# Tagged discriminated unions (ref-based)
# ---------------------------------------------------------------------------

HookInput = Annotated[
    Union[PreToolUseInput, PostToolUseInput, PostToolUseFailureInput, SessionStartInput, SessionEndInput, SetupInput, StopInput, StopFailureInput, PreCompactInput, PostCompactInput, SubagentStartInput, SubagentStopInput, UserPromptSubmitInput, PermissionRequestInput, PermissionDeniedInput, NotificationInput, ElicitationInput, ElicitationResultInput, FileChangedInput, ConfigChangeInput, InstructionsLoadedInput, CwdChangedInput, WorktreeCreateInput, WorktreeRemoveInput, TaskCreatedInput, TaskCompletedInput, TeammateIdleInput],
    Field(discriminator='hook_event_name'),
]


# ── Resolve forward refs for every emitted BaseModel ──
# Pydantic v2's TypeAdapter (used in discriminated unions)
# constructs validators eagerly; classes that reference
# later-defined models would error on first validation
# without an explicit rebuild pass.
import sys as _sys
for _name in list(globals()):
    _obj = globals()[_name]
    if isinstance(_obj, type) and issubclass(_obj, BaseModel):
        try:
            _obj.model_rebuild()
        except Exception:
            pass
del _name, _obj
