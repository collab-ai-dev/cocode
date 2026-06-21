//! Provider-scoped credential persistence. `CredentialBackend` mirrors codex's
//! `AuthStorageBackend` (File / Keyring / Auto / Ephemeral). Tokens are redacted
//! in `Debug`; the file backend writes `0600` atomically (temp + rename).

use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use coco_keyring_store::DefaultKeyringStore;
use coco_keyring_store::KeyringStore;
use coco_types::OAuthFlowId;
use serde::Deserialize;
use serde::Serialize;

use crate::error::Result;
use crate::error::StoreSnafu;
use crate::token_cell::TokenSnapshot;

const KEYRING_SERVICE: &str = "Coco Provider Auth";

/// Persisted, provider-scoped credential. `login_epoch` bumps on a fresh login
/// (not on refresh) so credential *identity* changes are distinguishable from
/// token rotation.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredCredential {
    pub flow: OAuthFlowId,
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default)]
    pub login_epoch: u64,
}

impl fmt::Debug for StoredCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredCredential")
            .field("flow", &self.flow)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "<redacted>"))
            .field("account_id", &self.account_id)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("plan_type", &self.plan_type)
            .field("email", &self.email)
            .field("login_epoch", &self.login_epoch)
            .finish()
    }
}

impl StoredCredential {
    pub fn to_snapshot(&self) -> TokenSnapshot {
        TokenSnapshot {
            access_token: self.access_token.clone(),
            account_id: self.account_id.clone(),
            refresh_token: self.refresh_token.clone(),
            subscription_type: self.plan_type.clone(),
            expires_at_ms: self.expires_at_ms,
            login_epoch: self.login_epoch,
        }
    }
}

/// Storage backend over a provider-instance name (e.g. `"openai-chatgpt"`).
pub trait CredentialBackend: Send + Sync {
    fn load(&self, name: &str) -> Result<Option<StoredCredential>>;
    fn save(&self, name: &str, cred: &StoredCredential) -> Result<()>;
    fn delete(&self, name: &str) -> Result<bool>;
}

fn parse(json: &str) -> Result<StoredCredential> {
    serde_json::from_str(json).map_err(|e| {
        crate::error::InternalSnafu {
            message: format!("parse stored credential: {e}"),
        }
        .build()
    })
}

fn encode(cred: &StoredCredential) -> Result<String> {
    serde_json::to_string_pretty(cred).map_err(|e| {
        crate::error::InternalSnafu {
            message: format!("encode stored credential: {e}"),
        }
        .build()
    })
}

/// OS keyring backend (macOS Keychain / Linux Secret Service / Windows CM).
pub struct KeyringBackend {
    store: DefaultKeyringStore,
}

impl Default for KeyringBackend {
    fn default() -> Self {
        Self {
            store: DefaultKeyringStore,
        }
    }
}

impl CredentialBackend for KeyringBackend {
    fn load(&self, name: &str) -> Result<Option<StoredCredential>> {
        let raw = self.store.load(KEYRING_SERVICE, name).map_err(|e| {
            StoreSnafu {
                message: e.message(),
            }
            .build()
        })?;
        raw.map(|s| parse(&s)).transpose()
    }

    fn save(&self, name: &str, cred: &StoredCredential) -> Result<()> {
        let json = encode(cred)?;
        self.store.save(KEYRING_SERVICE, name, &json).map_err(|e| {
            StoreSnafu {
                message: e.message(),
            }
            .build()
        })
    }

    fn delete(&self, name: &str) -> Result<bool> {
        self.store.delete(KEYRING_SERVICE, name).map_err(|e| {
            StoreSnafu {
                message: e.message(),
            }
            .build()
        })
    }
}

/// File backend: `<auth_dir>/<name>.json`, mode 0600, atomic temp + rename.
pub struct FileBackend {
    auth_dir: PathBuf,
}

impl FileBackend {
    pub fn new(auth_dir: PathBuf) -> Self {
        Self { auth_dir }
    }

    /// Map an instance name to its credential file, rejecting anything that
    /// isn't a flat slug. The name flows from user config keys and the `coco
    /// login <name>` arg; an unvalidated `..`/`/` would let `join` escape
    /// `auth_dir` and scatter a `0600` secret file to an arbitrary path.
    fn path(&self, name: &str) -> Result<PathBuf> {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return Err(StoreSnafu {
                message: format!(
                    "invalid provider instance name '{name}': only [A-Za-z0-9_-] are allowed"
                ),
            }
            .build());
        }
        Ok(self.auth_dir.join(format!("{name}.json")))
    }

    /// Create the auth directory `0700` (secrets dir) rather than the umask
    /// default (`0755`, world-listable).
    fn ensure_dir(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&self.auth_dir)
                .map_err(io_to_store)
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir_all(&self.auth_dir).map_err(io_to_store)
        }
    }
}

impl CredentialBackend for FileBackend {
    fn load(&self, name: &str) -> Result<Option<StoredCredential>> {
        match std::fs::read_to_string(self.path(name)?) {
            Ok(s) => parse(&s).map(Some),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StoreSnafu {
                message: e.to_string(),
            }
            .build()),
        }
    }

    fn save(&self, name: &str, cred: &StoredCredential) -> Result<()> {
        let json = encode(cred)?;
        let path = self.path(name)?;
        self.ensure_dir()?;
        let tmp = path.with_extension("json.tmp");
        write_0600(&tmp, json.as_bytes())?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            StoreSnafu {
                message: e.to_string(),
            }
            .build()
        })
    }

    fn delete(&self, name: &str) -> Result<bool> {
        match std::fs::remove_file(self.path(name)?) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(StoreSnafu {
                message: e.to_string(),
            }
            .build()),
        }
    }
}

/// Auto backend: keyring first, file fallback (the production default).
pub struct AutoBackend {
    keyring: KeyringBackend,
    file: FileBackend,
}

impl AutoBackend {
    pub fn new(auth_dir: PathBuf) -> Self {
        Self {
            keyring: KeyringBackend::default(),
            file: FileBackend::new(auth_dir),
        }
    }
}

impl CredentialBackend for AutoBackend {
    fn load(&self, name: &str) -> Result<Option<StoredCredential>> {
        match self.keyring.load(name) {
            Ok(Some(c)) => Ok(Some(c)),
            // No keyring entry, or the keyring is unavailable → consult the
            // file. (`save` guarantees only one backend holds a copy, so this
            // can't surface a stale shadow of a fresher keyring copy.)
            Ok(None) | Err(_) => self.file.load(name),
        }
    }

    fn save(&self, name: &str, cred: &StoredCredential) -> Result<()> {
        // Exactly ONE backend must hold the credential, or `load`'s
        // keyring-first priority could later serve a stale shadow of a copy the
        // other backend refreshed. On each save we write the chosen backend and
        // best-effort delete the other.
        if self.keyring.save(name, cred).is_ok() {
            let _ = self.file.delete(name);
            return Ok(());
        }
        // Keyring unavailable (e.g. headless Linux) → file is authoritative.
        self.file.save(name, cred)?;
        let _ = self.keyring.delete(name);
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<bool> {
        let k = self.keyring.delete(name).unwrap_or(false);
        let f = self.file.delete(name).unwrap_or(false);
        Ok(k || f)
    }
}

/// In-memory backend for tests / `--no-persist` runs.
#[derive(Default)]
pub struct EphemeralBackend {
    map: Mutex<HashMap<String, StoredCredential>>,
}

impl CredentialBackend for EphemeralBackend {
    fn load(&self, name: &str) -> Result<Option<StoredCredential>> {
        Ok(self
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(name)
            .cloned())
    }

    fn save(&self, name: &str, cred: &StoredCredential) -> Result<()> {
        self.map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_string(), cred.clone());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<bool> {
        Ok(self
            .map
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(name)
            .is_some())
    }
}

fn io_to_store(e: std::io::Error) -> crate::error::ProviderAuthError {
    StoreSnafu {
        message: e.to_string(),
    }
    .build()
}

fn write_0600(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).map_err(io_to_store)?;
    f.write_all(bytes).map_err(io_to_store)?;
    f.sync_all().map_err(io_to_store)?;
    Ok(())
}

#[cfg(test)]
#[path = "store.test.rs"]
mod tests;
