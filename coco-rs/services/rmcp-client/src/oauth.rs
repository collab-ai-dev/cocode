//! This file handles all logic related to managing MCP OAuth credentials.
//! All credentials are stored using the keyring crate which uses os-specific keyring services.
//! https://crates.io/crates/keyring
//! macOS: macOS keychain.
//! Windows: Windows Credential Manager
//! Linux: DBus-based Secret Service, the kernel keyutils, and a combo of the two
//! FreeBSD, OpenBSD: DBus-based Secret Service
//!
//! For Linux, we use linux-native-async-persistent which uses both keyutils and async-secret-service (see below) for storage.
//! See the docs for the keyutils_persistent module for a full explanation of why both are used. Because this store uses the
//! async-secret-service, you must specify the additional features required by that store
//!
//! async-secret-service provides access to the DBus-based Secret Service storage on Linux, FreeBSD, and OpenBSD. This is an asynchronous
//! keystore that always encrypts secrets when they are transferred across the bus. If DBus isn't installed the keystore will fall back to the json
//! file because we don't use the "vendored" feature.
//!
//! If the keyring is not available or fails, we fall back to CODEX_HOME/.credentials.json which is consistent with other coding CLI agents.

use crate::Result;
use crate::ResultExt;
use crate::RmcpClientError;
use oauth2::AccessToken;
use oauth2::RefreshToken;
use oauth2::Scope;
use oauth2::TokenResponse;
use oauth2::basic::BasicTokenType;
use rmcp::transport::auth::OAuthTokenResponse;
use rmcp::transport::auth::VendorExtraTokenFields;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::map::Map as JsonMap;
use sha2::Digest;
use sha2::Sha256;
use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;
use tracing::warn;

use coco_keyring_store::DefaultKeyringStore;
use coco_keyring_store::KeyringStore;
use rmcp::transport::auth::AuthorizationManager;
use tokio::sync::Mutex;

const KEYRING_SERVICE: &str = "Codex MCP Credentials";
const REFRESH_SKEW_MILLIS: u64 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredOAuthTokens {
    pub server_name: String,
    pub url: String,
    pub client_id: String,
    pub token_response: WrappedOAuthTokenResponse,
    #[serde(default)]
    pub expires_at: Option<u64>,
}

/// Determine where Codex should store and read MCP credentials.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OAuthCredentialsStoreMode {
    /// `Keyring` when available; otherwise, `File`.
    /// Credentials stored in the keyring will only be readable by Codex unless the user explicitly grants access via OS-level keyring access.
    #[default]
    Auto,
    /// CODEX_HOME/.credentials.json
    /// This file will be readable to Codex and other applications running as the same user.
    File,
    /// Keyring when available, otherwise fail.
    Keyring,
}

/// Wrap OAuthTokenResponse to allow for partial equality comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrappedOAuthTokenResponse(pub OAuthTokenResponse);

impl PartialEq for WrappedOAuthTokenResponse {
    fn eq(&self, other: &Self) -> bool {
        match (serde_json::to_string(self), serde_json::to_string(other)) {
            (Ok(s1), Ok(s2)) => s1 == s2,
            _ => false,
        }
    }
}

pub(crate) fn load_oauth_tokens(
    server_name: &str,
    url: &str,
    store_mode: OAuthCredentialsStoreMode,
    config_home: &std::path::Path,
) -> Result<Option<StoredOAuthTokens>> {
    let keyring_store = DefaultKeyringStore;
    match store_mode {
        OAuthCredentialsStoreMode::Auto => load_oauth_tokens_from_keyring_with_fallback_to_file(
            &keyring_store,
            server_name,
            url,
            config_home,
        ),
        OAuthCredentialsStoreMode::File => {
            load_oauth_tokens_from_file(server_name, url, config_home)
        }
        OAuthCredentialsStoreMode::Keyring => {
            load_oauth_tokens_from_keyring(&keyring_store, server_name, url)
                .with_ctx("failed to read OAuth tokens from keyring")
        }
    }
}

pub(crate) fn has_oauth_tokens(
    server_name: &str,
    url: &str,
    store_mode: OAuthCredentialsStoreMode,
    config_home: &std::path::Path,
) -> Result<bool> {
    Ok(load_oauth_tokens(server_name, url, store_mode, config_home)?.is_some())
}

pub fn has_valid_oauth_tokens(
    server_name: &str,
    url: &str,
    store_mode: OAuthCredentialsStoreMode,
    config_home: &std::path::Path,
) -> Result<bool> {
    let Some(tokens) = load_oauth_tokens(server_name, url, store_mode, config_home)? else {
        return Ok(false);
    };
    Ok(!token_needs_refresh(tokens.expires_at))
}

fn refresh_expires_in_from_timestamp(tokens: &mut StoredOAuthTokens) {
    let Some(expires_at) = tokens.expires_at else {
        return;
    };

    match expires_in_from_timestamp(expires_at) {
        Some(seconds) => {
            let duration = Duration::from_secs(seconds);
            tokens.token_response.0.set_expires_in(Some(&duration));
        }
        None => {
            tokens.token_response.0.set_expires_in(None);
        }
    }
}

fn load_oauth_tokens_from_keyring_with_fallback_to_file<K: KeyringStore>(
    keyring_store: &K,
    server_name: &str,
    url: &str,
    config_home: &std::path::Path,
) -> Result<Option<StoredOAuthTokens>> {
    match load_oauth_tokens_from_keyring(keyring_store, server_name, url) {
        Ok(Some(tokens)) => Ok(Some(tokens)),
        Ok(None) => load_oauth_tokens_from_file(server_name, url, config_home),
        Err(error) => {
            warn!("failed to read OAuth tokens from keyring: {error}");
            load_oauth_tokens_from_file(server_name, url, config_home)
                .with_ctx_lazy(|| format!("failed to read OAuth tokens from keyring: {error}"))
        }
    }
}

fn load_oauth_tokens_from_keyring<K: KeyringStore>(
    keyring_store: &K,
    server_name: &str,
    url: &str,
) -> Result<Option<StoredOAuthTokens>> {
    let key = compute_store_key(server_name, url)?;
    match keyring_store.load(KEYRING_SERVICE, &key) {
        Ok(Some(serialized)) => {
            let mut tokens: StoredOAuthTokens = serde_json::from_str(&serialized)
                .with_ctx("failed to deserialize OAuth tokens from keyring")?;
            refresh_expires_in_from_timestamp(&mut tokens);
            Ok(Some(tokens))
        }
        Ok(None) => Ok(None),
        Err(error) => Err(error.into_error().into()),
    }
}

pub fn save_oauth_tokens(
    server_name: &str,
    tokens: &StoredOAuthTokens,
    store_mode: OAuthCredentialsStoreMode,
    config_home: &std::path::Path,
) -> Result<()> {
    let keyring_store = DefaultKeyringStore;
    match store_mode {
        OAuthCredentialsStoreMode::Auto => save_oauth_tokens_with_keyring_with_fallback_to_file(
            &keyring_store,
            server_name,
            tokens,
            config_home,
        ),
        OAuthCredentialsStoreMode::File => save_oauth_tokens_to_file(tokens, config_home),
        OAuthCredentialsStoreMode::Keyring => {
            save_oauth_tokens_with_keyring(&keyring_store, server_name, tokens, config_home)
        }
    }
}

pub struct OAuthAccessTokenSave<'a> {
    pub server_name: &'a str,
    pub url: &'a str,
    pub client_id: &'a str,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: Option<i64>,
    pub scopes: Option<String>,
    pub store_mode: OAuthCredentialsStoreMode,
    pub config_home: &'a std::path::Path,
}

pub fn save_oauth_access_token(params: OAuthAccessTokenSave<'_>) -> Result<()> {
    let mut token_response = OAuthTokenResponse::new(
        AccessToken::new(params.access_token),
        BasicTokenType::Bearer,
        VendorExtraTokenFields::default(),
    );
    if let Some(refresh_token) = params.refresh_token {
        token_response.set_refresh_token(Some(RefreshToken::new(refresh_token)));
    }
    if let Some(scopes) = params.scopes {
        let scopes = scopes
            .split_whitespace()
            .filter(|scope| !scope.is_empty())
            .map(|scope| Scope::new(scope.to_string()))
            .collect::<Vec<_>>();
        if !scopes.is_empty() {
            token_response.set_scopes(Some(scopes));
        }
    }
    if let Some(expires_in) = params
        .expires_in
        .and_then(|seconds| u64::try_from(seconds).ok())
        && expires_in > 0
    {
        let duration = Duration::from_secs(expires_in);
        token_response.set_expires_in(Some(&duration));
    }

    let expires_at = compute_expires_at_millis(&token_response);
    let stored = StoredOAuthTokens {
        server_name: params.server_name.to_string(),
        url: params.url.to_string(),
        client_id: params.client_id.to_string(),
        token_response: WrappedOAuthTokenResponse(token_response),
        expires_at,
    };
    save_oauth_tokens(
        params.server_name,
        &stored,
        params.store_mode,
        params.config_home,
    )
}

fn save_oauth_tokens_with_keyring<K: KeyringStore>(
    keyring_store: &K,
    server_name: &str,
    tokens: &StoredOAuthTokens,
    config_home: &std::path::Path,
) -> Result<()> {
    let serialized = serde_json::to_string(tokens).with_ctx("failed to serialize OAuth tokens")?;

    let key = compute_store_key(server_name, &tokens.url)?;
    match keyring_store.save(KEYRING_SERVICE, &key, &serialized) {
        Ok(()) => {
            if let Err(error) = delete_oauth_tokens_from_file(&key, config_home) {
                warn!("failed to remove OAuth tokens from fallback storage: {error:?}");
            }
            Ok(())
        }
        Err(error) => {
            let message = format!(
                "failed to write OAuth tokens to keyring: {}",
                error.message()
            );
            warn!("{message}");
            Err(RmcpClientError::Generic { message })
        }
    }
}

fn save_oauth_tokens_with_keyring_with_fallback_to_file<K: KeyringStore>(
    keyring_store: &K,
    server_name: &str,
    tokens: &StoredOAuthTokens,
    config_home: &std::path::Path,
) -> Result<()> {
    match save_oauth_tokens_with_keyring(keyring_store, server_name, tokens, config_home) {
        Ok(()) => Ok(()),
        Err(error) => {
            let message = error.to_string();
            warn!("falling back to file storage for OAuth tokens: {message}");
            save_oauth_tokens_to_file(tokens, config_home)
                .with_ctx_lazy(|| format!("failed to write OAuth tokens to keyring: {message}"))
        }
    }
}

pub fn delete_oauth_tokens(
    server_name: &str,
    url: &str,
    store_mode: OAuthCredentialsStoreMode,
    config_home: &std::path::Path,
) -> Result<bool> {
    let keyring_store = DefaultKeyringStore;
    delete_oauth_tokens_from_keyring_and_file(
        &keyring_store,
        store_mode,
        server_name,
        url,
        config_home,
    )
}

fn delete_oauth_tokens_from_keyring_and_file<K: KeyringStore>(
    keyring_store: &K,
    store_mode: OAuthCredentialsStoreMode,
    server_name: &str,
    url: &str,
    config_home: &std::path::Path,
) -> Result<bool> {
    let key = compute_store_key(server_name, url)?;
    let keyring_result = keyring_store.delete(KEYRING_SERVICE, &key);
    let keyring_removed = match keyring_result {
        Ok(removed) => removed,
        Err(error) => {
            let message = error.message();
            warn!("failed to delete OAuth tokens from keyring: {message}");
            match store_mode {
                OAuthCredentialsStoreMode::Auto | OAuthCredentialsStoreMode::Keyring => {
                    return Err(error.into_error())
                        .with_ctx("failed to delete OAuth tokens from keyring");
                }
                OAuthCredentialsStoreMode::File => false,
            }
        }
    };

    let file_removed = delete_oauth_tokens_from_file(&key, config_home)?;
    Ok(keyring_removed || file_removed)
}

#[derive(Clone)]
pub(crate) struct OAuthPersistor {
    inner: Arc<OAuthPersistorInner>,
}

struct OAuthPersistorInner {
    server_name: String,
    url: String,
    authorization_manager: Arc<Mutex<AuthorizationManager>>,
    store_mode: OAuthCredentialsStoreMode,
    config_home: PathBuf,
    last_credentials: Mutex<Option<StoredOAuthTokens>>,
}

impl OAuthPersistor {
    pub(crate) fn new(
        server_name: String,
        url: String,
        authorization_manager: Arc<Mutex<AuthorizationManager>>,
        store_mode: OAuthCredentialsStoreMode,
        config_home: PathBuf,
        initial_credentials: Option<StoredOAuthTokens>,
    ) -> Self {
        Self {
            inner: Arc::new(OAuthPersistorInner {
                server_name,
                url,
                authorization_manager,
                store_mode,
                config_home,
                last_credentials: Mutex::new(initial_credentials),
            }),
        }
    }

    /// Persists the latest stored credentials if they have changed.
    /// Deletes the credentials if they are no longer present.
    pub(crate) async fn persist_if_needed(&self) -> Result<()> {
        let (client_id, maybe_credentials) = {
            let manager = self.inner.authorization_manager.clone();
            let guard = manager.lock().await;
            guard.get_credentials().await
        }?;

        match maybe_credentials {
            Some(credentials) => {
                let mut last_credentials = self.inner.last_credentials.lock().await;
                let new_token_response = WrappedOAuthTokenResponse(credentials.clone());
                let same_token = last_credentials
                    .as_ref()
                    .map(|prev| prev.token_response == new_token_response)
                    .unwrap_or(false);
                let expires_at = if same_token {
                    last_credentials.as_ref().and_then(|prev| prev.expires_at)
                } else {
                    compute_expires_at_millis(&credentials)
                };
                let stored = StoredOAuthTokens {
                    server_name: self.inner.server_name.clone(),
                    url: self.inner.url.clone(),
                    client_id,
                    token_response: new_token_response,
                    expires_at,
                };
                if last_credentials.as_ref() != Some(&stored) {
                    save_oauth_tokens(
                        &self.inner.server_name,
                        &stored,
                        self.inner.store_mode,
                        &self.inner.config_home,
                    )?;
                    *last_credentials = Some(stored);
                }
            }
            None => {
                let mut last_serialized = self.inner.last_credentials.lock().await;
                if last_serialized.take().is_some()
                    && let Err(error) = delete_oauth_tokens(
                        &self.inner.server_name,
                        &self.inner.url,
                        self.inner.store_mode,
                        &self.inner.config_home,
                    )
                {
                    warn!(
                        "failed to remove OAuth tokens for server {}: {error}",
                        self.inner.server_name
                    );
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn refresh_if_needed(&self) -> Result<()> {
        let expires_at = {
            let guard = self.inner.last_credentials.lock().await;
            guard.as_ref().and_then(|tokens| tokens.expires_at)
        };

        if !token_needs_refresh(expires_at) {
            return Ok(());
        }

        {
            let manager = self.inner.authorization_manager.clone();
            let guard = manager.lock().await;
            guard.refresh_token().await.with_ctx_lazy(|| {
                format!(
                    "failed to refresh OAuth tokens for server {}",
                    self.inner.server_name
                )
            })?;
        }

        self.persist_if_needed().await
    }
}

const FALLBACK_FILENAME: &str = ".credentials.json";
const MCP_SERVER_TYPE: &str = "http";

type FallbackFile = BTreeMap<String, FallbackTokenEntry>;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FallbackTokenEntry {
    server_name: String,
    server_url: String,
    client_id: String,
    access_token: String,
    #[serde(default)]
    expires_at: Option<u64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scopes: Vec<String>,
}

fn load_oauth_tokens_from_file(
    server_name: &str,
    url: &str,
    config_home: &std::path::Path,
) -> Result<Option<StoredOAuthTokens>> {
    let Some(store) = read_fallback_file(config_home)? else {
        return Ok(None);
    };

    let key = compute_store_key(server_name, url)?;

    for entry in store.values() {
        let entry_key = compute_store_key(&entry.server_name, &entry.server_url)?;
        if entry_key != key {
            continue;
        }

        let mut token_response = OAuthTokenResponse::new(
            AccessToken::new(entry.access_token.clone()),
            BasicTokenType::Bearer,
            VendorExtraTokenFields::default(),
        );

        if let Some(refresh) = entry.refresh_token.clone() {
            token_response.set_refresh_token(Some(RefreshToken::new(refresh)));
        }

        let scopes = entry.scopes.clone();
        if !scopes.is_empty() {
            token_response.set_scopes(Some(scopes.into_iter().map(Scope::new).collect()));
        }

        let mut stored = StoredOAuthTokens {
            server_name: entry.server_name.clone(),
            url: entry.server_url.clone(),
            client_id: entry.client_id.clone(),
            token_response: WrappedOAuthTokenResponse(token_response),
            expires_at: entry.expires_at,
        };
        refresh_expires_in_from_timestamp(&mut stored);

        return Ok(Some(stored));
    }

    Ok(None)
}

fn save_oauth_tokens_to_file(
    tokens: &StoredOAuthTokens,
    config_home: &std::path::Path,
) -> Result<()> {
    let key = compute_store_key(&tokens.server_name, &tokens.url)?;
    let mut store = read_fallback_file(config_home)?.unwrap_or_default();

    let token_response = &tokens.token_response.0;
    let expires_at = tokens
        .expires_at
        .or_else(|| compute_expires_at_millis(token_response));
    let refresh_token = token_response
        .refresh_token()
        .map(|token| token.secret().to_string());
    let scopes = token_response
        .scopes()
        .map(|s| s.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    let entry = FallbackTokenEntry {
        server_name: tokens.server_name.clone(),
        server_url: tokens.url.clone(),
        client_id: tokens.client_id.clone(),
        access_token: token_response.access_token().secret().to_string(),
        expires_at,
        refresh_token,
        scopes,
    };

    store.insert(key, entry);
    write_fallback_file(&store, config_home)
}

fn delete_oauth_tokens_from_file(key: &str, config_home: &std::path::Path) -> Result<bool> {
    let mut store = match read_fallback_file(config_home)? {
        Some(store) => store,
        None => return Ok(false),
    };

    let removed = store.remove(key).is_some();

    if removed {
        write_fallback_file(&store, config_home)?;
    }

    Ok(removed)
}

pub(crate) fn compute_expires_at_millis(response: &OAuthTokenResponse) -> Option<u64> {
    let expires_in = response.expires_in()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    let expiry = now.checked_add(expires_in)?;
    let millis = expiry.as_millis();
    if millis > u128::from(u64::MAX) {
        Some(u64::MAX)
    } else {
        Some(millis as u64)
    }
}

fn expires_in_from_timestamp(expires_at: u64) -> Option<u64> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    let now_ms = now.as_millis() as u64;

    if expires_at <= now_ms {
        None
    } else {
        Some((expires_at - now_ms) / 1000)
    }
}

fn token_needs_refresh(expires_at: Option<u64>) -> bool {
    let Some(expires_at) = expires_at else {
        return false;
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64;

    now.saturating_add(REFRESH_SKEW_MILLIS) >= expires_at
}

fn compute_store_key(server_name: &str, server_url: &str) -> Result<String> {
    let mut payload = JsonMap::new();
    payload.insert(
        "type".to_string(),
        Value::String(MCP_SERVER_TYPE.to_string()),
    );
    payload.insert("url".to_string(), Value::String(server_url.to_string()));
    payload.insert("headers".to_string(), Value::Object(JsonMap::new()));

    let truncated = sha_256_prefix(&Value::Object(payload))?;
    Ok(format!("{server_name}|{truncated}"))
}

fn fallback_file_path(config_home: &std::path::Path) -> Result<PathBuf> {
    Ok(config_home.join(FALLBACK_FILENAME))
}

fn read_fallback_file(config_home: &std::path::Path) -> Result<Option<FallbackFile>> {
    let path = fallback_file_path(config_home)?;
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_ctx_lazy(|| {
                format!("failed to read credentials file at {}", path.display())
            });
        }
    };

    match serde_json::from_str::<FallbackFile>(&contents) {
        Ok(store) => Ok(Some(store)),
        Err(e) => Err(e)
            .with_ctx_lazy(|| format!("failed to parse credentials file at {}", path.display())),
    }
}

fn write_fallback_file(store: &FallbackFile, config_home: &std::path::Path) -> Result<()> {
    let path = fallback_file_path(config_home)?;

    if store.is_empty() {
        if path.exists() {
            fs::remove_file(path)?;
        }
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let serialized = serde_json::to_string(store)?;
    fs::write(&path, serialized)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }

    Ok(())
}

fn sha_256_prefix(value: &Value) -> Result<String> {
    let serialized =
        serde_json::to_string(&value).with_ctx("failed to serialize MCP OAuth key payload")?;
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    let truncated = &hex[..16];
    Ok(truncated.to_string())
}

#[cfg(test)]
#[path = "oauth.test.rs"]
mod tests;
