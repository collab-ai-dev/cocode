use super::*;

#[test]
fn grok_login_shorthands_select_subscription_provider() {
    assert_eq!(
        instance_name(Some("grok")),
        coco_config::builtin::GROK_PROVIDER
    );
    assert_eq!(
        instance_name(Some("xai-oauth")),
        coco_config::builtin::GROK_PROVIDER
    );
    assert_eq!(instance_name(Some("xai")), "xai");
}

#[test]
fn login_examples_match_each_oauth_provider_family() {
    assert_eq!(example_model(OAuthFlowId::OpenAiChatGpt), "gpt-5.5");
    assert_eq!(
        example_model(OAuthFlowId::GeminiCodeAssist),
        "gemini-2.5-pro"
    );
    assert_eq!(example_model(OAuthFlowId::XaiGrok), "grok-code-fast-1");
}
