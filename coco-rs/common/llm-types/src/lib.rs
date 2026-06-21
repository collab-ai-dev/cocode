//! Pure DTO re-exports from `vercel-ai-provider`.
//!
//! Scope: the message / content schema that domain crates name. Strictly
//! **data shapes only**. Provider runtime interfaces — `LanguageModelV4`
//! trait, `Provider` trait, model runtime/client machinery, retry, auth, prompt-cache
//! detection — intentionally live in `services/inference` and stay there.
//!
//! Two crates directly depend on `vercel-ai-provider` by design:
//!
//!   - `coco-llm-types`     (this crate) — DTO seam
//!   - `services/inference`              — runtime / client seam
//!
//! Switching the underlying SDK requires editing both. That is the
//! deliberate dual-seam shape; trying to collapse to a single seam would
//! force runtime concerns into a "types-only" crate or schema concerns
//! into a "client" crate. Two narrow seams beats one wide seam.
//!
//! Inclusion criterion: a type belongs here iff it is referenced by a
//! Message-family DTO field or by domain code that names content shapes.
//! Variant-internal sub-types (e.g. `ReasoningFilePart`) that are only
//! reached via `match` patterns don't need re-exporting until a caller
//! actually names them.
//!
//! See `scripts/check-vercel-ai-seam.sh` for the gate.

// === Message envelope ===
pub use vercel_ai_provider::LanguageModelV4Message as LlmMessage;
pub use vercel_ai_provider::LanguageModelV4Prompt as LlmPrompt;

// === Content envelopes (role-keyed) ===
pub use vercel_ai_provider::AssistantContentPart;
pub use vercel_ai_provider::ToolContentPart;
pub use vercel_ai_provider::UserContentPart;

// === Content parts ===
pub use vercel_ai_provider::DataContent;
pub use vercel_ai_provider::FilePart;
pub use vercel_ai_provider::ReasoningPart;
pub use vercel_ai_provider::SharedV4FileData;
pub use vercel_ai_provider::TextPart;
pub use vercel_ai_provider::ToolCallPart;
pub use vercel_ai_provider::ToolInputInvalidReason;
pub use vercel_ai_provider::ToolResultContent;
pub use vercel_ai_provider::ToolResultContentPart;
pub use vercel_ai_provider::ToolResultPart;

// === Provider-extension bags ===
//
// Typed key-value bags carried on content parts / responses. Schema
// for provider-specific extension data, not runtime interfaces.
pub use vercel_ai_provider::ProviderMetadata;
pub use vercel_ai_provider::ProviderOptions;

// === Finish reasons / token usage / response shapes ===
//
// Result-side DTOs that flow back from a provider call and get
// persisted in transcripts. Same DTO-only classification as message
// content parts — single naming authority lives here.
pub use vercel_ai_provider::FinishReason;
pub use vercel_ai_provider::ReasoningLevel;
pub use vercel_ai_provider::ResponseFormat;
pub use vercel_ai_provider::ResponseMetadata;
pub use vercel_ai_provider::UnifiedFinishReason as StopReason;
pub use vercel_ai_provider::Usage;
