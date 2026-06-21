//! `coco-provider-auth` — interactive OAuth login + subscription credential
//! management for LLM providers (OpenAI ChatGPT subscription in P1).
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
pub mod error;
pub mod flow;
pub mod jwt;
pub mod pkce;
pub mod refresh;
pub mod store;
pub mod token_cell;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::Weak;

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
    http: reqwest::Client,
    /// Lazily populated, keyed by provider-instance name (`providers.<name>`).
    providers: Mutex<HashMap<String, ManagedProvider>>,
    /// Weak self-ref so `&self` methods can spawn `Weak`-holding refreshers.
    me: OnceLock<Weak<AuthService>>,
}

impl AuthService {
    /// Build a service over the given credential backend.
    pub fn new(backend: Arc<dyn CredentialBackend>) -> Arc<Self> {
        let service = Arc::new(Self {
            backend,
            http: reqwest::Client::new(),
            providers: Mutex::new(HashMap::new()),
            me: OnceLock::new(),
        });
        let _ = service.me.set(Arc::downgrade(&service));
        service
    }

    /// Convenience: keyring-with-file-fallback backend under `<config_dir>/auth/`.
    pub fn with_config_dir(config_dir: std::path::PathBuf) -> Arc<Self> {
        Self::new(Arc::new(AutoBackend::new(config_dir.join("auth"))))
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
            return Some(p.cell.clone());
        }
        // Cache miss: discover the flow + load any stored credential OUTSIDE the
        // lock (I/O), then get-or-create under the lock. We MUST return the cell
        // that actually lives in the map: under a concurrent first-touch, a
        // racing thread may have won the insert, and returning our local cell
        // would orphan it — the refresher and re-login only ever `store()` into
        // the map's cell, so an orphan would never see refreshed tokens/logout.
        let stored = self.backend.load(name).ok().flatten();
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
        let mut cred = flow::login(descriptor, provider_name, opts, &self.http).await?;
        // Bump login_epoch relative to any prior credential (identity change).
        let prev_epoch = self
            .backend
            .load(provider_name)
            .ok()
            .flatten()
            .map(|c| c.login_epoch)
            .unwrap_or(0);
        cred.login_epoch = prev_epoch.saturating_add(1);
        self.backend.save(provider_name, &cred)?;
        // Publish the freshly-acquired token into the live cell. On a re-login
        // `cell_for` returns the existing cell (holding the OLD token), so the
        // explicit `store` is load-bearing — and it is serialized under the
        // refresh lock so a racing in-flight refresh can't clobber it.
        if let Some(cell) = self.cell_for(provider_name, Some(flow)) {
            let lock = self.refresh_lock_for(provider_name);
            let _permit = match &lock {
                Some(l) => l.acquire().await.ok(),
                None => None,
            };
            cell.store(cred.to_snapshot());
        }
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
        // Best-effort RFC 7009 revocation BEFORE clearing local state, so the
        // grant is invalidated server-side, not just forgotten locally.
        if let Some(cred) = self.backend.load(provider_name).ok().flatten()
            && let Some(descriptor) = descriptor_for(cred.flow)
        {
            let token = cred
                .refresh_token
                .clone()
                .unwrap_or_else(|| cred.access_token.clone());
            if let Err(e) = refresh::revoke(descriptor, &token, &self.http).await {
                warn!(
                    provider = provider_name,
                    "token revocation failed (continuing logout): {e}"
                );
            }
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
        let (backend, http, descriptor, cell, lock, sleep_ms) = {
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
                sleep_ms,
            )
        };

        tokio::time::sleep(std::time::Duration::from_millis(sleep_ms as u64)).await;
        if weak.upgrade().is_none() {
            return; // service dropped while we slept
        }

        match refresh_once(
            &backend, &http, &name, descriptor, &cell, &lock, /* force */ false,
        )
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
async fn refresh_once(
    backend: &Arc<dyn CredentialBackend>,
    http: &reqwest::Client,
    provider_name: &str,
    descriptor: &'static OAuthFlowDescriptor,
    cell: &TokenCell,
    lock: &Arc<Semaphore>,
    force: bool,
) -> Result<()> {
    let _permit = lock.acquire().await.map_err(|e| {
        error::InternalSnafu {
            message: format!("refresh semaphore closed: {e}"),
        }
        .build()
    })?;
    let Some(snap) = cell.snapshot() else {
        return Ok(()); // logged out while we waited
    };
    // `force` (reactive 401) refreshes even when not near-expiry — a rejected
    // token may be revoked or clock-skewed rather than clock-expired.
    if !force && !snap.needs_refresh(now_ms()) {
        return Ok(()); // someone else already refreshed (or re-login made it fresh)
    }
    let new_snap = refresh::refresh(descriptor, provider_name, &snap, http).await?;
    cell.store(new_snap.clone());
    persist_refreshed(backend, provider_name, descriptor.flow, &new_snap);
    Ok(())
}

/// Persist refreshed tokens, preserving the durable identity fields
/// (`login_epoch` / `email`) from the prior stored credential.
fn persist_refreshed(
    backend: &Arc<dyn CredentialBackend>,
    provider_name: &str,
    flow: OAuthFlowId,
    snap: &TokenSnapshot,
) {
    let prev = backend.load(provider_name).ok().flatten();
    let cred = StoredCredential {
        flow,
        access_token: snap.access_token.clone(),
        refresh_token: snap.refresh_token.clone(),
        id_token: prev.as_ref().and_then(|c| c.id_token.clone()),
        account_id: snap.account_id.clone(),
        expires_at_ms: snap.expires_at_ms,
        plan_type: snap.subscription_type.clone(),
        email: prev.as_ref().and_then(|c| c.email.clone()),
        // Carried on the live snapshot (never reset), so a transiently
        // unreadable backend can't silently downgrade the identity epoch.
        login_epoch: snap.login_epoch,
    };
    if let Err(e) = backend.save(provider_name, &cred) {
        warn!(
            provider = provider_name,
            "persist refreshed credential: {e}"
        );
    }
}

impl ProviderCredentialResolver for AuthService {
    fn subscription_creds(&self, provider_name: &str) -> Option<SubscriptionCredsSupplier> {
        // Lazily load the instance's cell (keyed by name). `None` flow hint
        // means we only resolve when a stored credential exists — so api-key
        // providers / never-logged-in instances correctly report no supplier
        // and callers can gate availability.
        let cell = self.cell_for(provider_name, None)?;
        cell.snapshot()?; // only report a supplier when logged in
        Some(cell.supplier())
    }

    fn refresh_now(&self, provider_name: &str) -> RefreshFuture {
        // Only managed (already-resolved) instances can refresh. A 401 implies
        // a client was built, so the instance is in the map; if not, no-op.
        let parts = {
            let map = self.lock_providers();
            map.get(provider_name)
                .map(|p| (p.descriptor, p.cell.clone(), p.refresh_lock.clone()))
        };
        let backend = self.backend.clone();
        let http = self.http.clone();
        let name = provider_name.to_string();
        Box::pin(async move {
            let Some((descriptor, cell, lock)) = parts else {
                return false;
            };
            refresh_once(
                &backend, &http, &name, descriptor, &cell, &lock, /* force */ true,
            )
            .await
            .is_ok()
        })
    }
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
