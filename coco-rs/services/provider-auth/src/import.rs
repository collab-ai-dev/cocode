//! Import a credential from another tool's on-disk auth file (e.g. the Codex
//! CLI's `~/.codex/auth.json`) into coco's own store. User-initiated and
//! explicit-path only — no discovery, no auto-scan. The source is validated
//! (regular file, not a symlink) before any read, and is never modified.

use std::path::Path;

use coco_types::OAuthFlowId;
use serde::Deserialize;

use crate::error::InternalSnafu;
use crate::error::Result;
use crate::error::StoreSnafu;
use crate::jwt;
use crate::store::StoredCredential;

/// OpenAI id_token claim path holding the ChatGPT account id — mirrors the
/// descriptor's `AccountIdSource::IdTokenClaim` used by the live OAuth flow.
const CHATGPT_ACCOUNT_ID_CLAIM: &[&str] = &["https://api.openai.com/auth", "chatgpt_account_id"];

/// The subset of `~/.codex/auth.json` (ChatGPT/Codex CLI) that we map. Unknown
/// keys are ignored so format drift in fields we don't use is harmless.
#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    tokens: Option<CodexTokens>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

/// Read + validate an external Codex auth file into a `StoredCredential` for
/// the ChatGPT subscription flow ([`OAuthFlowId::OpenAiChatGpt`]). Rejects
/// symlinks and non-regular files before reading; never writes to `path`.
///
/// `expires_at_ms` is recovered from the access-token JWT `exp` because the
/// codex file omits it — without this the imported token would look
/// non-expiring, never proactively refresh, and 401 on first use.
pub fn read_codex_auth(path: &Path) -> Result<StoredCredential> {
    // Validate BEFORE reading: reject a symlinked / non-regular source outright
    // (do not canonicalize-then-follow) — the path is attacker-influenceable.
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        StoreSnafu {
            message: format!("cannot stat import file {}: {e}", path.display()),
        }
        .build()
    })?;
    if meta.file_type().is_symlink() {
        return StoreSnafu {
            message: format!(
                "refusing to import a symlinked credential file: {}",
                path.display()
            ),
        }
        .fail();
    }
    if !meta.is_file() {
        return StoreSnafu {
            message: format!("import path is not a regular file: {}", path.display()),
        }
        .fail();
    }

    let raw = std::fs::read_to_string(path).map_err(|e| {
        StoreSnafu {
            message: format!("cannot read import file {}: {e}", path.display()),
        }
        .build()
    })?;
    let parsed: CodexAuthFile = serde_json::from_str(&raw).map_err(|e| {
        InternalSnafu {
            message: format!("invalid codex auth JSON in {}: {e}", path.display()),
        }
        .build()
    })?;
    let tokens = parsed.tokens.ok_or_else(|| {
        InternalSnafu {
            message: format!("no `tokens` object in {}", path.display()),
        }
        .build()
    })?;

    let account_id = tokens.account_id.clone().or_else(|| {
        tokens
            .id_token
            .as_deref()
            .and_then(|t| jwt::read_string_claim(t, CHATGPT_ACCOUNT_ID_CLAIM))
    });
    let expires_at_ms = jwt::read_exp_ms(&tokens.access_token);
    let principal = crate::store::OAuthPrincipal::from_access_token(&tokens.access_token);

    Ok(StoredCredential {
        flow: OAuthFlowId::OpenAiChatGpt,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        account_id,
        principal,
        expires_at_ms,
        plan_type: None,
        email: None,
        login_epoch: 0,
    })
}

#[cfg(test)]
#[path = "import.test.rs"]
mod tests;
