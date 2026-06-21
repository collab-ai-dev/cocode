//! Typed payload carrier for [`AttachmentMessage`](super::AttachmentMessage).
//!
//! Pairs with [`AttachmentKind`](crate::AttachmentKind) (the 60-variant
//! discriminant): `kind` classifies the `Attachment.type`,
//! `body` carries the data. `AttachmentBody` variants cover only kinds
//! coco-rs actually produces, so `FeatureGated` / `RuntimeBookkeeping`
//! kinds don't pollute the payload surface.
//!
//! # Invariant
//!
//! `kind` and `body` must agree — e.g. `AttachmentKind::HookCancelled` requires
//! `AttachmentBody::Silent(SilentPayload::HookCancelled(..))`. The constructor
//! helpers on [`AttachmentMessage`](super::AttachmentMessage) enforce this; do
//! not construct by struct literal.

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use crate::HookEventType;

use super::aliases::LlmMessage;

// ─── AttachmentBody ─────────────────────────────────────────────────────

/// Typed payload for an [`AttachmentMessage`](super::AttachmentMessage).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "body", rename_all = "snake_case")]
pub enum AttachmentBody {
    /// Pre-rendered `LlmMessage` — reaches the model when filtered in.
    Api(LlmMessage),
    /// Typed silent payload — UI/transcript only, never sent to the API.
    Silent(SilentPayload),
    /// Discriminator-only — for `FeatureGated` / `RuntimeBookkeeping` kinds.
    Unit,
}

/// Typed structured extras carried alongside an [`AttachmentBody::Api`] body.
///
/// Used for kinds that surface both a rendered model-visible prompt
/// **and** a structured payload that downstream consumers (transcript
/// persistence, SDK observers, telemetry) want preserved verbatim.
/// `body` carries the rendered prompt; `extras` carries the
/// derived-from-structure original data.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AttachmentExtras {
    SkillDiscovery(SkillDiscoveryPayload),
    CompactFileReference(CompactFileReferencePayload),
    /// Display-only summary of the `@`-mentioned files / directories resolved
    /// for a turn. Carried on a `Unit`-body `AttachmentKind::File` message so
    /// the transcript can render a compact `Read <path> (N lines)` /
    /// `Listed directory <path>/` row per item — the model-visible content is
    /// injected separately as `<system-reminder>`-wrapped tool narration, so
    /// this payload never reaches the API.
    MentionSummary(MentionSummaryPayload),
}

/// One resolved `@`-mention, for the transcript's compact display row.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum MentionItemKind {
    /// File read into context.
    #[default]
    File,
    /// File already in context this session (dedup) — no fresh line count.
    AlreadyRead,
    /// Directory listing.
    Directory,
    /// Image file.
    Image,
    /// Large PDF attached as a reference.
    Pdf,
}

/// A single `@`-mention's display metadata.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MentionSummaryItem {
    /// Path relative to CWD at resolution time, for stable display.
    pub display_path: String,
    pub kind: MentionItemKind,
    /// Line count for `File` mentions; page count for `Pdf`. `None` when the
    /// figure is unavailable (dedup / image).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<i32>,
    /// Whether the file content was truncated by the per-attachment budget.
    #[serde(default)]
    pub truncated: bool,
}

/// Display-only payload for [`AttachmentExtras::MentionSummary`].
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MentionSummaryPayload {
    pub items: Vec<MentionSummaryItem>,
}

/// TS parity: `CompactFileReferenceAttachment`
/// (`utils/attachments.ts:307-312`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactFileReferencePayload {
    pub filename: String,
    pub display_path: String,
}

// ─── Silent payloads (one per silent AttachmentKind) ────────────────────

/// Typed payload for silent attachment kinds.
///
/// Variant names map 1:1 to the [`AttachmentKind`](crate::AttachmentKind)
/// silent variants. Adding a new silent kind requires adding a matching
/// variant here — enforced by the constructor helpers on
/// [`AttachmentMessage`](super::AttachmentMessage) +
/// the `silent_kind_round_trips_through_payload` parity test.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SilentPayload {
    // ── Silent events (Coverage::SilentEvent) ──
    HookCancelled(HookCancelledPayload),
    HookErrorDuringExecution(HookErrorDuringExecutionPayload),
    HookNonBlockingError(HookNonBlockingErrorPayload),
    HookSystemMessage(HookSystemMessagePayload),
    HookPermissionDecision(HookPermissionDecisionPayload),
    CommandPermissions(CommandPermissionsPayload),
    StructuredOutput(StructuredOutputPayload),
    DynamicSkill(DynamicSkillPayload),
    MaxTurnsReached(MaxTurnsReachedPayload),

    // ── Silent reminders (Coverage::SilentReminder — in-crate) ──
    AlreadyReadFile(AlreadyReadFilePayload),
    EditedImageFile(EditedImageFilePayload),
}

/// TS parity: `HookCancelledAttachment` (`utils/attachments.ts:396-403`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookCancelledPayload {
    pub hook_name: String,
    pub tool_use_id: String,
    pub hook_event: HookEventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

/// TS parity: `HookErrorDuringExecutionAttachment` (`utils/attachments.ts:405-414`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookErrorDuringExecutionPayload {
    pub content: String,
    pub hook_name: String,
    pub tool_use_id: String,
    pub hook_event: HookEventType,
}

/// TS parity: `HookNonBlockingErrorAttachment` (`utils/attachments.ts:429+`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookNonBlockingErrorPayload {
    pub error: String,
    pub hook_name: String,
    pub tool_use_id: String,
    pub hook_event: HookEventType,
}

/// TS parity: `HookSystemMessageAttachment` (`utils/attachments.ts:388-394`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookSystemMessagePayload {
    pub content: String,
    pub hook_name: String,
    pub tool_use_id: String,
    pub hook_event: HookEventType,
}

/// TS parity: `HookPermissionDecisionAttachment` (`utils/attachments.ts:381-386`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookPermissionDecisionPayload {
    pub decision: HookPermissionDecision,
    pub tool_use_id: String,
    pub hook_event: HookEventType,
}

/// `allow` / `deny` / `ask` decision from hook output.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum HookPermissionDecision {
    #[default]
    Allow,
    Deny,
    Ask,
}

/// TS parity: `command_permissions` (`utils/attachments.ts:605-608`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CommandPermissionsPayload {
    #[serde(rename = "allowedTools")]
    pub allowed_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// TS parity: `structured_output` (`utils/attachments.ts:639+`,
/// `services/tools/toolExecution.ts:1276`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct StructuredOutputPayload {
    pub data: serde_json::Value,
}

/// TS parity: `dynamic_skill` (`utils/attachments.ts:525+`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DynamicSkillPayload {
    #[serde(rename = "skillDir")]
    pub skill_dir: String,
    #[serde(rename = "skillNames")]
    pub skill_names: Vec<String>,
    #[serde(rename = "displayPath")]
    pub display_path: String,
}

/// TS parity: `skill_discovery` (`utils/attachments.ts:538-542`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SkillDiscoveryPayload {
    pub skills: Vec<SkillDiscoverySkill>,
    pub signal: String,
    pub source: SkillDiscoverySource,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillDiscoverySource {
    #[default]
    Native,
    Aki,
    Both,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SkillDiscoverySkill {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "shortId")]
    pub short_id: Option<String>,
}

/// TS parity: `max_turns_reached` (`query.ts:1508`, `query.ts:1707`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MaxTurnsReachedPayload {
    #[serde(rename = "maxTurns")]
    pub max_turns: i32,
    #[serde(rename = "turnCount")]
    pub turn_count: i32,
}

/// TS parity: `AlreadyReadFileAttachment` (`utils/attachments.ts:323-333`).
///
/// TS carries the (potentially truncated) file content inline for UI display
/// even though `normalizeAttachmentForAPI` returns `[]`. coco-rs follows
/// suit — `content` is the last-known file body used by transcript viewers.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AlreadyReadFilePayload {
    /// Absolute or resolved path (engine-populated).
    pub filename: PathBuf,
    /// Path relative to CWD at creation time, for stable display.
    pub display_path: String,
    /// Cached content from `FileReadState` at dedup time.
    #[serde(default)]
    pub content: String,
    /// Whether the content was truncated due to size limits.
    #[serde(default)]
    pub truncated: bool,
}

/// TS parity: `edited_image_file` (`utils/attachments.ts:456-460`).
///
/// Image bytes can't be diffed textually; the UI renders a marker / thumbnail.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EditedImageFilePayload {
    pub filename: PathBuf,
    /// Path relative to CWD at creation time.
    pub display_path: String,
}

#[cfg(test)]
#[path = "attachment_body.test.rs"]
mod tests;
