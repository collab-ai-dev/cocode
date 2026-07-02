//! Speech-to-text seam.
//!
//! coco routes STT through this crate (the vercel-ai runtime seam) so leaf
//! crates (`coco-voice`, `app/cli`) never import `vercel-ai*` directly — the
//! seam guard (`scripts/check-vercel-ai-seam.sh`) enforces that. Provides a
//! typed [`transcribe_audio`] wrapper over `vercel_ai::transcribe` plus an
//! OpenAI-wire model-handle builder.

use std::collections::HashMap;
use std::sync::Arc;

use coco_config::ProviderConfig;
use tokio_util::sync::CancellationToken;
use vercel_ai::transcribe::AudioData;
use vercel_ai::transcribe::TranscribeOptions;
use vercel_ai::transcribe::TranscriptionModel;
use vercel_ai_provider::ProviderOptions;
use vercel_ai_provider::ProviderV4;

use crate::credentials::ProviderCredentialResolver;
use crate::errors::InferenceError;

/// Re-export of the runtime transcription-model trait through the seam. Leaf
/// crates hold `Arc<dyn coco_inference::TranscriptionModelV4>` without a direct
/// `vercel-ai-provider` dependency.
pub use vercel_ai_provider::TranscriptionModelV4;

/// Result of a transcription call.
#[derive(Debug, Clone)]
pub struct TranscriptionOutput {
    /// The recognized text (verbatim from the provider; caller trims).
    pub text: String,
    /// Detected / used language when the provider reports it.
    pub language: Option<String>,
}

/// Transcribe a finished audio buffer (WAV bytes) via an injected model handle.
///
/// `language` is a BCP-47 / ISO-639-1 code, or `None` (or `"auto"`) to let the
/// model auto-detect.
pub async fn transcribe_audio(
    model: Arc<dyn TranscriptionModelV4>,
    audio: Vec<u8>,
    language: Option<String>,
    cancel: CancellationToken,
) -> Result<TranscriptionOutput, InferenceError> {
    let result = vercel_ai::transcribe::transcribe(TranscribeOptions {
        // `from_v4` is mandatory: the `String` path resolves via
        // `get_default_provider()`, which coco never sets.
        model: TranscriptionModel::from_v4(model),
        audio: AudioData::Bytes(audio),
        provider_options: language_provider_options(language.as_deref()),
        max_retries: Some(2),
        headers: None,
        abort_signal: Some(cancel),
    })
    .await
    .map_err(|e| {
        crate::errors::InvalidRequestSnafu {
            message: format!("transcription failed: {e}"),
        }
        .build()
    })?;

    Ok(TranscriptionOutput {
        text: result.text,
        language: result.language,
    })
}

/// Build an OpenAI-wire transcription model handle from a resolved provider
/// config, reusing [`crate::model_factory::build_openai_provider`]'s auth
/// construction (so credentials/base-url stay in the provider layer).
pub fn build_openai_transcription_model(
    provider_cfg: &ProviderConfig,
    resolver: Option<&Arc<dyn ProviderCredentialResolver>>,
    model_id: &str,
    timeout_secs: i64,
) -> Result<Arc<dyn TranscriptionModelV4>, InferenceError> {
    let provider =
        crate::model_factory::build_openai_provider(provider_cfg, resolver, timeout_secs, None)?;
    provider.transcription_model(model_id).map_err(|e| {
        crate::errors::ProviderBuildFailedSnafu {
            provider: "openai",
            provider_name: provider_cfg.name.clone(),
            message: format!("transcription model `{model_id}`: {e}"),
        }
        .build()
    })
}

/// Build `{"openai": {"language": "<code>"}}`, or `None` for auto-detect.
fn language_provider_options(language: Option<&str>) -> Option<ProviderOptions> {
    let language = language?;
    if language.is_empty() || language.eq_ignore_ascii_case("auto") {
        return None;
    }
    let mut openai = HashMap::new();
    openai.insert(
        "language".to_string(),
        serde_json::Value::String(language.to_string()),
    );
    let mut namespaced = HashMap::new();
    namespaced.insert("openai".to_string(), openai);
    Some(ProviderOptions(namespaced))
}

#[cfg(test)]
#[path = "transcription.test.rs"]
mod tests;
