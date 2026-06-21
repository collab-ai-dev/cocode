//! OAuth flows, token storage/refresh for MCP servers.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use coco_error::BoxedError;
use coco_error::StatusCode;
use coco_error::boxed;
use serde::Deserialize;
use serde::Serialize;
use tracing::info;
use tracing::warn;

/// OAuth configuration for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Server metadata URL override (for non-standard servers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_server_metadata_url: Option<String>,
}

/// OAuth token set.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub token_type: String,
}

impl OAuthTokens {
    /// Whether the access token has expired.
    pub fn is_expired(&self, now_ms: i64) -> bool {
        self.expires_at.is_some_and(|exp| now_ms >= exp)
    }

    /// Whether the token needs refresh (expired or within 5 min of expiry).
    pub fn needs_refresh(&self, now_ms: i64) -> bool {
        self.expires_at.is_some_and(|exp| now_ms >= exp - 300_000)
    }
}

// ── Token persistence ──

/// On-disk structure for all MCP OAuth credentials, keyed by server key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct OAuthStorageFile {
    #[serde(default)]
    mcp_oauth: HashMap<String, StoredTokenEntry>,
}

/// A stored token entry for a single server.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTokenEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(default)]
    token_type: String,
}

impl From<&OAuthTokens> for StoredTokenEntry {
    fn from(tokens: &OAuthTokens) -> Self {
        Self {
            access_token: Some(tokens.access_token.clone()),
            refresh_token: tokens.refresh_token.clone(),
            expires_at: tokens.expires_at,
            token_type: tokens.token_type.clone(),
        }
    }
}

impl StoredTokenEntry {
    fn to_tokens(&self) -> Option<OAuthTokens> {
        let access_token = self.access_token.as_ref()?;
        Some(OAuthTokens {
            access_token: access_token.clone(),
            refresh_token: self.refresh_token.clone(),
            expires_at: self.expires_at,
            token_type: self.token_type.clone(),
        })
    }
}

/// Persistent OAuth token store, backed by a JSON file on disk.
///
/// Tokens are keyed by a server key that combines the server name with a
/// hash of its config, preventing credential reuse across different server
/// configurations that happen to share the same name.
pub struct OAuthTokenStore {
    /// Path to the JSON storage file (e.g. `~/.coco/oauth_tokens.json`).
    storage_path: PathBuf,
}

impl OAuthTokenStore {
    /// Create a new store backed by the given file path.
    pub fn new(storage_path: PathBuf) -> Self {
        Self { storage_path }
    }

    /// Create a store at the default location under `config_home`.
    pub fn from_config_home(config_home: &Path) -> Self {
        Self {
            storage_path: config_home.join("oauth_tokens.json"),
        }
    }

    /// Load tokens for a specific server key.
    pub fn load(&self, server_key: &str) -> Result<Option<OAuthTokens>, BoxedError> {
        let file = self.read_file()?;
        Ok(file
            .mcp_oauth
            .get(server_key)
            .and_then(StoredTokenEntry::to_tokens))
    }

    /// Store tokens for a specific server key.
    pub fn save(&self, server_key: &str, tokens: &OAuthTokens) -> Result<(), BoxedError> {
        let mut file = self.read_file()?;
        file.mcp_oauth
            .insert(server_key.to_string(), StoredTokenEntry::from(tokens));
        self.write_file(&file)?;
        info!(server_key = %server_key, "saved OAuth tokens to disk");
        Ok(())
    }

    /// Remove tokens for a specific server key.
    pub fn remove(&self, server_key: &str) -> Result<(), BoxedError> {
        let mut file = self.read_file()?;
        file.mcp_oauth.remove(server_key);
        self.write_file(&file)?;
        info!(server_key = %server_key, "removed OAuth tokens from disk");
        Ok(())
    }

    /// Check if tokens exist for a server key (even if expired).
    pub fn has_tokens(&self, server_key: &str) -> bool {
        self.read_file()
            .ok()
            .and_then(|f| f.mcp_oauth.get(server_key).cloned())
            .and_then(|entry| entry.to_tokens())
            .is_some()
    }

    /// Read the storage file, returning an empty store if not found.
    fn read_file(&self) -> Result<OAuthStorageFile, BoxedError> {
        match std::fs::read_to_string(&self.storage_path) {
            Ok(content) => {
                let file: OAuthStorageFile = serde_json::from_str(&content)
                    .map_err(|e| boxed(e, StatusCode::InvalidJson))?;
                Ok(file)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OAuthStorageFile::default()),
            Err(e) => Err(boxed(e, StatusCode::IoError)),
        }
    }

    /// Write the storage file atomically (write to temp, rename).
    fn write_file(&self, file: &OAuthStorageFile) -> Result<(), BoxedError> {
        if let Some(parent) = self.storage_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| boxed(e, StatusCode::IoError))?;
        }

        let tmp_path = self.storage_path.with_extension("json.tmp");
        let content =
            serde_json::to_string_pretty(file).map_err(|e| boxed(e, StatusCode::Internal))?;
        std::fs::write(&tmp_path, content).map_err(|e| boxed(e, StatusCode::IoError))?;
        std::fs::rename(&tmp_path, &self.storage_path)
            .map_err(|e| boxed(e, StatusCode::IoError))?;
        Ok(())
    }
}

// ── Token refresh ──

/// Outcome of a token check/refresh attempt.
#[derive(Debug)]
pub enum TokenCheckResult {
    /// Token is still valid, no refresh needed.
    Valid(OAuthTokens),
    /// Token was refreshed and the new tokens are returned.
    Refreshed(OAuthTokens),
    /// No tokens found for this server.
    NoTokens,
    /// Refresh failed (token was invalid or server rejected).
    RefreshFailed { reason: String },
}

/// Check and optionally refresh OAuth tokens for a server.
///
/// If the current access token is still valid (not within 5 min of expiry),
/// returns it as-is. If it needs refresh and a refresh_token is available,
/// posts to the token endpoint to get new tokens. On `invalid_grant` the
/// stored tokens are evicted so subsequent attempts don't retry endlessly.
pub async fn check_and_refresh_token(
    store: &OAuthTokenStore,
    server_key: &str,
    config: &OAuthConfig,
    now_ms: i64,
) -> TokenCheckResult {
    // Load stored tokens
    let tokens = match store.load(server_key) {
        Ok(Some(t)) => t,
        Ok(None) => return TokenCheckResult::NoTokens,
        Err(e) => {
            warn!(server_key = %server_key, "failed to load tokens: {e}");
            return TokenCheckResult::NoTokens;
        }
    };

    // Still valid?
    if !tokens.needs_refresh(now_ms) {
        return TokenCheckResult::Valid(tokens);
    }

    // Need refresh — must have a refresh_token and a token_url
    let refresh_token = match &tokens.refresh_token {
        Some(rt) => rt.clone(),
        None => {
            return TokenCheckResult::RefreshFailed {
                reason: "no refresh token available".to_string(),
            };
        }
    };

    let token_url = match &config.token_url {
        Some(url) => url.clone(),
        None => {
            return TokenCheckResult::RefreshFailed {
                reason: "no token URL configured".to_string(),
            };
        }
    };

    // Perform the refresh request
    match do_token_refresh(&token_url, &refresh_token, config).await {
        Ok(new_tokens) => {
            if let Err(e) = store.save(server_key, &new_tokens) {
                warn!(server_key = %server_key, "failed to persist refreshed tokens: {e}");
            }
            TokenCheckResult::Refreshed(new_tokens)
        }
        Err(reason) => {
            // If invalid_grant, remove tokens so we don't retry endlessly
            if reason.contains("invalid_grant") {
                let _ = store.remove(server_key);
            }
            TokenCheckResult::RefreshFailed { reason }
        }
    }
}

/// Perform the OAuth2 token refresh POST.
async fn do_token_refresh(
    token_url: &str,
    refresh_token: &str,
    config: &OAuthConfig,
) -> Result<OAuthTokens, String> {
    let client = reqwest::Client::new();
    let mut params = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];

    let client_id_owned;
    if let Some(id) = &config.client_id {
        client_id_owned = id.clone();
        params.push(("client_id", &client_id_owned));
    }

    let response = client
        .post(token_url)
        .form(&params)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("refresh request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<no body>".to_string());

        // Detect invalid_grant from body
        if body.contains("invalid_grant")
            || body.contains("invalid_refresh_token")
            || body.contains("expired_refresh_token")
        {
            return Err("invalid_grant".to_string());
        }
        return Err(format!("token refresh HTTP {status}: {body}"));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("invalid JSON in refresh response: {e}"))?;

    // Even on 200, some servers put errors in the body (e.g. Slack)
    if let Some(error) = body.get("error").and_then(|v| v.as_str()) {
        if error == "invalid_grant"
            || error == "invalid_refresh_token"
            || error == "expired_refresh_token"
        {
            return Err("invalid_grant".to_string());
        }
        return Err(format!("OAuth error in 200 body: {error}"));
    }

    let access_token = body
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("no access_token in refresh response")?
        .to_string();

    let new_refresh = body
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| Some(refresh_token.to_string()));

    let expires_in = body.get("expires_in").and_then(serde_json::Value::as_i64);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let expires_at = expires_in.map(|secs| now_ms + secs * 1000);

    let token_type = body
        .get("token_type")
        .and_then(|v| v.as_str())
        .unwrap_or("Bearer")
        .to_string();

    Ok(OAuthTokens {
        access_token,
        refresh_token: new_refresh,
        expires_at,
        token_type,
    })
}

// ── Server key generation ──

/// Generate a unique server key from name + config hash.
///
/// Prevents credential reuse across different servers sharing a name.
pub fn server_key(server_name: &str, server_url: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;
    use std::hash::Hasher;

    let mut hasher = DefaultHasher::new();
    server_name.hash(&mut hasher);
    server_url.hash(&mut hasher);
    let hash = hasher.finish();

    format!("{server_name}|{hash:016x}")
}

/// Check if a server has discovery state but no usable tokens.
///
/// A server in this state would 401 on connect.
pub fn has_discovery_but_no_token(store: &OAuthTokenStore, server_key: &str) -> bool {
    match store.load(server_key) {
        Ok(Some(tokens)) => tokens.access_token.is_empty() && tokens.refresh_token.is_none(),
        Ok(None) => false,
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "auth.test.rs"]
mod tests;
