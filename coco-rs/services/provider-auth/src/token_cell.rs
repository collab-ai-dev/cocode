//! Process-stable, lock-free credential cell. One `TokenCell` per provider for
//! the process lifetime; refresh AND interactive re-login both `store()` into
//! the SAME cell so the per-request header closure (built once at provider
//! construction) always serves live credentials with zero client rebuild.

use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use coco_inference::SubscriptionCreds;
use coco_inference::SubscriptionCredsSupplier;

/// In-memory, cloneable credential snapshot read on every request. `account_id`
/// lives here too so an account switch is transparent. Tokens are redacted in
/// `Debug`.
#[derive(Clone)]
pub struct TokenSnapshot {
    pub access_token: String,
    pub account_id: Option<String>,
    pub refresh_token: Option<String>,
    pub subscription_type: Option<String>,
    pub expires_at_ms: Option<i64>,
    /// Credential-identity epoch, carried through refresh so a re-persist never
    /// loses it (it bumps only on a fresh `login`). Lives on the live cell so it
    /// survives even when the backing store is transiently unreadable.
    pub login_epoch: u64,
}

impl fmt::Debug for TokenSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TokenSnapshot")
            .field("access_token", &"<redacted>")
            .field("account_id", &self.account_id)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("subscription_type", &self.subscription_type)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("login_epoch", &self.login_epoch)
            .finish()
    }
}

impl TokenSnapshot {
    /// Within 5 min of expiry (or already expired). `None` expiry never refreshes.
    pub fn needs_refresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms
            .is_some_and(|exp| now_ms >= exp - 300_000)
    }

    fn to_subscription_creds(&self) -> SubscriptionCreds {
        SubscriptionCreds {
            access_token: self.access_token.clone(),
            account_id: self.account_id.clone(),
            subscription_type: self.subscription_type.clone(),
            project_id: None,
        }
    }
}

/// Lock-free shared cell. `Clone` shares the same underlying `ArcSwapOption`.
#[derive(Clone, Default)]
pub struct TokenCell {
    inner: Arc<ArcSwapOption<TokenSnapshot>>,
}

impl TokenCell {
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(ArcSwapOption::empty()),
        }
    }

    pub fn from_snapshot(snap: TokenSnapshot) -> Self {
        Self {
            inner: Arc::new(ArcSwapOption::new(Some(Arc::new(snap)))),
        }
    }

    /// Replace the live snapshot (login / refresh). Never replaces the cell
    /// itself, preserving the supplier closures already handed out.
    pub fn store(&self, snap: TokenSnapshot) {
        self.inner.store(Some(Arc::new(snap)));
    }

    /// Clear the credential (logout) — subsequent requests carry no bearer.
    pub fn clear(&self) {
        self.inner.store(None);
    }

    /// Current snapshot, if logged in.
    pub fn snapshot(&self) -> Option<TokenSnapshot> {
        self.inner.load_full().map(|s| (*s).clone())
    }

    /// A synchronous, lock-free supplier for the provider header closure.
    pub fn supplier(&self) -> SubscriptionCredsSupplier {
        let inner = self.inner.clone();
        Arc::new(move || inner.load_full().map(|s| s.to_subscription_creds()))
    }
}
