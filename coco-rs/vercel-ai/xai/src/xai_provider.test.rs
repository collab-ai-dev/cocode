use super::*;
use crate::GrokCreds;
use pretty_assertions::assert_eq;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;

fn provider() -> XaiProvider {
    create_xai(XaiProviderSettings::api_key(None, Some("test-key".into())))
}

#[test]
fn provider_name_is_xai() {
    assert_eq!(provider().provider(), "xai");
}

#[test]
fn chat_model_carries_sub_provider() {
    let p = provider();
    let chat = p.chat("grok-4.5");
    assert_eq!(chat.provider(), "xai.chat");
    assert_eq!(chat.model_id(), "grok-4.5");
}

#[test]
fn language_model_defaults_to_chat() {
    let p = provider();
    let lm = p.language_model("grok-4.5").expect("language model");
    assert_eq!(lm.provider(), "xai.chat");
}

#[test]
fn embedding_models_are_unsupported() {
    let p = provider();
    assert!(p.embedding_model("x").is_err());
}

#[test]
fn multimodal_models_carry_sub_providers() {
    let p = provider();

    let image = p.image("grok-imagine-image");
    assert_eq!(image.provider(), "xai.image");
    assert_eq!(image.model_id(), "grok-imagine-image");

    let video = p.video("grok-imagine-video");
    assert_eq!(video.provider(), "xai.video");
    assert_eq!(video.model_id(), "grok-imagine-video");

    let speech = p.speech("");
    assert_eq!(speech.provider(), "xai.speech");
    assert_eq!(speech.model_id(), "");

    let transcription = p.transcription("");
    assert_eq!(transcription.provider(), "xai.transcription");
    assert_eq!(transcription.model_id(), "");
}

#[test]
fn provider_trait_exposes_multimodal_models() {
    let p = provider();
    let image = p.image_model("grok-imagine-image").expect("image model");
    assert_eq!(image.provider(), "xai.image");
    let video = p.video_model("grok-imagine-video").expect("video model");
    assert_eq!(video.provider(), "xai.video");
    let speech = p.speech_model("").expect("speech model");
    assert_eq!(speech.provider(), "xai.speech");
    let transcription = p.transcription_model("").expect("transcription model");
    assert_eq!(transcription.provider(), "xai.transcription");
}

#[test]
fn headers_include_auth_and_user_agent() {
    let p = provider();
    let headers = p.make_config("chat", "grok-4.5").get_headers();
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    assert!(headers.get("User-Agent").unwrap().contains("ai-sdk/xai/"));
}

#[test]
fn default_base_url_trims_trailing_slash() {
    let p = create_xai(XaiProviderSettings::api_key(
        Some("https://custom.example.com/v1/".into()),
        Some("k".into()),
    ));
    assert_eq!(
        p.make_config("chat", "grok-4.5").url("/chat/completions"),
        "https://custom.example.com/v1/chat/completions"
    );
}

#[test]
fn custom_headers_are_merged() {
    let mut extra = HashMap::new();
    extra.insert("X-Team".to_string(), "grok".to_string());
    let p = create_xai(XaiProviderSettings {
        connection: XaiConnection::ApiKey {
            base_url: None,
            api_key: Some("k".into()),
        },
        headers: Some(extra),
        client: None,
    });
    let headers = p.make_config("chat", "grok-4.5").get_headers();
    assert_eq!(headers.get("X-Team").map(String::as_str), Some("grok"));
}

#[test]
fn grok_subscription_headers_read_live_credentials() {
    use std::sync::Mutex;

    let token = Arc::new(Mutex::new("first".to_string()));
    let token_for_supplier = token.clone();
    let p = create_xai(XaiProviderSettings::grok_subscription(Arc::new(
        move || {
            Some(GrokCreds {
                access_token: token_for_supplier.lock().expect("token lock").clone(),
            })
        },
    )));

    let first = p.make_config("responses", "grok-code-fast-1").get_headers();
    assert_eq!(
        first.get("Authorization").map(String::as_str),
        Some("Bearer first")
    );
    assert_eq!(
        first.get("X-XAI-Token-Auth").map(String::as_str),
        Some("xai-grok-cli")
    );
    assert_eq!(
        first.get("x-authenticateresponse").map(String::as_str),
        Some("authenticate-response")
    );
    assert_eq!(
        first.get("x-grok-client-identifier").map(String::as_str),
        Some("grok-shell")
    );
    assert_eq!(
        first.get("x-grok-client-mode").map(String::as_str),
        Some("interactive")
    );
    assert_eq!(
        first.get("x-grok-model-override").map(String::as_str),
        Some("grok-code-fast-1")
    );
    assert!(first.contains_key("x-grok-client-version"));
    assert_eq!(
        p.make_config("responses", "grok-code-fast-1")
            .url("/responses"),
        "https://cli-chat-proxy.grok.com/v1/responses"
    );

    *token.lock().expect("token lock") = "refreshed".to_string();
    let refreshed = p.make_config("responses", "grok-code-fast-1").get_headers();
    assert_eq!(
        refreshed.get("Authorization").map(String::as_str),
        Some("Bearer refreshed")
    );
}

#[test]
fn grok_subscription_auth_headers_cannot_be_overridden_by_custom_headers() {
    let custom_headers = HashMap::from([
        ("Authorization".into(), "Bearer attacker-controlled".into()),
        ("X-XAI-Token-Auth".into(), "wrong-mode".into()),
        ("x-grok-model-override".into(), "wrong-model".into()),
    ]);
    let provider = create_xai(XaiProviderSettings {
        connection: XaiConnection::GrokSubscription {
            creds: Arc::new(|| {
                Some(GrokCreds {
                    access_token: "real-token".into(),
                })
            }),
        },
        headers: Some(custom_headers),
        client: None,
    });
    let headers = provider
        .make_config("responses", "grok-code-fast-1")
        .get_headers();

    assert_eq!(headers["Authorization"], "Bearer real-token");
    assert_eq!(headers["X-XAI-Token-Auth"], "xai-grok-cli");
    assert_eq!(headers["x-grok-model-override"], "grok-code-fast-1");
}
