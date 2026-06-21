//! Gemini **Code Assist** subscription transport for the Vercel AI SDK (Rust).
//!
//! coco-original — there is **no** `@ai-sdk` Code Assist provider to port. The
//! structure mirrors `@ai-sdk/google-vertex`'s reuse of `@ai-sdk/google`: this
//! crate depends on [`vercel_ai_google`] and reuses its Gemini wire codec
//! (`GoogleGenerativeAILanguageModel::get_args` + `map_response` +
//! `create_google_stream`). It only swaps the *transport*:
//!
//! - base URL `https://cloudcode-pa.googleapis.com/v1internal` with `:method`
//!   RPC routing (`:generateContent` / `:streamGenerateContent`);
//! - `Authorization: Bearer <oauth>` instead of `x-goog-api-key`;
//! - the request body is wrapped in a Code Assist envelope
//!   `{ model, project, user_prompt_id, request: <generateContent body> }`;
//! - the response is unwrapped from `{ response: <generateContent response> }`;
//! - a lazy, cached project-onboarding handshake
//!   (`loadCodeAssist` → `onboardUser` → poll LRO) discovers the GCP project id.
//!
//! The onboarding handshake is the one stateful piece (`Arc<Mutex<…>>`) that
//! does not fit the other adapters' lock-free header-closure model — which is
//! exactly why Code Assist lives in its own crate rather than bloating the
//! faithful `@ai-sdk/google` port.
//!
//! Reads the standard `GOOGLE_CLOUD_PROJECT` / `GOOGLE_CLOUD_PROJECT_ID` env
//! vars (vendor convention, like `vercel-ai-google` reading
//! `GOOGLE_GENERATIVE_AI_API_KEY`) as an onboarding shortcut. Coco-free.

pub mod auth;
pub mod code_assist_types;
pub mod language_model;
pub mod onboarding;
pub mod provider;

pub use auth::CodeAssistCreds;
pub use auth::CodeAssistCredsSupplier;
pub use language_model::GoogleCodeAssistLanguageModel;
pub use provider::GoogleCodeAssistProvider;
pub use provider::GoogleCodeAssistProviderSettings;
pub use provider::create_google_code_assist;

/// Default Code Assist endpoint (host + internal API version).
pub const CODE_ASSIST_BASE_URL: &str = "https://cloudcode-pa.googleapis.com/v1internal";
