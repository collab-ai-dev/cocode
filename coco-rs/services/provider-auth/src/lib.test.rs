use super::*;
use coco_inference::ProviderCredentialResolver;
use coco_types::OAuthFlowId;

fn cred(token: &str, account: &str) -> StoredCredential {
    StoredCredential {
        flow: OAuthFlowId::OpenAiChatGpt,
        access_token: token.into(),
        refresh_token: Some("rt".into()),
        id_token: None,
        account_id: Some(account.into()),
        // Far-future expiry so the background refresher stays idle.
        expires_at_ms: Some(crate::refresh::now_ms() + 86_400_000),
        plan_type: Some("pro".into()),
        email: Some("u@example.com".into()),
        login_epoch: 1,
    }
}

#[test]
fn gemini_flow_is_wired_with_google_specifics() {
    use crate::descriptor::AccountIdSource;
    use crate::descriptor::BodyEncoding;
    use crate::descriptor::RefreshTokenRotation;

    let d = descriptor_for(OAuthFlowId::GeminiCodeAssist).expect("gemini descriptor wired");
    // Desktop-app OAuth carries a client secret (OpenAI does not).
    assert!(d.client_secret.is_some());
    // Google refresh is form-encoded with a persistent refresh token.
    assert!(matches!(d.refresh_encoding, BodyEncoding::Form));
    assert!(matches!(d.refresh_rotation, RefreshTokenRotation::Persists));
    // Account email comes from a userinfo endpoint, not a JWT claim.
    assert!(matches!(
        d.account_id,
        AccountIdSource::UserInfoEndpoint { .. }
    ));
}

#[test]
fn fresh_service_is_not_logged_in() {
    let svc = AuthService::new(Arc::new(EphemeralBackend::default()));
    let st = svc
        .status("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .expect("status");
    assert_eq!(st.state, AuthState::NotConfigured);
    assert_eq!(st.readiness, AuthReadinessLevel::None);
    assert_eq!(st.provider_name, "openai-chatgpt");
    assert!(svc.subscription_creds("openai-chatgpt").is_none());
    assert!(svc.subscription_creds("anything").is_none());
}

/// The headline capability: two configured OpenAI-OAuth instances (e.g. one
/// Responses, one Chat — or two accounts) logged in separately are keyed by
/// their INSTANCE name and resolve independently. A model role bound to either
/// gets that instance's own credentials; an api-key / unconfigured instance
/// reports no supplier.
#[tokio::test]
async fn multiple_instances_of_same_flow_resolve_independently() {
    // Hermetic: point logout's best-effort revoke at a dead local port so it
    // fails instantly instead of reaching the real revocation endpoint.
    unsafe {
        std::env::set_var(
            coco_config::EnvKey::CocoAuthOpenaiRevokeUrl.as_str(),
            "http://127.0.0.1:1/revoke",
        );
    }

    let backend = Arc::new(EphemeralBackend::default());
    backend
        .save("openai-chatgpt", &cred("tok-A", "acct-A"))
        .unwrap();
    backend
        .save("openai-chat-oauth", &cred("tok-B", "acct-B"))
        .unwrap();
    let svc = AuthService::new(backend);

    let supplier_a = svc
        .subscription_creds("openai-chatgpt")
        .expect("instance A logged in");
    let supplier_b = svc
        .subscription_creds("openai-chat-oauth")
        .expect("instance B logged in");
    let a = supplier_a().expect("A creds");
    let b = supplier_b().expect("B creds");
    assert_eq!(a.access_token, "tok-A");
    assert_eq!(a.account_id.as_deref(), Some("acct-A"));
    assert_eq!(b.access_token, "tok-B");
    assert_eq!(b.account_id.as_deref(), Some("acct-B"));

    // An unconfigured / api-key instance has no stored credential → no supplier.
    assert!(svc.subscription_creds("anthropic").is_none());

    // Status is per-instance.
    assert_eq!(
        svc.status("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
            .unwrap()
            .state,
        AuthState::Available
    );
    assert!(svc.logout("openai-chatgpt").await.unwrap());
    assert!(svc.subscription_creds("openai-chatgpt").is_none());
    // Logging one instance out does not affect the other.
    assert!(svc.subscription_creds("openai-chat-oauth").is_some());

    unsafe {
        std::env::remove_var(coco_config::EnvKey::CocoAuthOpenaiRevokeUrl.as_str());
    }
}
