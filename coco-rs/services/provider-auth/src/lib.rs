//! `coco-provider-auth` — interactive OAuth login + subscription credential
//! management for LLM providers (OpenAI ChatGPT, Gemini Code Assist, and Grok).
//!
//! Generic, provider-agnostic machinery: PKCE + loopback login ([`flow`]),
//! provider-scoped storage ([`store`]), a process-stable lock-free credential
//! cell ([`token_cell`]), and a serialized refresh executor ([`refresh`]). The
//! per-provider wire contract lives in each `vercel-ai-<provider>` crate; this
//! crate only acquires/refreshes/stores credentials and hands `model_factory`
//! a live supplier via [`coco_inference::ProviderCredentialResolver`].
//!
//! **Keyed by provider-INSTANCE name**, not by flow. `login` activates
//! credentials for a *configured provider instance* (the `providers.<name>`
//! key); the OAuth flow is derived from that instance's `auth: OAuth { flow }`.
//! So multiple instances of the same flow — e.g. `openai-chatgpt` (Responses)
//! and a second `openai-chat-oauth` (Chat), or two accounts — are independent:
//! each has its own `TokenCell`, store file, and refresher. A model role bound
//! to any logged-in instance resolves its own credentials; instances on api-key
//! providers coexist untouched. This is the additive, per-instance model jcode
//! uses (and codex-rs's single-auth-mode does not).
//!
//! `AuthService` is the single source of truth (codex `AuthManager` analog): one
//! instance per process (see `app/cli::provider_login::shared_auth_service`)
//! means one `TokenCell` + one serialized refresher per provider instance, so a
//! rotating single-use refresh token is never double-spent.

pub mod descriptor;
mod device;
pub mod error;
pub mod flow;
pub mod import;
pub mod jwt;
pub mod pkce;
mod process_lock;
pub mod refresh;
pub mod store;
pub mod token_cell;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::Weak;

use coco_config::CredentialStoreMode;
use coco_inference::ProviderCredentialResolver;
use coco_inference::RefreshFuture;
use coco_inference::SubscriptionCredsSupplier;
use coco_types::AuthReadinessLevel;
use coco_types::AuthRefreshSupport;
use coco_types::AuthState;
use coco_types::OAuthFlowId;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tracing::warn;

pub use crate::descriptor::OAuthFlowDescriptor;
pub use crate::descriptor::descriptor_for;
pub use crate::error::ProviderAuthError;
pub use crate::error::Result;
pub use crate::flow::LoginOptions;
pub use crate::flow::LoginSurface;
use crate::refresh::now_ms;
pub use crate::store::AutoBackend;
pub use crate::store::CredentialBackend;
pub use crate::store::EphemeralBackend;
pub use crate::store::StoredCredential;
use crate::token_cell::TokenCell;
use crate::token_cell::TokenSnapshot;

/// Per-instance login status, surfaced to `coco auth status` / the TUI picker.
#[derive(Debug, Clone)]
pub struct ProviderAuthStatus {
    pub provider_name: String,
    pub flow: OAuthFlowId,
    /// Human-facing flow label (e.g. "ChatGPT subscription"), from the descriptor.
    pub display_name: &'static str,
    pub state: AuthState,
    pub readiness: AuthReadinessLevel,
    pub refresh_support: AuthRefreshSupport,
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub expires_at_ms: Option<i64>,
}

/// One managed provider INSTANCE: which flow it uses, its live cell, refresh
/// lock, and background-refresher handle.
struct ManagedProvider {
    descriptor: &'static OAuthFlowDescriptor,
    cell: TokenCell,
    refresh_lock: Arc<Semaphore>,
    refresher: Mutex<Option<JoinHandle<()>>>,
}

/// Credential acquisition + lifecycle service, keyed by provider-instance name.
/// Hold one `Arc<AuthService>` per process; implements
/// [`ProviderCredentialResolver`] for `model_factory`.
pub struct AuthService {
    backend: Arc<dyn CredentialBackend>,
    /// Directory containing per-provider cross-process refresh lock files.
    /// Ephemeral/custom test backends omit it and rely on the process lock.
    refresh_lock_dir: Option<PathBuf>,
    http: reqwest::Client,
    /// Lazily populated, keyed by provider-instance name (`providers.<name>`).
    providers: Mutex<HashMap<String, ManagedProvider>>,
    /// Weak self-ref so `&self` methods can spawn `Weak`-holding refreshers.
    me: OnceLock<Weak<AuthService>>,
}

/// Whether the running binary is a signed distribution build (release profile),
/// versus an unsigned dev / test build (debug profile). Decides whether the
/// default credential store ([`AuthService::with_config_dir`]) may read the OS
/// keychain. `debug_assertions` is the build-provenance proxy for "unsigned": it
/// is off only for `--release` (CI / official) artifacts and on for every
/// `cargo build` / `cargo test` binary, which are unsigned or ad-hoc-signed.
fn build_is_signed_release() -> bool {
    !cfg!(debug_assertions)
}

impl AuthService {
    /// Build a service over the given credential backend.
    pub fn new(backend: Arc<dyn CredentialBackend>) -> Arc<Self> {
        Self::new_with_lock_dir(backend, None)
    }

    fn new_with_lock_dir(
        backend: Arc<dyn CredentialBackend>,
        refresh_lock_dir: Option<PathBuf>,
    ) -> Arc<Self> {
        let service = Arc::new(Self {
            backend,
            refresh_lock_dir,
            http: reqwest::Client::new(),
            providers: Mutex::new(HashMap::new()),
            me: OnceLock::new(),
        });
        let _ = service.me.set(Arc::downgrade(&service));
        service
    }

    /// The default deployment backend under `<config_dir>/auth/`, selected by
    /// build provenance:
    ///
    /// - **signed release build** → [`AutoBackend`] (OS keychain first, file
    ///   fallback) — credentials rest in the OS vault;
    /// - **unsigned dev / test build** → file-only backend — the OS keychain is
    ///   never touched.
    ///
    /// Why gate on signing: the macOS Keychain ACL is keyed to the accessing
    /// binary's code signature. An unsigned / ad-hoc-signed dev or test binary
    /// (every `cargo build` / `cargo test` artifact) reading an item that a
    /// *differently*-signed release binary created is not in that item's ACL, so
    /// macOS pops a modal "allow access" prompt — which blocks headless PTY e2e
    /// tests until the nextest timeout kills them, and re-fires on every local
    /// rebuild. `COCO_CONFIG_DIR` cannot isolate this: it redirects the file
    /// backend but not the process-global, OS-session-scoped keychain namespace.
    /// Mirrors codex's LOCAL_DEV_BUILD keyring downgrade; signed releases keep the
    /// more secure keychain-backed store.
    pub fn with_config_dir(config_dir: std::path::PathBuf) -> Arc<Self> {
        let auth_dir = config_dir.join("auth");
        let backend: Arc<dyn CredentialBackend> = if build_is_signed_release() {
            Arc::new(AutoBackend::new(auth_dir.clone()))
        } else {
            Arc::new(store::FileBackend::new(auth_dir.clone()))
        };
        Self::new_with_lock_dir(backend, Some(auth_dir))
    }

    /// Build the service over an explicitly-pinned credential backend, for when
    /// the user forces a mode via `COCO_AUTH_CREDENTIAL_STORE` /
    /// `GlobalConfig.auth_credential_store`. Callers that leave it unset use
    /// [`Self::with_config_dir`] (the build-provenance default) instead. An
    /// explicit mode overrides the build-provenance heuristic — e.g. `File`
    /// keeps a locally-built (unsigned) `--release` binary off the keychain.
    pub fn with_store(config_dir: std::path::PathBuf, mode: CredentialStoreMode) -> Arc<Self> {
        let auth_dir = config_dir.join("auth");
        let backend: Arc<dyn CredentialBackend> = match mode {
            CredentialStoreMode::Auto => Arc::new(AutoBackend::new(auth_dir.clone())),
            CredentialStoreMode::File => Arc::new(store::FileBackend::new(auth_dir.clone())),
            CredentialStoreMode::Keyring => Arc::new(store::KeyringBackend::default()),
            CredentialStoreMode::Ephemeral => Arc::new(EphemeralBackend::default()),
        };
        let refresh_lock_dir = (mode != CredentialStoreMode::Ephemeral).then_some(auth_dir);
        Self::new_with_lock_dir(backend, refresh_lock_dir)
    }

    fn lock_providers(&self) -> std::sync::MutexGuard<'_, HashMap<String, ManagedProvider>> {
        self.providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Get-or-create the live cell for an instance. On a cache miss, the flow is
    /// discovered from the stored credential (or `flow_hint` when there is no
    /// stored credential yet, i.e. during login). Returns `None` when the
    /// instance has neither a stored credential nor a hint (i.e. not an
    /// OAuth-backed provider this service knows about).
    fn cell_for(&self, name: &str, flow_hint: Option<OAuthFlowId>) -> Option<TokenCell> {
        // Fast path: already managed.
        if let Some(p) = self.lock_providers().get(name) {
            if flow_hint.is_some_and(|expected| expected != p.descriptor.flow) {
                return None;
            }
            return Some(p.cell.clone());
        }
        // Cache miss: discover the flow + load any stored credential OUTSIDE the
        // lock (I/O), then get-or-create under the lock. We MUST return the cell
        // that actually lives in the map: under a concurrent first-touch, a
        // racing thread may have won the insert, and returning our local cell
        // would orphan it — the refresher and re-login only ever `store()` into
        // the map's cell, so an orphan would never see refreshed tokens/logout.
        let stored = self.backend.load(name).ok().flatten();
        if let (Some(credential), Some(expected)) = (&stored, flow_hint)
            && credential.flow != expected
        {
            warn!(
                provider = name,
                stored_flow = %credential.flow,
                configured_flow = %expected,
                "stored credential belongs to a different OAuth flow"
            );
            return None;
        }
        let flow = stored.as_ref().map(|c| c.flow).or(flow_hint)?;
        let descriptor = descriptor_for(flow)?;
        let local = match &stored {
            Some(c) => TokenCell::from_snapshot(c.to_snapshot()),
            None => TokenCell::empty(),
        };
        let cell = self
            .lock_providers()
            .entry(name.to_string())
            .or_insert_with(|| ManagedProvider {
                descriptor,
                cell: local,
                refresh_lock: Arc::new(Semaphore::new(1)),
                refresher: Mutex::new(None),
            })
            .cell
            .clone();
        // `spawn_refresher` is idempotent, so a race-loser calling it is a no-op.
        if cell.snapshot().is_some() {
            self.spawn_refresher(name);
        }
        Some(cell)
    }

    /// The per-instance refresh-serialization semaphore, if the instance is
    /// already managed. `login`/`logout` acquire it so their cell mutation
    /// cannot race an in-flight background refresh.
    fn refresh_lock_for(&self, name: &str) -> Option<Arc<Semaphore>> {
        self.lock_providers()
            .get(name)
            .map(|p| p.refresh_lock.clone())
    }

    /// Run the interactive login flow for a configured provider instance,
    /// persist the credential keyed by `provider_name`, and update its live
    /// cell. `flow` comes from the instance's `auth: OAuth { flow }`.
    pub async fn login(
        &self,
        provider_name: &str,
        flow: OAuthFlowId,
        opts: &LoginOptions,
    ) -> Result<ProviderAuthStatus> {
        let descriptor = descriptor_for(flow).ok_or_else(|| {
            error::InternalSnafu {
                message: format!("no descriptor wired for flow {flow}"),
            }
            .build()
        })?;
        let cell = self.cell_for(provider_name, Some(flow)).ok_or_else(|| {
            error::InternalSnafu {
                message: format!(
                    "provider '{provider_name}' is already bound to a different OAuth flow"
                ),
            }
            .build()
        })?;
        let lock = self.refresh_lock_for(provider_name).ok_or_else(|| {
            error::InternalSnafu {
                message: format!("missing refresh lock for provider '{provider_name}'"),
            }
            .build()
        })?;
        let mut cred = flow::login(descriptor, provider_name, opts, &self.http).await?;
        let _permit = lock.acquire().await.map_err(|e| {
            error::InternalSnafu {
                message: format!("refresh semaphore closed: {e}"),
            }
            .build()
        })?;
        let _process_lock =
            process_lock::acquire(self.refresh_lock_dir.as_deref(), provider_name).await?;
        // Bump login_epoch relative to any prior credential (identity change).
        let prev_epoch = self
            .backend
            .load(provider_name)?
            .map(|c| c.login_epoch)
            .unwrap_or(0);
        cred.login_epoch = prev_epoch.saturating_add(1);
        self.backend.save(provider_name, &cred)?;
        // Publish the freshly-acquired token into the live cell. On a re-login
        // `cell_for` returns the existing cell (holding the OLD token), so the
        // explicit `store` is load-bearing — and it is serialized under the
        // refresh lock so a racing in-flight refresh can't clobber it.
        cell.store(cred.to_snapshot());
        drop(_process_lock);
        drop(_permit);
        self.spawn_refresher(provider_name);
        self.status(provider_name, flow)
    }

    /// Adopt a credential obtained OUT-OF-BAND (e.g. imported from another
    /// tool's auth file) for a provider instance. Persists and publishes it
    /// through the SAME path as [`Self::login`] — `login_epoch` bump,
    /// `backend.save`, serialized cell `store`, refresher spawn — so the
    /// single-cell and rotating-refresh invariants hold. No network or
    /// interactive flow; `cred.flow` selects the provider's OAuth flow.
    pub async fn import(
        &self,
        provider_name: &str,
        cred: crate::store::StoredCredential,
    ) -> Result<ProviderAuthStatus> {
        let flow = cred.flow;
        let cell = self.cell_for(provider_name, Some(flow)).ok_or_else(|| {
            error::InternalSnafu {
                message: format!(
                    "provider '{provider_name}' is already bound to a different OAuth flow"
                ),
            }
            .build()
        })?;
        let lock = self.refresh_lock_for(provider_name).ok_or_else(|| {
            error::InternalSnafu {
                message: format!("missing refresh lock for provider '{provider_name}'"),
            }
            .build()
        })?;
        let _permit = lock.acquire().await.map_err(|e| {
            error::InternalSnafu {
                message: format!("refresh semaphore closed: {e}"),
            }
            .build()
        })?;
        let _process_lock =
            process_lock::acquire(self.refresh_lock_dir.as_deref(), provider_name).await?;
        let prev_epoch = self
            .backend
            .load(provider_name)?
            .map(|c| c.login_epoch)
            .unwrap_or(0);
        let mut cred = cred;
        cred.login_epoch = prev_epoch.saturating_add(1);
        self.backend.save(provider_name, &cred)?;
        cell.store(cred.to_snapshot());
        drop(_process_lock);
        drop(_permit);
        self.spawn_refresher(provider_name);
        self.status(provider_name, flow)
    }

    /// Clear stored credentials and the live cell (logout). Best-effort
    /// server-side token revocation runs first (failures don't block logout).
    /// Async because it serializes against an in-flight refresh: it aborts the
    /// background refresher, then takes the per-instance refresh lock before
    /// clearing, so a refresh that is mid network round-trip cannot `store()`
    /// resurrected credentials afterwards.
    pub async fn logout(&self, provider_name: &str) -> Result<bool> {
        // Materialize the managed cell/lock for a credential first touched by
        // logout, so local and background operations share one serialization
        // point even when no model has used the provider in this process.
        if let Some(credential) = self.backend.load(provider_name)? {
            let _ = self.cell_for(provider_name, Some(credential.flow));
        }

        // Take the refresh lock + the running refresher handle under the map
        // lock (same providers→refresher ordering as `spawn_refresher`).
        let (lock, refresher) = {
            let map = self.lock_providers();
            match map.get(provider_name) {
                Some(p) => (
                    Some(p.refresh_lock.clone()),
                    p.refresher
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take(),
                ),
                None => (None, None),
            }
        };
        if let Some(handle) = refresher {
            handle.abort(); // stop new refresh iterations
        }
        // Wait for any in-flight refresh to release the permit before clearing.
        let _permit = match &lock {
            Some(l) => l.acquire().await.ok(),
            None => None,
        };
        let _process_lock =
            process_lock::acquire(self.refresh_lock_dir.as_deref(), provider_name).await?;

        // Re-read after both locks: another process may have refreshed or
        // re-authenticated before this logout acquired the file lock.
        if let Some(credential) = self.backend.load(provider_name)?
            && let Some(descriptor) = descriptor_for(credential.flow)
        {
            let token = credential
                .refresh_token
                .clone()
                .unwrap_or_else(|| credential.access_token.clone());
            if let Err(error) = refresh::revoke(descriptor, &token, &self.http).await {
                warn!(
                    provider = provider_name,
                    "token revocation failed (continuing logout): {error}"
                );
            }
        }
        let removed = self.backend.delete(provider_name)?;
        if let Some(p) = self.lock_providers().get(provider_name) {
            p.cell.clear();
        }
        Ok(removed)
    }

    /// Current status for a provider instance. `flow` is the instance's
    /// configured flow (used for the status metadata + descriptor lookup).
    pub fn status(&self, provider_name: &str, flow: OAuthFlowId) -> Result<ProviderAuthStatus> {
        let stored = self.backend.load(provider_name).ok().flatten();
        let snap = self
            .cell_for(provider_name, Some(flow))
            .and_then(|c| c.snapshot());
        let now = now_ms();
        let state = match &snap {
            Some(s) if s.expires_at_ms.is_some_and(|e| now >= e) => AuthState::Expired,
            Some(_) => AuthState::Available,
            None => AuthState::NotConfigured,
        };
        let readiness = match state {
            AuthState::Available => AuthReadinessLevel::RequestValid,
            AuthState::Expired => AuthReadinessLevel::CredentialPresent,
            AuthState::NotConfigured => AuthReadinessLevel::None,
        };
        Ok(ProviderAuthStatus {
            provider_name: provider_name.to_string(),
            flow,
            display_name: descriptor_for(flow).map_or("", |d| d.display_name),
            state,
            readiness,
            refresh_support: AuthRefreshSupport::Automatic,
            email: stored.as_ref().and_then(|c| c.email.clone()),
            plan_type: stored.and_then(|c| c.plan_type),
            expires_at_ms: snap.and_then(|s| s.expires_at_ms),
        })
    }

    /// Spawn (idempotently) the background refresher for an instance. The task
    /// holds only a `Weak` to the service so it cannot keep it alive.
    fn spawn_refresher(&self, name: &str) {
        // `status()` / `subscription_creds()` are sync + public and reach here.
        // Degrade gracefully (no background refresh) rather than panicking when
        // called outside a tokio runtime.
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            warn!(
                provider = %name,
                "no tokio runtime; background token refresh disabled for this call"
            );
            return;
        };
        let Some(weak) = self.me.get().cloned() else {
            return;
        };
        let map = self.lock_providers();
        let Some(p) = map.get(name) else {
            return;
        };
        let mut guard = p
            .refresher
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.as_ref().is_some_and(|h| !h.is_finished()) {
            return; // already running
        }
        let name = name.to_string();
        let handle = rt.spawn(async move { run_refresher(weak, name).await });
        *guard = Some(handle);
    }
}

/// Background loop for one instance: sleep until ~60s before expiry, refresh,
/// repeat. Exits on logout (empty cell), terminal `SessionExpired`, or when the
/// owning `AuthService` is dropped (the `Weak` fails to upgrade). Never holds
/// the `Arc` across an `await`.
async fn run_refresher(weak: Weak<AuthService>, name: String) {
    loop {
        let (backend, http, descriptor, cell, lock, lock_dir, sleep_ms) = {
            let Some(service) = weak.upgrade() else {
                return;
            };
            let map = service.lock_providers();
            let Some(p) = map.get(&name) else {
                return;
            };
            let Some(snap) = p.cell.snapshot() else {
                return; // logged out
            };
            let sleep_ms = snap
                .expires_at_ms
                .map(|exp| (exp - now_ms() - 60_000).max(0))
                .unwrap_or(i64::from(u32::MAX)); // no expiry → effectively idle
            (
                service.backend.clone(),
                service.http.clone(),
                p.descriptor,
                p.cell.clone(),
                p.refresh_lock.clone(),
                service.refresh_lock_dir.clone(),
                sleep_ms,
            )
        };

        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms as u64)).await;
        if weak.upgrade().is_none() {
            return; // service dropped while we slept
        }

        match refresh_once(RefreshAttempt {
            backend: &backend,
            http: &http,
            provider_name: &name,
            descriptor,
            cell: &cell,
            lock: &lock,
            lock_dir: lock_dir.as_deref(),
            rejected_access_token: None,
        })
        .await
        {
            Ok(()) => {
                // Defensive anti-spin: if the refresh "succeeded" but the new
                // token is STILL near-expiry (server clock skew / an endpoint
                // issuing already-stale tokens), `sleep_ms` would recompute to
                // ~0 and we'd hammer the token endpoint. Back off instead. The
                // healthy path (token refreshed to a far-future expiry) skips
                // this entirely.
                if cell.snapshot().is_some_and(|s| s.needs_refresh(now_ms())) {
                    warn!(provider = %name, "refreshed token still near expiry; backing off");
                    tokio::time::sleep(std::time::Duration::from_secs(REFRESH_BACKOFF_SECS)).await;
                }
            }
            Err(ProviderAuthError::SessionExpired { .. }) => {
                warn!(provider = %name, "session expired; re-login required");
                return;
            }
            Err(e) => {
                warn!(provider = %name, "refresh failed: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(REFRESH_BACKOFF_SECS)).await;
            }
        }
    }
}

/// Fixed backoff applied after a failed refresh or a refresh that did not
/// advance the token's expiry, so the background loop can never busy-spin.
const REFRESH_BACKOFF_SECS: u64 = 30;

/// Serialized, double-checked refresh: acquire the per-instance lock, re-check
/// freshness (so concurrent expiry triggers collapse to ONE token exchange —
/// the rotating refresh token is single-use), refresh, update the cell, and
/// persist the rotated tokens. Because `login`/`logout` also mutate the cell
/// under this same lock, the post-acquire re-check below also correctly sees a
/// concurrent logout (snapshot `None`) or re-login (already fresh).
struct RefreshAttempt<'a> {
    backend: &'a Arc<dyn CredentialBackend>,
    http: &'a reqwest::Client,
    provider_name: &'a str,
    descriptor: &'static OAuthFlowDescriptor,
    cell: &'a TokenCell,
    lock: &'a Arc<Semaphore>,
    lock_dir: Option<&'a std::path::Path>,
    rejected_access_token: Option<&'a str>,
}

async fn refresh_once(attempt: RefreshAttempt<'_>) -> Result<()> {
    let RefreshAttempt {
        backend,
        http,
        provider_name,
        descriptor,
        cell,
        lock,
        lock_dir,
        rejected_access_token,
    } = attempt;
    let _permit = lock.acquire().await.map_err(|e| {
        error::InternalSnafu {
            message: format!("refresh semaphore closed: {e}"),
        }
        .build()
    })?;
    let _process_lock = process_lock::acquire(lock_dir, provider_name).await?;

    // Another process may have refreshed, logged out, or re-authenticated while
    // this process was waiting. Durable state wins and is adopted before the
    // freshness check, preventing reuse of a rotated refresh token.
    let durable = backend.load(provider_name)?;
    let Some(credential) = durable else {
        cell.clear();
        return Ok(());
    };
    if credential.flow != descriptor.flow {
        return Err(error::InternalSnafu {
            message: format!(
                "stored OAuth flow {} does not match configured flow {} for provider '{provider_name}'",
                credential.flow, descriptor.flow
            ),
        }
        .build());
    }
    let snap = credential.to_snapshot();
    cell.store(snap.clone());
    // Reactive refresh is bound to the token that actually received 401/403.
    // If another task/process already replaced it while we waited, adopting the
    // durable token is sufficient and avoids spending a second rotating token.
    if rejected_access_token.is_some_and(|rejected| rejected != snap.access_token) {
        return Ok(());
    }
    if rejected_access_token.is_none() && !snap.needs_refresh(now_ms()) {
        return Ok(()); // someone else already refreshed (or re-login made it fresh)
    }
    let new_snap = refresh::refresh(descriptor, provider_name, &snap, http).await?;
    persist_refreshed(backend, provider_name, descriptor.flow, &new_snap)?;
    cell.store(new_snap);
    Ok(())
}

/// Persist refreshed tokens, preserving the durable identity fields
/// (`login_epoch` / `email`) from the prior stored credential.
fn persist_refreshed(
    backend: &Arc<dyn CredentialBackend>,
    provider_name: &str,
    flow: OAuthFlowId,
    snap: &TokenSnapshot,
) -> Result<()> {
    let prev = backend.load(provider_name)?;
    let cred = StoredCredential {
        flow,
        access_token: snap.access_token.clone(),
        refresh_token: snap.refresh_token.clone(),
        id_token: prev.as_ref().and_then(|c| c.id_token.clone()),
        account_id: snap.account_id.clone(),
        principal: snap.principal.clone(),
        expires_at_ms: snap.expires_at_ms,
        plan_type: snap.subscription_type.clone(),
        email: prev.as_ref().and_then(|c| c.email.clone()),
        // Carried on the live snapshot (never reset), so a transiently
        // unreadable backend can't silently downgrade the identity epoch.
        login_epoch: snap.login_epoch,
    };
    backend.save(provider_name, &cred)
}

impl ProviderCredentialResolver for AuthService {
    fn subscription_creds(
        &self,
        provider_name: &str,
        expected_flow: OAuthFlowId,
    ) -> Option<SubscriptionCredsSupplier> {
        // Lazily load the instance's cell and verify the durable credential's
        // flow matches the provider configuration before exposing a supplier.
        let cell = self.cell_for(provider_name, Some(expected_flow))?;
        cell.snapshot()?; // only report a supplier when logged in
        Some(cell.supplier())
    }

    fn refresh_now(&self, provider_name: &str) -> RefreshFuture {
        // Only managed (already-resolved) instances can refresh. A 401 implies
        // a client was built, so the instance is in the map; if not, no-op.
        let parts = {
            let map = self.lock_providers();
            map.get(provider_name).map(|p| {
                (
                    p.descriptor,
                    p.cell.clone(),
                    p.refresh_lock.clone(),
                    p.cell.snapshot().map(|snapshot| snapshot.access_token),
                )
            })
        };
        let lock_dir = self.refresh_lock_dir.clone();
        let backend = self.backend.clone();
        let http = self.http.clone();
        let name = provider_name.to_string();
        Box::pin(async move {
            let Some((descriptor, cell, lock, rejected_access_token)) = parts else {
                return false;
            };
            let Some(rejected_access_token) = rejected_access_token else {
                return false;
            };
            refresh_once(RefreshAttempt {
                backend: &backend,
                http: &http,
                provider_name: &name,
                descriptor,
                cell: &cell,
                lock: &lock,
                lock_dir: lock_dir.as_deref(),
                rejected_access_token: Some(&rejected_access_token),
            })
            .await
            .is_ok()
        })
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
