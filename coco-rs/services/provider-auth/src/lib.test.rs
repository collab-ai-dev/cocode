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
        principal: None,
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
fn grok_flow_is_wired_for_device_code_and_rotating_refresh() {
    use crate::descriptor::BodyEncoding;
    use crate::descriptor::OAuthGrant;
    use crate::descriptor::RefreshTokenRotation;

    let descriptor = descriptor_for(OAuthFlowId::XaiGrok).expect("Grok descriptor wired");
    let OAuthGrant::DeviceCode(grant) = descriptor.grant else {
        panic!("Grok must use RFC 8628 device authorization")
    };
    assert_eq!(descriptor.client_id, "b1a00492-073a-47ea-816f-4c329264a828");
    assert_eq!(descriptor.token_url, "https://auth.x.ai/oauth2/token");
    assert_eq!(grant.authorize_url, "https://auth.x.ai/oauth2/device/code");
    assert_eq!(grant.request_extra, &[("referrer", "grok-build")]);
    assert_eq!(grant.client_version_header, Some("x-grok-client-version"));
    assert_eq!(grant.client_surface_header, Some("x-grok-client-surface"));
    assert_eq!(grant.timeout_secs, 600);
    assert_eq!(
        descriptor.scope,
        "openid profile email offline_access grok-cli:access api:access \
         conversations:read conversations:write"
    );
    assert!(matches!(descriptor.refresh_encoding, BodyEncoding::Form));
    assert!(matches!(
        descriptor.refresh_rotation,
        RefreshTokenRotation::Rotates
    ));
}

#[test]
fn build_is_official_distribution_is_false_in_test_builds() {
    // No local build sets COCO_BUILD_OFFICIAL, so a test binary must classify as
    // unofficial — this is what keeps `with_config_dir` on the file backend and
    // off the OS keychain, so headless PTY e2e tests never block on a macOS
    // "allow access" prompt. Guards against inverting the gate.
    assert!(!build_is_official_distribution());
}

#[test]
fn official_build_flag_accepts_only_exact_one() {
    // The predicate behind `build_is_official_distribution`, tested at both
    // polarities — the compile-time env var it reads is fixed for any given
    // build, so this is the only place the `true` branch is reachable.
    //
    // Everything but "1" must stay unofficial: this gate previously keyed on
    // `!cfg!(debug_assertions)`, which silently classified every local
    // `cargo build --release` as an official artifact and sent it to the
    // keychain. Opt-in and exact is what makes that unrepresentable.
    assert!(official_build_flag("1"));
    for raw in ["", "0", "true", "TRUE", "yes", "2", " 1"] {
        assert!(!official_build_flag(raw), "{raw:?} must not be official");
    }
}

#[tokio::test]
async fn with_config_dir_credentials_live_under_the_config_dir() {
    // A (debug/test) build picks the file-only backend, so a credential written
    // under <config_dir>/auth is authoritative and fully isolated — this is what
    // lets e2e tests repoint COCO_CONFIG_DIR at a temp dir instead of leaking
    // into (or blocking on) the real OS keychain.
    let tmp = tempfile::tempdir().unwrap();
    crate::store::FileBackend::new(tmp.path().join("auth"))
        .save("openai-chatgpt", &cred("at", "acct"))
        .unwrap();

    let svc = AuthService::with_config_dir(tmp.path().to_path_buf());
    let st = svc
        .status("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .expect("status");
    assert_eq!(st.state, AuthState::Available);
    assert_eq!(st.email.as_deref(), Some("u@example.com"));
}

#[tokio::test]
async fn with_store_file_reads_credentials_from_config_dir() {
    // An explicit `file` mode roots the store at <config_dir>/auth regardless of
    // build provenance — this is the escape hatch for a locally-built (unsigned)
    // `--release` binary that would otherwise hit the keychain.
    let tmp = tempfile::tempdir().unwrap();
    crate::store::FileBackend::new(tmp.path().join("auth"))
        .save("openai-chatgpt", &cred("at", "acct"))
        .unwrap();
    let svc = AuthService::with_store(
        tmp.path().to_path_buf(),
        coco_config::CredentialStoreMode::File,
    );
    let st = svc
        .status("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .expect("status");
    assert_eq!(st.state, AuthState::Available);
}

#[test]
fn with_store_ephemeral_ignores_persisted_credentials() {
    // Seed an on-disk credential; the ephemeral backend must not read it.
    let tmp = tempfile::tempdir().unwrap();
    crate::store::FileBackend::new(tmp.path().join("auth"))
        .save("openai-chatgpt", &cred("at", "acct"))
        .unwrap();
    let svc = AuthService::with_store(
        tmp.path().to_path_buf(),
        coco_config::CredentialStoreMode::Ephemeral,
    );
    let st = svc
        .status("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .expect("status");
    assert_eq!(st.state, AuthState::NotConfigured);
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
    assert!(
        svc.subscription_creds("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
            .is_none()
    );
    assert!(
        svc.subscription_creds("anything", OAuthFlowId::OpenAiChatGpt)
            .is_none()
    );
}

#[test]
fn resolver_rejects_credentials_from_a_different_oauth_flow() {
    let backend = Arc::new(EphemeralBackend::default());
    backend
        .save("shared-name", &cred("openai-token", "acct"))
        .unwrap();
    let service = AuthService::new(backend);

    assert!(
        service
            .subscription_creds("shared-name", OAuthFlowId::XaiGrok)
            .is_none()
    );
}

struct RejectingSaveBackend {
    credential: StoredCredential,
}

impl CredentialBackend for RejectingSaveBackend {
    fn load(&self, _name: &str) -> Result<Option<StoredCredential>> {
        Ok(Some(self.credential.clone()))
    }

    fn save(&self, _name: &str, _cred: &StoredCredential) -> Result<()> {
        Err(crate::error::StoreSnafu {
            message: "injected save failure".to_string(),
        }
        .build())
    }

    fn delete(&self, _name: &str) -> Result<bool> {
        Ok(false)
    }
}

#[tokio::test]
async fn failed_durable_import_does_not_publish_new_live_token() {
    let backend = Arc::new(RejectingSaveBackend {
        credential: cred("durable-old", "acct-old"),
    });
    let service = AuthService::new(backend);
    let before = service
        .subscription_creds("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .expect("old credential loaded");

    let error = service
        .import("openai-chatgpt", cred("new-but-uncommitted", "acct-new"))
        .await
        .expect_err("save failure must abort import");

    assert!(error.to_string().contains("injected save failure"));
    assert_eq!(
        before().expect("old credential remains live").access_token,
        "durable-old"
    );
}

#[tokio::test]
async fn process_refresh_lock_serializes_independent_callers() {
    let directory = tempfile::tempdir().unwrap();
    let first = process_lock::acquire(Some(directory.path()), "grok")
        .await
        .expect("first lock");
    let path = directory.path().to_path_buf();
    let mut second = tokio::spawn(async move { process_lock::acquire(Some(&path), "grok").await });

    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut second)
            .await
            .is_err(),
        "second process-style caller must wait for the file lock"
    );
    drop(first);
    second
        .await
        .expect("lock task joins")
        .expect("second lock succeeds after release");
}

#[tokio::test]
async fn reactive_refresh_rotates_durable_and_live_credentials_together() {
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access",
            "refresh_token": "fresh-refresh",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&server)
        .await;

    let backend = Arc::new(EphemeralBackend::default());
    backend
        .save("openai-chatgpt", &cred("rejected-access", "acct"))
        .unwrap();
    let service = AuthService::new(backend.clone());
    service
        .subscription_creds("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .expect("credential is managed");

    let mut descriptor = crate::descriptor::OPENAI_CHATGPT;
    descriptor.token_url = Box::leak(format!("{}/token", server.uri()).into_boxed_str());
    let descriptor = Box::leak(Box::new(descriptor));
    service
        .lock_providers()
        .get_mut("openai-chatgpt")
        .expect("managed provider")
        .descriptor = descriptor;

    assert!(service.refresh_now("openai-chatgpt").await);
    let live = service
        .subscription_creds("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .unwrap()()
    .unwrap();
    let durable = backend.load("openai-chatgpt").unwrap().unwrap();
    assert_eq!(live.access_token, "fresh-access");
    assert_eq!(durable.access_token, "fresh-access");
    assert_eq!(durable.refresh_token.as_deref(), Some("fresh-refresh"));
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
        .subscription_creds("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
        .expect("instance A logged in");
    let supplier_b = svc
        .subscription_creds("openai-chat-oauth", OAuthFlowId::OpenAiChatGpt)
        .expect("instance B logged in");
    let a = supplier_a().expect("A creds");
    let b = supplier_b().expect("B creds");
    assert_eq!(a.access_token, "tok-A");
    assert_eq!(a.account_id.as_deref(), Some("acct-A"));
    assert_eq!(b.access_token, "tok-B");
    assert_eq!(b.account_id.as_deref(), Some("acct-B"));

    // An unconfigured / api-key instance has no stored credential → no supplier.
    assert!(
        svc.subscription_creds("anthropic", OAuthFlowId::OpenAiChatGpt)
            .is_none()
    );

    // Status is per-instance.
    assert_eq!(
        svc.status("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
            .unwrap()
            .state,
        AuthState::Available
    );
    assert!(svc.logout("openai-chatgpt").await.unwrap());
    assert!(
        svc.subscription_creds("openai-chatgpt", OAuthFlowId::OpenAiChatGpt)
            .is_none()
    );
    // Logging one instance out does not affect the other.
    assert!(
        svc.subscription_creds("openai-chat-oauth", OAuthFlowId::OpenAiChatGpt)
            .is_some()
    );

    unsafe {
        std::env::remove_var(coco_config::EnvKey::CocoAuthOpenaiRevokeUrl.as_str());
    }
}
