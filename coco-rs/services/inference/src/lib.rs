//! LLM inference client via vercel-ai, retry engine, auth, rate limiting.
//!
//! `ModelRuntimeRegistry` wraps provider models and owns all LLM call
//! policy: retries, fallback, recovery, prompt-cache detection.
//!
//! This crate is the **runtime seam** for vercel-ai. It owns the runtime
//! contract (`LanguageModelV4` trait, `LanguageModelCallOptions`,
//! GenerateResult / StreamResult, `Provider` trait) and the client
//! machinery (private client, retry, auth, prompt-cache detection).
//!
//! DTOs (message envelope, content parts, ProviderOptions, StopReason,
//! Usage, …) live in `coco-llm-types`. Together they form the
//! dual-seam: two narrow crates own direct `vercel-ai-provider`
//! dependencies, an SDK upgrade edits both. See
//! `scripts/check-vercel-ai-seam.sh`.

pub mod auth;
pub mod build_call_options;
pub mod cache_convert;
pub mod cache_detection;
mod client;
pub mod credentials;
pub mod errors;
pub mod fingerprint;
pub mod header_template;
pub mod logging;
pub mod lsp_integration;
pub mod model_factory;
pub mod model_runtime;
pub mod prompt_layout;
pub mod retry;
pub mod stream;
pub mod thinking_convert;
pub mod usage;

pub use build_call_options::PerCallOverrides;
pub use build_call_options::build_call_options;
pub use build_call_options::build_call_options_with_extra;
pub use cache_detection::CacheBreakDetector;
pub use cache_detection::CacheBreakResult;
pub use cache_detection::CacheState;
pub use cache_detection::PromptStateInput;
pub use client::QueryParams;
pub use client::QueryResult;
pub use credentials::ProviderCredentialResolver;
pub use credentials::RefreshFuture;
pub use credentials::RefreshHook;
pub use credentials::SubscriptionCreds;
pub use credentials::SubscriptionCredsSupplier;
pub use errors::InferenceError;
pub use fingerprint::ProviderClientFingerprint;
pub use header_template::HeaderVars;
pub use header_template::PerBuildVars;
pub use header_template::TemplateError;
pub use logging::ErrorLog;
pub use logging::KnownGateway;
pub use logging::RequestLog;
pub use logging::ResponseLog;
pub use logging::detect_gateway;
pub use logging::format_request_log;
pub use logging::format_response_log;
pub use model_runtime::ModelCallHandle;
pub use model_runtime::ModelCommunicationOutcome;
pub use model_runtime::ModelFallbackReason;
pub use model_runtime::ModelRuntime;
pub use model_runtime::ModelRuntimeClient;
pub use model_runtime::ModelRuntimeEvent;
pub use model_runtime::ModelRuntimeFeedbackOutcome;
pub use model_runtime::ModelRuntimeQueryOutcome;
pub use model_runtime::ModelRuntimeRegistry;
pub use model_runtime::ModelRuntimeSnapshot;
pub use model_runtime::ModelRuntimeSource;
pub use model_runtime::ModelStreamOpenOutcome;
pub use model_runtime::PrebuiltLanguageModelSlot;
pub use prompt_layout::AnthropicCacheControl;
pub use prompt_layout::AnthropicSystemBlock;
pub use prompt_layout::CacheHint;
pub use prompt_layout::PROMPT_LAYOUT_NAMESPACE;
pub use prompt_layout::PromptEnvelope;
pub use prompt_layout::PromptHashInputs;
pub use prompt_layout::PromptLayoutOptions;
pub use prompt_layout::PromptPart;
pub use prompt_layout::PromptSection;
pub use prompt_layout::PromptSectionKind;
pub use prompt_layout::PromptSource;
pub use prompt_layout::build_prompt_layout_from_prompt;
pub use prompt_layout::put_layout_options;
pub use prompt_layout::take_layout_options;
pub use retry::RetryConfig;
pub use stream::AssistantTurnSnapshot;
pub use stream::CustomSegment;
pub use stream::FileSegment;
pub use stream::ReasoningFileSegment;
pub use stream::ReasoningSegment;
pub use stream::SourceSegment;
pub use stream::StreamEvent;
pub use stream::StreamMetrics;
pub use stream::StreamProcessorConfig;
pub use stream::TextSegment;
pub use stream::ToolApprovalRequestSegment;
pub use stream::ToolCallSegment;
pub use stream::TurnPart;
pub use stream::default_process_stream_config;
pub use stream::synthetic_error_stream;
pub use stream::synthetic_error_stream_after_text;
pub use stream::synthetic_stream_from_content;
pub use thinking_convert::to_extra_body;
pub use usage::UsageAccumulator;

// ─── Vercel-ai re-export hub ──────────────────────────────────────────────
//
// Runtime contract: things callers of the model runtime and the
// generic agent loop name (`LanguageModel` trait, call options, results,
// errors, usage). DTOs (message envelope, content parts, ProviderOptions)
// live in `coco-llm-types` and are NOT re-exported here — see the
// dual-seam rationale in `scripts/check-vercel-ai-seam.sh`.
//
// Naming convention: types whose vercel-ai name carries a version
// digit are renamed to strip the digit so `vercel-ai` upgrades (V4 → V5)
// stay local to this file.

// Language-model protocol family — runtime/contract surface.
pub use vercel_ai_provider::LanguageModelV4 as LanguageModel;
pub use vercel_ai_provider::LanguageModelV4CallOptions as LanguageModelCallOptions;
pub use vercel_ai_provider::LanguageModelV4GenerateResult as LanguageModelGenerateResult;
pub use vercel_ai_provider::LanguageModelV4StreamResult as LanguageModelStreamResult;
pub use vercel_ai_provider::LanguageModelV4Tool as LanguageModelTool;
pub use vercel_ai_provider::LanguageModelV4ToolChoice as LanguageModelToolChoice;
pub use vercel_ai_provider::ProviderV4 as Provider;
pub use vercel_ai_provider::ResponseFormat;
pub use vercel_ai_provider::language_model::v4::LanguageModelV4FunctionTool as LanguageModelFunctionTool;
pub use vercel_ai_provider::language_model::v4::LanguageModelV4ProviderTool as LanguageModelProviderTool;
// OpenAI custom-grammar tool constructor — the provider-specific realization of
// a Freeform `ToolSpec` (apply_patch). Kept in the provider crate so callers
// pass only neutral (name, description, syntax, definition).
pub use vercel_ai_openai::tool::openai_custom_grammar_tool;

// Provider-internal content variants not part of the DTO seam (used by
// vercel-ai's own conversion code and by streaming-side rebuild logic
// inside this crate — kept here, not promoted to coco-llm-types).
pub use vercel_ai_provider::CustomPart;
pub use vercel_ai_provider::FileRawData;
pub use vercel_ai_provider::ReasoningFilePart;

// Errors + primitive — runtime/error-shape, not DTO. Stay in inference.
pub use vercel_ai_provider::AISdkError;
pub use vercel_ai_provider::JSONValue;
// Raw wire-traffic debug sink interface (no version digit → passes
// through unchanged). Runtime-seam re-export so consumers like
// `app/query` reach it via `coco_inference::*` instead of declaring a
// `vercel-ai*` dep (enforced by `check-vercel-ai-seam.sh`).
pub use vercel_ai_provider::WireTap;
pub use vercel_ai_provider::WireTapHandle;

/// One-line `use coco_inference::prelude::*;` to bring the common subset
/// of model-runtime + LLM types into scope.
pub mod prelude {
    pub use crate::LanguageModel;
    pub use crate::LanguageModelCallOptions;
    pub use crate::LanguageModelGenerateResult;
    pub use crate::LanguageModelStreamResult;
    pub use crate::LanguageModelTool;
    pub use crate::QueryParams;
    pub use crate::QueryResult;
    pub use crate::StreamEvent;
    pub use crate::StreamMetrics;
    pub use crate::StreamProcessorConfig;
    pub use coco_llm_types::AssistantContentPart;
    pub use coco_llm_types::FinishReason;
    pub use coco_llm_types::LlmMessage;
    pub use coco_llm_types::LlmPrompt;
    pub use coco_llm_types::TextPart;
    pub use coco_llm_types::ToolCallPart;
    pub use coco_llm_types::Usage;
    pub use coco_llm_types::UserContentPart;
}
