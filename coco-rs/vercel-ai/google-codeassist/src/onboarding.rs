//! Lazy project-onboarding handshake: `loadCodeAssist` → (if needed)
//! `onboardUser` → poll the long-running operation until it yields the GCP
//! project id. Ported from jcode's `ensure_state`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use vercel_ai_provider::AISdkError;
use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::generate_id;
use vercel_ai_provider_utils::get_from_api_with_client;
use vercel_ai_provider_utils::post_json_to_api_with_client;

use vercel_ai_google::GoogleFailedResponseHandler;

use crate::code_assist_types::GeminiUserTier;
use crate::code_assist_types::LoadCodeAssistRequest;
use crate::code_assist_types::LoadCodeAssistResponse;
use crate::code_assist_types::LongRunningOperationResponse;
use crate::code_assist_types::OnboardUserRequest;
use crate::code_assist_types::USER_TIER_FREE;
use crate::code_assist_types::USER_TIER_LEGACY;
use crate::code_assist_types::client_metadata;

/// Resolved Code Assist session state — discovered once, cached per model.
#[derive(Debug, Clone)]
pub struct OnboardingState {
    pub project_id: String,
    pub session_id: String,
}

const LRO_POLL_INTERVAL_SECS: u64 = 2;
const LRO_MAX_POLLS: u32 = 30;

/// Bearer + JSON content-type headers for a Code Assist request.
pub(crate) fn auth_headers(access_token: &str) -> HashMap<String, String> {
    let mut h = HashMap::new();
    h.insert(
        "Authorization".to_string(),
        format!("Bearer {access_token}"),
    );
    h.insert("Content-Type".to_string(), "application/json".to_string());
    h
}

fn project_from_env() -> Option<String> {
    std::env::var("GOOGLE_CLOUD_PROJECT")
        .ok()
        .or_else(|| std::env::var("GOOGLE_CLOUD_PROJECT_ID").ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Run the full handshake against `base_url` (`…/v1internal`), returning the
/// discovered project id plus a fresh session id. Mirrors jcode's
/// `setup_runtime_state`.
pub async fn run_onboarding(
    client: Arc<reqwest::Client>,
    base_url: &str,
    access_token: &str,
) -> Result<OnboardingState, AISdkError> {
    let headers = auth_headers(access_token);
    let env_project = project_from_env();
    let metadata = client_metadata(env_project.clone());

    // 1. loadCodeAssist — discover tier + any existing project.
    let load_body = serde_json::to_value(LoadCodeAssistRequest {
        cloudaicompanion_project: env_project.clone(),
        metadata: metadata.clone(),
    })
    .map_err(|e| AISdkError::new(format!("encode loadCodeAssist: {e}")))?;
    let load: LoadCodeAssistResponse = post_json(
        &format!("{base_url}:loadCodeAssist"),
        &headers,
        &load_body,
        &client,
    )
    .await?;
    // Surface a `VALIDATION_REQUIRED` ineligibility as an actionable error.
    validate_load_response(&load)?;

    let project_id = if load.current_tier.is_some() {
        // 2. Already onboarded → use the reported project (or the env override).
        load.cloudaicompanion_project
            .clone()
            .or(env_project.clone())
            .ok_or_else(|| ineligible_or_project_error(&load))?
    } else {
        // 3. Onboard: the default allowed tier (else legacy-tier). The free tier
        // sends no project; paid/legacy tiers send the env project.
        let tier = choose_onboard_tier(&load);
        let is_free = tier.id.as_deref() == Some(USER_TIER_FREE);
        let onboard_req = OnboardUserRequest {
            tier_id: tier.id.clone(),
            cloudaicompanion_project: if is_free { None } else { env_project.clone() },
            metadata: if is_free {
                client_metadata(None)
            } else {
                metadata.clone()
            },
        };
        let onboard_body = serde_json::to_value(&onboard_req)
            .map_err(|e| AISdkError::new(format!("encode onboardUser: {e}")))?;
        let mut lro: LongRunningOperationResponse = post_json(
            &format!("{base_url}:onboardUser"),
            &headers,
            &onboard_body,
            &client,
        )
        .await?;

        // 4. Poll the long-running operation until done (bounded).
        let mut polls = 0;
        while !lro.done.unwrap_or(false) {
            if polls >= LRO_MAX_POLLS {
                return Err(AISdkError::new(
                    "Code Assist: onboarding did not complete in time",
                ));
            }
            let Some(op_name) = lro.name.clone() else {
                break;
            };
            tokio::time::sleep(Duration::from_secs(LRO_POLL_INTERVAL_SECS)).await;
            // jcode trims a leading `/` from the operation name.
            let op_path = op_name.trim_start_matches('/');
            lro = get_json(&format!("{base_url}/{op_path}"), &headers, &client).await?;
            polls += 1;
        }

        lro.response
            .and_then(|r| r.cloudaicompanion_project)
            .and_then(|p| p.id)
            .or(env_project.clone())
            .ok_or_else(|| ineligible_or_project_error(&load))?
    };

    Ok(OnboardingState {
        project_id,
        session_id: generate_id("session"),
    })
}

/// Error out when the account has no current tier and an ineligible tier
/// reports `VALIDATION_REQUIRED` with a URL (jcode parity).
fn validate_load_response(res: &LoadCodeAssistResponse) -> Result<(), AISdkError> {
    if res.current_tier.is_none()
        && let Some(v) = res.ineligible_tiers.as_ref().and_then(|tiers| {
            tiers.iter().find(|t| {
                t.reason_code.as_deref() == Some("VALIDATION_REQUIRED")
                    && t.validation_url.is_some()
            })
        })
    {
        let desc = v
            .reason_message
            .clone()
            .unwrap_or_else(|| "Account validation required".to_string());
        let url = v.validation_url.clone().unwrap_or_default();
        return Err(AISdkError::new(format!(
            "{desc}. Complete account validation: {url}"
        )));
    }
    Ok(())
}

/// The default allowed tier, else `legacy-tier` (jcode parity).
fn choose_onboard_tier(res: &LoadCodeAssistResponse) -> GeminiUserTier {
    if let Some(default_tier) = res.allowed_tiers.as_ref().and_then(|tiers| {
        tiers
            .iter()
            .find(|t| t.is_default.unwrap_or(false))
            .cloned()
    }) {
        return default_tier;
    }
    GeminiUserTier {
        id: Some(USER_TIER_LEGACY.to_string()),
        is_default: None,
    }
}

/// A helpful error when onboarding yields no project: prefer the ineligible
/// reasons, else point at the `GOOGLE_CLOUD_PROJECT` env vars (jcode parity).
fn ineligible_or_project_error(res: &LoadCodeAssistResponse) -> AISdkError {
    if let Some(reasons) = res
        .ineligible_tiers
        .as_ref()
        .filter(|tiers| !tiers.is_empty())
    {
        let joined = reasons
            .iter()
            .filter_map(|t| t.reason_message.as_deref())
            .collect::<Vec<_>>()
            .join(", ");
        if !joined.is_empty() {
            return AISdkError::new(joined);
        }
    }
    AISdkError::new(
        "Gemini Code Assist requires setting GOOGLE_CLOUD_PROJECT or GOOGLE_CLOUD_PROJECT_ID \
         for this account. See the Gemini Code Assist auth docs.",
    )
}

async fn post_json<T: DeserializeOwned + Send + Sync + 'static>(
    url: &str,
    headers: &HashMap<String, String>,
    body: &Value,
    client: &Arc<reqwest::Client>,
) -> Result<T, AISdkError> {
    post_json_to_api_with_client(
        url,
        Some(headers.clone()),
        body,
        JsonResponseHandler::new(),
        GoogleFailedResponseHandler,
        None,
        Some(client.clone()),
    )
    .await
}

async fn get_json<T: DeserializeOwned + Send + Sync + 'static>(
    url: &str,
    headers: &HashMap<String, String>,
    client: &Arc<reqwest::Client>,
) -> Result<T, AISdkError> {
    get_from_api_with_client(
        url,
        Some(headers.clone()),
        JsonResponseHandler::new(),
        GoogleFailedResponseHandler,
        None,
        Some(client.clone()),
    )
    .await
}
