//! Serde types for the Code Assist wire contract: the generate envelope and
//! the onboarding handshake (`loadCodeAssist` / `onboardUser` / LRO).
//!
//! The inner `generateContent` request/response shapes are NOT re-modeled here:
//! the request is the verbatim body produced by
//! `vercel_ai_google::GoogleGenerativeAILanguageModel::get_args` (a
//! `serde_json::Value`), and the response reuses
//! [`vercel_ai_google::GoogleGenerateContentResponse`]. Ported from jcode's
//! `jcode-provider-gemini` types.

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use vercel_ai_google::GoogleGenerateContentResponse;

/// Free-tier id used when the account has no current tier yet.
pub const USER_TIER_FREE: &str = "free-tier";
/// Legacy-tier id used as the onboarding fallback when no default tier is
/// offered (jcode parity — `choose_onboard_tier`).
pub const USER_TIER_LEGACY: &str = "legacy-tier";

/// Client identification sent on the onboarding calls (jcode parity, incl.
/// `duetProject`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientMetadata {
    pub ide_type: &'static str,
    pub platform: &'static str,
    pub plugin_type: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duet_project: Option<String>,
}

impl Default for ClientMetadata {
    fn default() -> Self {
        Self {
            ide_type: "IDE_UNSPECIFIED",
            platform: "PLATFORM_UNSPECIFIED",
            plugin_type: "GEMINI",
            duet_project: None,
        }
    }
}

/// Build client metadata carrying the (optional) GCP project as `duetProject`.
pub fn client_metadata(project: Option<String>) -> ClientMetadata {
    ClientMetadata {
        duet_project: project,
        ..ClientMetadata::default()
    }
}

/// `:generateContent` / `:streamGenerateContent` request envelope. Field names
/// match jcode's working client (no `rename_all`): `user_prompt_id` is wire
/// snake_case; `request` is the verbatim Gemini `generateContent` body.
#[derive(Debug, Clone, Serialize)]
pub struct CodeAssistGenerateRequest {
    pub model: String,
    pub project: String,
    pub user_prompt_id: String,
    pub request: Value,
}

/// `:generateContent` response envelope. `response` is the standard Gemini
/// `generateContent` response (reused from `vercel-ai-google`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAssistGenerateResponse {
    #[serde(default)]
    pub trace_id: Option<String>,
    #[serde(default)]
    pub response: Option<GoogleGenerateContentResponse>,
}

// ── Onboarding handshake ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadCodeAssistRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloudaicompanion_project: Option<String>,
    pub metadata: ClientMetadata,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadCodeAssistResponse {
    #[serde(default)]
    pub current_tier: Option<GeminiUserTier>,
    #[serde(default)]
    pub allowed_tiers: Option<Vec<GeminiUserTier>>,
    #[serde(default)]
    pub ineligible_tiers: Option<Vec<IneligibleTier>>,
    #[serde(default)]
    pub cloudaicompanion_project: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiUserTier {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub is_default: Option<bool>,
}

/// A tier the account is NOT eligible for, with the reason (and, for
/// `VALIDATION_REQUIRED`, a URL the user must visit).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IneligibleTier {
    #[serde(default)]
    pub reason_code: Option<String>,
    #[serde(default)]
    pub reason_message: Option<String>,
    #[serde(default)]
    pub validation_url: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardUserRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloudaicompanion_project: Option<String>,
    pub metadata: ClientMetadata,
}

/// Long-running-operation envelope returned by `onboardUser` and the poll GET.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LongRunningOperationResponse {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub done: Option<bool>,
    #[serde(default)]
    pub response: Option<OnboardUserResponse>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OnboardUserResponse {
    #[serde(default)]
    pub cloudaicompanion_project: Option<ProjectRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectRef {
    #[serde(default)]
    pub id: Option<String>,
}

#[cfg(test)]
#[path = "code_assist_types.test.rs"]
mod tests;
