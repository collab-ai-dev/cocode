use super::*;
use pretty_assertions::assert_eq;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::ProviderV4;
use vercel_ai_provider::TranscriptionModelV4;

fn provider() -> GroqProvider {
    create_groq(GroqProviderSettings {
        api_key: Some("test-key".into()),
        ..Default::default()
    })
}

#[test]
fn provider_name_is_groq() {
    assert_eq!(provider().provider(), "groq");
}

#[test]
fn chat_model_carries_sub_provider() {
    let p = provider();
    let chat = p.chat("llama-3.3-70b-versatile");
    assert_eq!(chat.provider(), "groq.chat");
    assert_eq!(chat.model_id(), "llama-3.3-70b-versatile");
}

#[test]
fn transcription_model_carries_sub_provider() {
    let p = provider();
    let t = p.transcription("whisper-large-v3");
    assert_eq!(t.provider(), "groq.transcription");
    assert_eq!(t.model_id(), "whisper-large-v3");
}

#[test]
fn language_model_defaults_to_chat() {
    let p = provider();
    let lm = p
        .language_model("llama-3.3-70b-versatile")
        .expect("language model");
    assert_eq!(lm.provider(), "groq.chat");
}

#[test]
fn embedding_and_image_models_are_unsupported() {
    let p = provider();
    assert!(p.embedding_model("x").is_err());
    assert!(p.image_model("x").is_err());
}

#[test]
fn transcription_model_via_trait() {
    let p = provider();
    assert!(p.transcription_model("whisper-large-v3").is_ok());
}

#[test]
fn headers_include_auth_and_user_agent() {
    let p = provider();
    let headers = p.make_config("chat").get_headers();
    assert_eq!(
        headers.get("Authorization").map(String::as_str),
        Some("Bearer test-key")
    );
    assert!(headers.get("User-Agent").unwrap().contains("ai-sdk/groq/"));
}

#[test]
fn default_base_url_trims_trailing_slash() {
    let p = create_groq(GroqProviderSettings {
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
fn browser_search_tool_is_exported() {
    let tool = crate::browser_search();
    assert_eq!(tool.id, "groq.browser_search");
}
