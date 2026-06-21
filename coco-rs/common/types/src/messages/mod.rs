//! Message-family types — Message envelope, content aliases, attachment
//! payloads, tool results, hook results, transcript persistence shapes.
//!
//! Lives in `coco-types` so wire-protocol envelopes (ServerNotification,
//! CoreEvent) at the same crate can carry typed `Message` payloads
//! without crossing a layer boundary. DTOs reach this module through
//! `coco-llm-types` (the vercel-ai DTO seam); no `vercel_ai_provider::*`
//! reference here.

pub mod aliases;
pub mod attachment_body;
pub mod attachment_emitter;
pub mod hook_result;
pub mod message;
pub mod serialized_message;
pub mod tool_result;
pub mod transcript;

pub use aliases::AssistantContent;
pub use aliases::DataContent;
pub use aliases::FileContent;
pub use aliases::LlmMessage;
pub use aliases::LlmPrompt;
pub use aliases::ReasoningContent;
pub use aliases::TextContent;
pub use aliases::ToolCallContent;
pub use aliases::ToolContent;
pub use aliases::ToolResultContent;
pub use aliases::ToolResultContentPart;
pub use aliases::ToolResultOutput;
pub use aliases::UserContent;
pub use aliases::tool_reference_content_part;

pub use attachment_body::AlreadyReadFilePayload;
pub use attachment_body::AttachmentBody;
pub use attachment_body::AttachmentExtras;
pub use attachment_body::CommandPermissionsPayload;
pub use attachment_body::CompactFileReferencePayload;
pub use attachment_body::DynamicSkillPayload;
pub use attachment_body::EditedImageFilePayload;
pub use attachment_body::HookCancelledPayload;
pub use attachment_body::HookErrorDuringExecutionPayload;
pub use attachment_body::HookNonBlockingErrorPayload;
pub use attachment_body::HookPermissionDecision;
pub use attachment_body::HookPermissionDecisionPayload;
pub use attachment_body::HookSystemMessagePayload;
pub use attachment_body::MaxTurnsReachedPayload;
pub use attachment_body::MentionItemKind;
pub use attachment_body::MentionSummaryItem;
pub use attachment_body::MentionSummaryPayload;
pub use attachment_body::SilentPayload;
pub use attachment_body::SkillDiscoveryPayload;
pub use attachment_body::SkillDiscoverySkill;
pub use attachment_body::SkillDiscoverySource;
pub use attachment_body::StructuredOutputPayload;

pub use attachment_emitter::AttachmentEmitter;

pub use hook_result::HookResult;

pub use message::ApiError;
pub use message::AssistantMessage;
pub use message::AttachmentMessage;
pub use message::FinishReason;
pub use message::MAX_SKILL_DISCOVERY_RENDERED_CHARS;
pub use message::Message;
pub use message::MessageKind;
pub use message::MessageOrigin;
pub use message::PartialCompactDirection;
pub use message::PreservedSegment;
pub use message::ProgressMessage;
pub use message::StopReason;
pub use message::SystemAgentsKilledMessage;
pub use message::SystemApiErrorMessage;
pub use message::SystemApiMetricsMessage;
pub use message::SystemAwaySummaryMessage;
pub use message::SystemBridgeStatusMessage;
pub use message::SystemCompactBoundaryMessage;
pub use message::SystemContextUsageMessage;
pub use message::SystemInformationalMessage;
pub use message::SystemLocalCommandMessage;
pub use message::SystemMemorySavedMessage;
pub use message::SystemMessage;
pub use message::SystemMessageLevel;
pub use message::SystemMicrocompactBoundaryMessage;
pub use message::SystemPermissionRetryMessage;
pub use message::SystemScheduledTaskFireMessage;
pub use message::SystemStopHookSummaryMessage;
pub use message::SystemTurnDurationMessage;
pub use message::SystemUserInterruptionMessage;
pub use message::TombstoneMessage;
pub use message::ToolResultMessage;
pub use message::UserMessage;
pub use message::Visibility;
pub use message::render_skill_discovery_prompt;

pub use serialized_message::SerializedMessage;

pub use tool_result::ToolResult;

pub use transcript::TranscriptEntry;
pub use transcript::TranscriptMessage;
