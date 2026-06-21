use pretty_assertions::assert_eq;

use super::*;

// ── OAuthTokens ──

#[test]
fn test_token_not_expired_when_no_expiry() {
    let tokens = OAuthTokens {
        access_token: "abc".to_string(),
        refresh_token: None,
        expires_at: None,
        token_type: "Bearer".to_string(),
    };
    assert!(!tokens.is_expired(1_000_000));
    assert!(!tokens.needs_refresh(1_000_000));
}

#[test]
fn test_token_expired() {
    let tokens = OAuthTokens {
        access_token: "abc".to_string(),
        refresh_token: None,
        expires_at: Some(1_000_000),
        token_type: "Bearer".to_string(),
    };
    assert!(tokens.is_expired(1_000_000));
    assert!(tokens.is_expired(2_000_000));
    assert!(!tokens.is_expired(999_999));
}

#[test]
fn test_token_needs_refresh_within_five_minutes() {
    let tokens = OAuthTokens {
        access_token: "abc".to_string(),
        refresh_token: Some("refresh".to_string()),
        expires_at: Some(1_000_000),
        token_type: "Bearer".to_string(),
    };
    // 5 minutes = 300_000 ms before expiry
    assert!(tokens.needs_refresh(700_001));
    assert!(!tokens.needs_refresh(699_999));
}

// ── OAuthTokenStore ──

#[test]
fn test_store_save_and_load() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let store = OAuthTokenStore::new(dir.path().join("tokens.json"));

    let tokens = OAuthTokens {
        access_token: "access_123".to_string(),
        refresh_token: Some("refresh_456".to_string()),
        expires_at: Some(9_999_999),
        token_type: "Bearer".to_string(),
    };

    store.save("my-server|abc", &tokens).expect("save");

    let loaded = store
        .load("my-server|abc")
        .expect("load")
        .expect("should exist");

    assert_eq!(loaded.access_token, "access_123");
    assert_eq!(loaded.refresh_token.as_deref(), Some("refresh_456"));
    assert_eq!(loaded.expires_at, Some(9_999_999));
}

#[test]
fn test_store_load_nonexistent() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let store = OAuthTokenStore::new(dir.path().join("tokens.json"));

    let loaded = store.load("nonexistent").expect("load");
    assert!(loaded.is_none());
}

#[test]
fn test_store_remove() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let store = OAuthTokenStore::new(dir.path().join("tokens.json"));

    let tokens = OAuthTokens {
        access_token: "tok".to_string(),
        refresh_token: None,
        expires_at: None,
        token_type: "Bearer".to_string(),
    };

    store.save("server-a", &tokens).expect("save");
    assert!(store.has_tokens("server-a"));

    store.remove("server-a").expect("remove");
    assert!(!store.has_tokens("server-a"));
}

#[test]
fn test_store_multiple_servers() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let store = OAuthTokenStore::new(dir.path().join("tokens.json"));

    let t1 = OAuthTokens {
        access_token: "tok1".to_string(),
        refresh_token: None,
        expires_at: None,
        token_type: "Bearer".to_string(),
    };
    let t2 = OAuthTokens {
        access_token: "tok2".to_string(),
        refresh_token: Some("ref2".to_string()),
        expires_at: Some(42),
        token_type: "Bearer".to_string(),
    };

    store.save("server-a", &t1).expect("save a");
    store.save("server-b", &t2).expect("save b");

    let a = store.load("server-a").expect("load a").expect("exists");
    let b = store.load("server-b").expect("load b").expect("exists");

    assert_eq!(a.access_token, "tok1");
    assert_eq!(b.access_token, "tok2");
    assert_eq!(b.refresh_token.as_deref(), Some("ref2"));
}

// ── server_key ──

#[test]
fn test_server_key_contains_name() {
    let key = server_key("my-server", "https://example.com/mcp");
    assert!(key.starts_with("my-server|"));
    assert!(key.len() > "my-server|".len());
}

#[test]
fn test_server_key_differs_for_different_urls() {
    let k1 = server_key("srv", "https://a.example.com");
    let k2 = server_key("srv", "https://b.example.com");
    assert_ne!(k1, k2);
}

// ── has_discovery_but_no_token ──

#[test]
fn test_has_discovery_but_no_token_false_when_missing() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let store = OAuthTokenStore::new(dir.path().join("tokens.json"));
    assert!(!has_discovery_but_no_token(&store, "missing"));
}

#[test]
fn test_has_discovery_but_no_token_false_when_has_token() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let store = OAuthTokenStore::new(dir.path().join("tokens.json"));

    let tokens = OAuthTokens {
        access_token: "valid".to_string(),
        refresh_token: None,
        expires_at: None,
        token_type: "Bearer".to_string(),
    };
    store.save("srv", &tokens).expect("save");
    assert!(!has_discovery_but_no_token(&store, "srv"));
}
