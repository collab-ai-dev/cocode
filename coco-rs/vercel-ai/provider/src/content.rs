//! Content part types for messages.
//!
//! Content is structured as typed parts that differ based on the message role:
//! - UserContentPart: Text, File
//! - AssistantContentPart: Text, File, Reasoning, ToolCall, ToolResult, Source, ToolApprovalRequest
//! - ToolContentPart: ToolResult, ToolApprovalResponse

use serde::Deserialize;
use serde::Serialize;

use crate::json_value::JSONValue;
use crate::shared::ProviderMetadata;
use crate::shared::ProviderOptions;
use crate::shared::SharedV4FileData;

/// A text content part.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPart {
    /// The text content.
    pub text: String,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl TextPart {
    /// Create a new text part.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            provider_metadata: None,
        }
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A file content part (image, document, etc.).
///
/// `data` is a tagged discriminated union:
/// - `Data { data }` — raw bytes or base64-encoded string.
/// - `Url { url }` — a URL pointing to the file.
/// - `Reference { reference }` — a provider reference (`{ [provider]: id }`).
/// - `Text { text }` — inline text content.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePart {
    /// The file data as a tagged discriminated union.
    pub data: SharedV4FileData,
    /// Either a full IANA media type (`type/subtype`) or just the top-level
    /// segment (e.g. `image`, `audio`).
    pub media_type: String,
    /// Optional filename.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl FilePart {
    /// Create a new file part.
    pub fn new(data: SharedV4FileData, media_type: impl Into<String>) -> Self {
        Self {
            data,
            media_type: media_type.into(),
            filename: None,
            provider_metadata: None,
        }
    }

    /// Create a file part from bytes.
    pub fn from_bytes(bytes: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self::new(SharedV4FileData::data_bytes(bytes), media_type)
    }

    /// Create a file part from a URL.
    pub fn from_url(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::new(SharedV4FileData::url(url), media_type)
    }

    /// Create a file part from base64.
    pub fn from_base64(base64: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::new(SharedV4FileData::data_base64(base64), media_type)
    }

    /// Create an image file part from bytes.
    pub fn image(bytes: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self::from_bytes(bytes, media_type)
    }

    /// Create an image file part from a URL.
    pub fn image_url(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::from_url(url, media_type)
    }

    /// Create an image file part from base64.
    pub fn image_base64(base64: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::from_base64(base64, media_type)
    }

    /// Create a text file part (inline text document).
    pub fn from_text(text: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::new(SharedV4FileData::text(text), media_type)
    }

    /// Set the filename.
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A reasoning content part (for thinking models).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningPart {
    /// The reasoning text.
    pub text: String,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ReasoningPart {
    /// Create a new reasoning part.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            provider_metadata: None,
        }
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A tool call content part.
///
/// `input` always carries the model's best-known emission — even when the
/// call is `invalid`. Adapters that fail JSON parsing fall back to
/// `JSONValue::Object({})` (so schema validation can report
/// specific missing fields; mirrors TS `parsed ?? {}` in
/// `utils/messages.ts:2694`). Adapters that detect a truly unrecoverable
/// `Value::String` payload preserve the raw bytes inside `input` so the
/// agent loop can surface the original emission in diagnostics.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallPart {
    /// The tool call ID.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The tool arguments as JSON. **Always populated** — see struct doc.
    pub input: JSONValue,
    /// Whether the tool call will be executed by the provider.
    /// If this flag is not set or is false, the tool call will be executed by the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    /// `true` when the tool call cannot be executed as emitted: the
    /// raw `arguments` could not be recovered into a parseable shape,
    /// the schema validator rejected the input, or the model named a
    /// tool that does not exist. Pair with [`Self::invalid_reason`]
    /// for the precise cause.
    ///
    /// Caller layers (`app/query` preparer, side queries) read this
    /// flag and emit a synthetic `tool_result(is_error: true)` so the
    /// agent loop's next turn carries a structured error back to the
    /// main LLM for self-correction.
    ///
    /// When `true`, signals that the tool call is structurally invalid.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invalid: bool,
    /// Structured reason accompanying [`Self::invalid`]. Set by
    /// whichever layer first detected the failure (provider adapter
    /// for unrecoverable JSON parse, `app/query::tool_input_validate`
    /// for schema or NoSuchTool failures). `None` when `invalid` is
    /// `false`; required to be `Some(_)` when `invalid` is `true`
    /// (invariant maintained by the constructors below).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_reason: Option<ToolInputInvalidReason>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

/// Structured cause for an invalid tool call. Drives the wrap prefix
/// chosen by `app/query`'s tool result synthesizer:
/// - [`Self::JsonParseFailed`] → `<tool_use_error>JSON parse failed: …</tool_use_error>`
/// - [`Self::SchemaViolation`] → `<tool_use_error>InputValidationError: …</tool_use_error>`
/// - [`Self::NoSuchTool`] → `<tool_use_error>No such tool available: …</tool_use_error>`
///
/// Structured cause for an invalid tool call — three failure modes.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolInputInvalidReason {
    /// Raw `arguments` bytes could not be parsed into JSON, even after
    /// repair. Reserved for the unrecoverable case (e.g. a streaming
    /// buffer that finished with truly malformed content). The common
    /// recoverable failure mode lands in [`Self::SchemaViolation`]
    /// instead because adapters fall back to `{}` on parse failure
    /// and let the schema validator report specific missing fields.
    JsonParseFailed {
        /// Original raw string preserved for diagnostics.
        raw: String,
        /// Parser error string (already redacted to a single line).
        error: String,
    },
    /// Schema validator rejected the input. `message` carries the
    /// LLM-friendly multi-line error already formatted via
    /// `format_schema_error` — error wrap wraps it verbatim inside the
    /// `<tool_use_error>InputValidationError: …</tool_use_error>`
    /// envelope without re-formatting.
    SchemaViolation {
        /// Pre-formatted, LLM-readable error body.
        message: String,
    },
    /// Model emitted a tool name not present in the request's tools
    /// list. Recovery is the agent loop: the next turn's prompt
    /// reminds the model which tools are available.
    NoSuchTool {
        /// The unknown name the model emitted.
        tool_name: String,
    },
}

impl ToolCallPart {
    /// Create a new tool call part. Defaults to `invalid = false`.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: JSONValue,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            input,
            provider_executed: None,
            invalid: false,
            invalid_reason: None,
            provider_metadata: None,
        }
    }

    /// Set whether the tool is executed by the provider.
    pub fn with_provider_executed(mut self, provider_executed: bool) -> Self {
        self.provider_executed = Some(provider_executed);
        self
    }

    /// Mark the tool call as invalid with a structured reason. Sets
    /// `invalid = true` and `invalid_reason = Some(reason)` atomically
    /// so the invariant "invalid ↔ invalid_reason.is_some()" cannot be
    /// broken from a single call site.
    pub fn with_invalid_reason(mut self, reason: ToolInputInvalidReason) -> Self {
        self.invalid = true;
        self.invalid_reason = Some(reason);
        self
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A tool result content part.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPart {
    /// The tool call ID this result is for.
    pub tool_call_id: String,
    /// The tool name.
    pub tool_name: String,
    /// The result content.
    pub output: ToolResultContent,
    /// Whether the tool call was successful.
    #[serde(default)]
    pub is_error: bool,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ToolResultPart {
    /// Create a new tool result part.
    pub fn new(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        output: ToolResultContent,
    ) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            tool_name: tool_name.into(),
            output,
            is_error: false,
            provider_metadata: None,
        }
    }

    /// Mark the result as an error.
    pub fn with_error(mut self) -> Self {
        self.is_error = true;
        self
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// Content of a tool result.
///
/// This matches the LanguageModelV4ToolResultOutput type from the v4 spec.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ToolResultContent {
    /// Text tool output that should be directly sent to the API.
    Text {
        /// The text content.
        value: String,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// JSON tool output.
    Json {
        /// The JSON content.
        value: JSONValue,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// Type when the user has denied the execution of the tool call.
    ExecutionDenied {
        /// Optional reason for the execution denial.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// Error text output.
    ErrorText {
        /// The error message.
        value: String,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// Error JSON output.
    ErrorJson {
        /// The error JSON.
        value: JSONValue,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// Multiple content parts.
    Content {
        /// The content parts.
        value: Vec<ToolResultContentPart>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl ToolResultContent {
    /// Create text content.
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            value: content.into(),
            provider_options: None,
        }
    }

    /// Create JSON content.
    pub fn json(content: JSONValue) -> Self {
        Self::Json {
            value: content,
            provider_options: None,
        }
    }

    /// Create execution denied content.
    pub fn execution_denied(reason: Option<String>) -> Self {
        Self::ExecutionDenied {
            reason,
            provider_options: None,
        }
    }

    /// Create error text content.
    pub fn error_text(message: impl Into<String>) -> Self {
        Self::ErrorText {
            value: message.into(),
            provider_options: None,
        }
    }

    /// Create error JSON content.
    pub fn error_json(error: JSONValue) -> Self {
        Self::ErrorJson {
            value: error,
            provider_options: None,
        }
    }

    /// Create from multiple parts.
    pub fn content_parts(parts: Vec<ToolResultContentPart>) -> Self {
        Self::Content {
            value: parts,
            provider_options: None,
        }
    }
}

impl From<String> for ToolResultContent {
    fn from(s: String) -> Self {
        Self::text(s)
    }
}

impl From<&str> for ToolResultContent {
    fn from(s: &str) -> Self {
        Self::text(s)
    }
}

/// A part of tool result content.
///
/// Matches the `content` array items in `LanguageModelV4ToolResultOutput`
/// from the v4 spec — TS source has 5 variants: `text`, `file-data`,
/// `file-url`, `file-reference`, `custom`. Image / non-image are
/// distinguished by `media_type` (image/png vs application/pdf etc.),
/// not by separate variants.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ToolResultContentPart {
    /// Text content.
    Text {
        /// The text content.
        text: String,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// File data (base64 encoded).
    FileData {
        /// Base-64 encoded media data.
        data: String,
        /// IANA media type.
        media_type: String,
        /// Optional filename.
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// File URL reference.
    FileUrl {
        /// URL of the file.
        url: String,
        /// IANA media type. TS spec carries this; Rust now honors it.
        media_type: String,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// Provider reference (cross-provider file identifier mapping).
    /// Replaces the old `FileId { file_id: ... }` variant; field is
    /// `providerReference` to match the TS spec exactly.
    FileReference {
        /// Provider-specific references for the file (e.g.
        /// `{ "openai": "file-abc", "anthropic": "file-xyz" }`).
        provider_reference: SharedV4ProviderReference,
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
    /// Custom content part for provider-specific content.
    Custom {
        /// Provider-specific options.
        #[serde(skip_serializing_if = "Option::is_none")]
        provider_options: Option<ProviderOptions>,
    },
}

impl ToolResultContentPart {
    /// Create a text content part.
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text {
            text: content.into(),
            provider_options: None,
        }
    }

    /// Create a file data part (base64).
    pub fn file_data(data: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::FileData {
            data: data.into(),
            media_type: media_type.into(),
            filename: None,
            provider_options: None,
        }
    }

    /// Create a file URL part.
    pub fn file_url(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::FileUrl {
            url: url.into(),
            media_type: media_type.into(),
            provider_options: None,
        }
    }

    /// Create a provider reference part from a provider→id map.
    pub fn file_reference(reference: SharedV4ProviderReference) -> Self {
        Self::FileReference {
            provider_reference: reference,
            provider_options: None,
        }
    }
}

/// Cross-provider file identifier mapping — `{provider_name: file_id}`.
pub type SharedV4ProviderReference = std::collections::HashMap<String, String>;

/// User message content parts.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum UserContentPart {
    /// Text content.
    Text(TextPart),
    /// File content (image, document, etc.).
    File(FilePart),
}

impl UserContentPart {
    /// Create a text part.
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text(TextPart::new(content))
    }

    /// Create a file part.
    pub fn file(data: SharedV4FileData, media_type: impl Into<String>) -> Self {
        Self::File(FilePart::new(data, media_type))
    }

    /// Create an image from bytes.
    pub fn image(bytes: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self::File(FilePart::image(bytes, media_type))
    }

    /// Create an image from a URL.
    pub fn image_url(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::File(FilePart::image_url(url, media_type))
    }
}

/// A reasoning file content part (file data that is part of reasoning).
///
/// `data` is a 2-arm tagged union:
/// - `Data { data }` — raw bytes or base64-encoded string.
/// - `Url { url }` — a URL pointing to the file.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningFilePart {
    /// The file data (raw bytes/base64 or URL).
    pub data: crate::language_model::v4::file::LanguageModelV4FileData,
    /// Either a full IANA media type (`type/subtype`) or just the top-level
    /// segment (e.g. `image`, `audio`).
    pub media_type: String,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ReasoningFilePart {
    /// Create a new reasoning file part.
    pub fn new(
        data: crate::language_model::v4::file::LanguageModelV4FileData,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            data,
            media_type: media_type.into(),
            provider_metadata: None,
        }
    }

    /// Create from base64 data.
    pub fn from_base64(base64: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::new(
            crate::language_model::v4::file::LanguageModelV4FileData::base64(base64),
            media_type,
        )
    }

    /// Create from bytes.
    pub fn from_bytes(bytes: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self::new(
            crate::language_model::v4::file::LanguageModelV4FileData::bytes(bytes),
            media_type,
        )
    }

    /// Create from a URL.
    pub fn from_url(url: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::new(
            crate::language_model::v4::file::LanguageModelV4FileData::url(url),
            media_type,
        )
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A custom content part for provider-specific extensions.
///
/// Used in both prompts (with `provider_options`) and responses (with `provider_metadata`).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPart {
    /// The kind of custom content.
    pub kind: String,
    /// Provider-specific options (prompt-side).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_options: Option<ProviderOptions>,
    /// Provider-specific metadata (response-side).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl CustomPart {
    /// Create a new custom part.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            provider_options: None,
            provider_metadata: None,
        }
    }

    /// Add provider options (prompt-side).
    pub fn with_provider_options(mut self, options: ProviderOptions) -> Self {
        self.provider_options = Some(options);
        self
    }

    /// Add provider metadata (response-side).
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// Assistant message content parts.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AssistantContentPart {
    /// Text content.
    Text(TextPart),
    /// File content.
    File(FilePart),
    /// Reasoning content (for thinking models).
    Reasoning(ReasoningPart),
    /// Reasoning file content (file data that is part of reasoning).
    ReasoningFile(ReasoningFilePart),
    /// Custom content for provider-specific extensions.
    Custom(CustomPart),
    /// Tool call.
    ToolCall(ToolCallPart),
    /// Tool result (when assistant receives tool results).
    ToolResult(ToolResultPart),
    /// Source reference (for citations).
    Source(SourcePart),
    /// Tool approval request (for provider-executed tools).
    ToolApprovalRequest(ToolApprovalRequestPart),
}

impl AssistantContentPart {
    /// Create a text part.
    pub fn text(content: impl Into<String>) -> Self {
        Self::Text(TextPart::new(content))
    }

    /// Create a reasoning part.
    pub fn reasoning(content: impl Into<String>) -> Self {
        Self::Reasoning(ReasoningPart::new(content))
    }

    /// Create a tool call part.
    pub fn tool_call(
        tool_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        input: JSONValue,
    ) -> Self {
        Self::ToolCall(ToolCallPart::new(tool_call_id, tool_name, input))
    }

    /// Create a source part.
    pub fn source(id: impl Into<String>, source_type: SourceType) -> Self {
        Self::Source(SourcePart::new(id, source_type))
    }

    /// Create a custom content part.
    pub fn custom(kind: impl Into<String>) -> Self {
        Self::Custom(CustomPart::new(kind))
    }

    /// Create a tool approval request part.
    pub fn tool_approval_request(
        approval_id: impl Into<String>,
        tool_call_id: impl Into<String>,
    ) -> Self {
        Self::ToolApprovalRequest(ToolApprovalRequestPart::new(approval_id, tool_call_id))
    }
}

/// A source reference content part (for citations).
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePart {
    /// The source type (url or document).
    pub source_type: SourceType,
    /// The source ID.
    pub id: String,
    /// The source URL (for URL sources).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The source title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The MIME type (for document sources).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    /// Optional filename (for document sources).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl SourcePart {
    /// Create a new source part.
    pub fn new(id: impl Into<String>, source_type: SourceType) -> Self {
        Self {
            source_type,
            id: id.into(),
            url: None,
            title: None,
            media_type: None,
            filename: None,
            provider_metadata: None,
        }
    }

    /// Create a URL source.
    pub fn url(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            source_type: SourceType::Url,
            id: id.into(),
            url: Some(url.into()),
            title: None,
            media_type: None,
            filename: None,
            provider_metadata: None,
        }
    }

    /// Create a URL source (alias for [`url`](Self::url)).
    pub fn url_source(id: impl Into<String>, url: impl Into<String>) -> Self {
        Self::url(id, url)
    }

    /// Create a document source with title and media type.
    pub fn document(
        id: impl Into<String>,
        title: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        Self {
            source_type: SourceType::Document,
            id: id.into(),
            url: None,
            title: Some(title.into()),
            media_type: Some(media_type.into()),
            filename: None,
            provider_metadata: None,
        }
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }

    /// Set the filename.
    pub fn with_filename(mut self, filename: impl Into<String>) -> Self {
        self.filename = Some(filename.into());
        self
    }
}

/// Types of sources.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// URL source referencing web content.
    Url,
    /// Document source referencing files/documents.
    Document,
}

/// A tool approval request content part (for provider-executed tools).
///
/// This is used for flows where the provider executes the tool (e.g. MCP tools)
/// but requires an explicit user approval before continuing.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApprovalRequestPart {
    /// ID of the approval request. This ID is referenced by the subsequent
    /// tool-approval-response (tool message) to approve or deny execution.
    pub approval_id: String,
    /// The tool call ID that this approval request is for.
    pub tool_call_id: String,
    /// The tool name (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Additional context about the tool call (optional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ToolApprovalRequestPart {
    /// Create a new tool approval request part.
    pub fn new(approval_id: impl Into<String>, tool_call_id: impl Into<String>) -> Self {
        Self {
            approval_id: approval_id.into(),
            tool_call_id: tool_call_id.into(),
            tool_name: None,
            context: None,
            provider_metadata: None,
        }
    }

    /// Set the tool name.
    pub fn with_tool_name(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_name = Some(tool_name.into());
        self
    }

    /// Set additional context.
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = Some(context.into());
        self
    }

    /// Add provider metadata.
    pub fn with_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// Tool message content parts.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToolContentPart {
    /// Tool result.
    ToolResult(ToolResultPart),
    /// Tool approval response.
    ToolApprovalResponse(ToolApprovalResponsePart),
}

/// A tool approval response part (for tools that require approval).
///
/// This contains the user's decision to approve or deny a provider-executed tool call.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolApprovalResponsePart {
    /// ID of the approval request that this response refers to.
    pub approval_id: String,
    /// Whether the approval was granted (true) or denied (false).
    pub approved: bool,
    /// Optional reason for approval or denial.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Provider-specific metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ToolApprovalResponsePart {
    /// Create a new tool approval response.
    pub fn new(approval_id: impl Into<String>, approved: bool) -> Self {
        Self {
            approval_id: approval_id.into(),
            approved,
            reason: None,
            provider_metadata: None,
        }
    }

    /// Add a reason for the approval or denial.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }
}

#[cfg(test)]
#[path = "content.test.rs"]
mod tests;
