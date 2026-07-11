use super::*;
use pretty_assertions::assert_eq;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;

fn provider() -> XaiProvider {
    create_xai(XaiProviderSettings {
        api_key: Some("test-key".into()),
        ..Default::default()
    })
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
    let headers = p.make_config("chat").get_headers();
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    assert!(headers.get("User-Agent").unwrap().contains("ai-sdk/xai/"));
}

#[test]
fn default_base_url_trims_trailing_slash() {
    let p = create_xai(XaiProviderSettings {
        api_key: Some("k".into()),
        base_url: Some("https://custom.example.com/v1/".into()),
        ..Default::default()
    });
    assert_eq!(
        p.make_config("chat").url("/chat/completions"),
        "https://custom.example.com/v1/chat/completions"
    );
}

#[test]
fn custom_headers_are_merged() {
    let mut extra = HashMap::new();
    extra.insert("X-Team".to_string(), "grok".to_string());
    let p = create_xai(XaiProviderSettings {
        api_key: Some("k".into()),
        headers: Some(extra),
        ..Default::default()
    });
    let headers = p.make_config("chat").get_headers();
    assert_eq!(headers.get("X-Team").map(String::as_str), Some("grok"));
}
