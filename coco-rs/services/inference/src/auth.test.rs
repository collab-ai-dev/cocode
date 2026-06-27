use super::*;

fn make_test_tokens() -> OAuthTokens {
    OAuthTokens {
        access_token: "test-token".into(),
        refresh_token: Some("refresh-token".into()),
        expires_at: Some(i64::MAX),
        subscription_type: Some("pro".into()),
        org_uuid: Some("org-123".into()),
    }
}

#[test]
fn test_oauth_tokens_not_expired() {
    let tokens = OAuthTokens {
        access_token: "abc".into(),
        refresh_token: None,
        expires_at: Some(i64::MAX),
        subscription_type: None,
        org_uuid: None,
    };
    assert!(!tokens.is_expired(1000));
    assert!(!tokens.needs_refresh(1000));
}

#[test]
fn test_oauth_tokens_expired() {
    let tokens = OAuthTokens {
        access_token: "abc".into(),
        refresh_token: None,
        expires_at: Some(500),
        subscription_type: None,
        org_uuid: None,
    };
    assert!(tokens.is_expired(1000));
    assert!(tokens.needs_refresh(1000));
}

#[test]
fn test_oauth_tokens_needs_refresh_within_window() {
    let tokens = OAuthTokens {
        access_token: "abc".into(),
        refresh_token: Some("refresh".into()),
        expires_at: Some(1_200_000), // 1200 seconds from epoch
        subscription_type: None,
        org_uuid: None,
    };
    // Within 5 min (300s = 300_000ms) of expiry.
    assert!(!tokens.is_expired(900_000));
    assert!(tokens.needs_refresh(900_001));
}

#[test]
fn test_resolve_auth_api_key() {
    // Save and restore env to avoid leaking state.
    let prev = std::env::var("ANTHROPIC_API_KEY").ok();
    // SAFETY: Test-only; single-threaded test runner for this test.
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", "test-key-123") };

    let auth = resolve_auth_from_env();
    assert!(matches!(auth, Some(AuthMethod::ApiKey { ref key }) if key == "test-key-123"));

    match prev {
        Some(v) => unsafe { std::env::set_var("ANTHROPIC_API_KEY", v) },
        None => unsafe { std::env::remove_var("ANTHROPIC_API_KEY") },
    }
}

#[test]
fn test_is_first_party_auth() {
    assert!(is_first_party_auth(&AuthMethod::ApiKey { key: "k".into() }));
    assert!(is_first_party_auth(&AuthMethod::OAuth(OAuthTokens {
        access_token: "t".into(),
        refresh_token: None,
        expires_at: None,
        subscription_type: None,
        org_uuid: None,
    })));
    assert!(!is_first_party_auth(&AuthMethod::Bedrock {
        region: "us-east-1".into(),
        profile: None,
    }));
}

#[test]
fn test_api_key_cache_hit() {
    // Insert a key directly into cache.
    if let Ok(mut cache) = api_key_cache().lock() {
        cache.insert(
            "echo test-cached-key".to_string(),
            CachedApiKey {
                key: "cached-value".into(),
                fetched_at: Instant::now(),
            },
        );
    }
    let result = get_api_key_from_helper("echo test-cached-key");
    assert_eq!(result, Some("cached-value".into()));
}

#[test]
fn test_save_and_load_oauth_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let tokens = make_test_tokens();

    save_oauth_tokens(dir.path(), &tokens).unwrap();

    let loaded = load_stored_oauth_tokens(Some(dir.path())).unwrap();
    assert_eq!(loaded.access_token, "test-token");
    assert_eq!(loaded.refresh_token.as_deref(), Some("refresh-token"));
    assert_eq!(loaded.subscription_type.as_deref(), Some("pro"));
    assert_eq!(loaded.org_uuid.as_deref(), Some("org-123"));
}

#[test]
fn test_clear_stored_oauth_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let tokens = make_test_tokens();

    save_oauth_tokens(dir.path(), &tokens).unwrap();
    assert!(load_stored_oauth_tokens(Some(dir.path())).is_some());

    clear_stored_oauth_tokens(dir.path()).unwrap();
    assert!(load_stored_oauth_tokens(Some(dir.path())).is_none());
}

#[test]
fn test_resolve_auth_force_env_auth() {
    let prev = std::env::var("ANTHROPIC_API_KEY").ok();
    // SAFETY: Test-only.
    unsafe { std::env::set_var("ANTHROPIC_API_KEY", "bare-key") };

    let options = AuthResolveOptions {
        force_env_auth: true,
        ..Default::default()
    };
    let auth = resolve_auth(&options);
    assert!(matches!(auth, Some(AuthMethod::ApiKey { ref key }) if key == "bare-key"));

    match prev {
        Some(v) => unsafe { std::env::set_var("ANTHROPIC_API_KEY", v) },
        None => unsafe { std::env::remove_var("ANTHROPIC_API_KEY") },
    }
}

#[test]
fn test_resolve_auth_stored_oauth_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let tokens = make_test_tokens();
    save_oauth_tokens(dir.path(), &tokens).unwrap();

    // Clear API key to force OAuth fallback
    let prev = std::env::var("ANTHROPIC_API_KEY").ok();
    // SAFETY: Test-only.
    unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };

    let options = AuthResolveOptions {
        config_dir: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let auth = resolve_auth(&options);
    assert!(matches!(auth, Some(AuthMethod::OAuth(ref t)) if t.access_token == "test-token"));

    if let Some(v) = prev {
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", v) }
    }
}

#[test]
fn test_resolve_auth_force_oauth() {
    let dir = tempfile::tempdir().unwrap();
    let tokens = make_test_tokens();
    save_oauth_tokens(dir.path(), &tokens).unwrap();

    let options = AuthResolveOptions {
        config_dir: Some(dir.path().to_path_buf()),
        force_login_method: Some(LoginMethod::OAuth),
        ..Default::default()
    };
    let auth = resolve_auth(&options);
    assert!(matches!(auth, Some(AuthMethod::OAuth(_))));
}
