//! `coco login` / `coco logout` command handlers + the session-shared
//! `AuthService` accessor used by every role-client construction site.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use anyhow::Result;
use anyhow::bail;
use coco_config::ProviderAuth;
use coco_config::global_config;
use coco_inference::ProviderCredentialResolver;
use coco_provider_auth::AuthService;
use coco_provider_auth::LoginOptions;
use coco_types::OAuthFlowId;

/// Process-wide, lazily-built `AuthService` shared by every client-construction
/// site (Main via `create_api_client`, role runtimes, subagents,
/// side-queries). One instance per process means exactly one
/// `TokenCell` and one serialized refresher per provider — the single-cell
/// invariant that keeps a rotating, single-use refresh token from being
/// double-spent by two `AuthService`s. The strong ref held here keeps the
/// background refresher alive for the session; the refresher holds only a `Weak`.
///
/// First call must be inside the tokio runtime (it spawns a refresher only when
/// already logged in); all CLI client-build paths run under `#[tokio::main]`.
pub fn shared_auth_service() -> Arc<AuthService> {
    static SERVICE: OnceLock<Arc<AuthService>> = OnceLock::new();
    SERVICE
        .get_or_init(|| AuthService::with_config_dir(global_config::config_home()))
        .clone()
}

/// The shared service as a `ProviderCredentialResolver` trait object, for
/// `model_factory` / `ModelRuntimeRegistry` / subagent / side-query client builds.
pub fn shared_resolver() -> Arc<dyn ProviderCredentialResolver> {
    shared_auth_service()
}

/// Resolve the CLI argument to a configured provider-INSTANCE name. The login
/// argument selects which configured provider to activate; `None` and the
/// shorthands `openai`/`chatgpt` map to the builtin OAuth instance
/// `openai-chatgpt`. Any other value is taken literally — so a user with a
/// second configured OAuth instance (e.g. `openai-chat-oauth`, or two accounts)
/// logs each in by its own name.
pub fn instance_name(provider: Option<&str>) -> String {
    use coco_config::builtin::GEMINI_CODE_ASSIST_PROVIDER;
    use coco_config::builtin::OPENAI_CHATGPT_PROVIDER;
    match provider {
        // `openai-oauth` / `oauth` are auth-method shorthands for users who
        // think in "which login" rather than "which product" — they resolve
        // to the same ChatGPT-subscription instance as `openai` / `chatgpt`.
        None | Some("openai") | Some("chatgpt") | Some("openai-oauth") | Some("oauth") => {
            OPENAI_CHATGPT_PROVIDER.to_string()
        }
        Some("gemini") | Some("google") => GEMINI_CODE_ASSIST_PROVIDER.to_string(),
        Some(other) => other.to_string(),
    }
}

/// Look up a configured provider instance's `auth` mode, preferring the fully
/// layered config (honors `config home/providers.json` + settings) and falling back
/// to builtins when a full build needs a Main model that isn't set yet (e.g. a
/// brand-new machine running `coco login` before configuring anything).
fn provider_auth_for(provider_name: &str) -> Result<ProviderAuth> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    if let Ok(rc) = coco_config::RuntimeConfigBuilder::from_process(&cwd).build() {
        return rc
            .providers
            .get(provider_name)
            .map(|p| p.auth.clone())
            .ok_or_else(|| {
                let config_dir = coco_utils_common::COCO_CONFIG_DIR_NAME;
                anyhow::anyhow!(
                    "unknown provider '{provider_name}' — configure it under `providers` in \
                     ~/{config_dir}/providers.json, or use a builtin (e.g. `openai`)"
                )
            });
    }
    coco_config::builtin_providers()
        .ok()
        .and_then(|ps| {
            ps.into_iter()
                .find(|p| p.name == provider_name)
                .map(|p| p.auth)
        })
        .ok_or_else(|| anyhow::anyhow!("unknown provider '{provider_name}'"))
}

/// Resolve the OAuth flow for a provider instance, or explain why it isn't an
/// OAuth-login provider.
fn oauth_flow_for(provider_name: &str) -> Result<OAuthFlowId> {
    match provider_auth_for(provider_name)? {
        ProviderAuth::OAuth { flow } => Ok(flow),
        ProviderAuth::ApiKey => bail!(
            "provider '{provider_name}' authenticates with an API key, not OAuth login — \
             set its env var (or `api_key` in providers.json) instead"
        ),
    }
}

/// `coco login [provider] [--no-browser]`. The argument selects the configured
/// provider instance to activate (multiple instances/accounts log in separately).
pub async fn run_login(
    provider: Option<String>,
    no_browser: bool,
    import: Option<PathBuf>,
) -> Result<()> {
    let name = instance_name(provider.as_deref());
    let flow = oauth_flow_for(&name)?;
    if let Some(path) = import {
        return run_import(&name, flow, &path).await;
    }
    let service = AuthService::with_config_dir(global_config::config_home());
    let opts = LoginOptions {
        open_browser: !no_browser,
        // Headless / SSH (`--no-browser`): the browser is likely on another
        // machine, so enable the paste fallback alongside the loopback wait.
        paste: no_browser,
        ..LoginOptions::interactive()
    };
    let status = service
        .login(&name, flow, &opts)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let who = status.email.as_deref().unwrap_or("your account");
    let plan = status.plan_type.as_deref().unwrap_or(status.display_name);
    println!(
        "\n✓ Logged in to `{}` ({}) as {who} ({plan}).",
        status.provider_name, status.display_name
    );
    println!(
        "  Use it by binding a model role to `{}` (e.g. `--models.main {}/gpt-5.5`, \
         or set it as a role in settings.json).",
        status.provider_name, status.provider_name
    );
    Ok(())
}

/// `coco login <provider> --import <path>` — adopt an existing credential from
/// another tool's auth file (e.g. the Codex CLI's `~/.codex/auth.json`) instead
/// of running OAuth. ChatGPT/Codex (`OpenAiChatGpt`) only for now — the other
/// wired flow (Gemini) has no compatible external file format. The path is read
/// once and never modified; symlinks are rejected in `read_codex_auth`.
async fn run_import(name: &str, flow: OAuthFlowId, path: &Path) -> Result<()> {
    if flow != OAuthFlowId::OpenAiChatGpt {
        bail!(
            "`--import` currently supports only the ChatGPT/Codex subscription \
             (`coco login openai --import …`); provider '{name}' uses a different flow"
        );
    }
    let cred =
        coco_provider_auth::import::read_codex_auth(path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let service = AuthService::with_config_dir(global_config::config_home());
    let status = service
        .import(name, cred)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let who = status.email.as_deref().unwrap_or("your account");
    let plan = status.plan_type.as_deref().unwrap_or(status.display_name);
    println!(
        "\n✓ Imported `{}` credential ({}) as {who} ({plan}) from {}.",
        status.provider_name,
        status.display_name,
        path.display()
    );
    println!(
        "  Use it by binding a model role to `{}` (e.g. `--models.main {}/gpt-5-5`).",
        status.provider_name, status.provider_name
    );
    Ok(())
}

/// `coco logout [provider]`.
pub async fn run_logout(provider: Option<String>) -> Result<()> {
    let name = instance_name(provider.as_deref());
    let service = AuthService::with_config_dir(global_config::config_home());
    let removed = service
        .logout(&name)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if removed {
        println!("✓ Logged out of `{name}`.");
    } else {
        println!("No stored credentials to clear for `{name}`.");
    }
    Ok(())
}

/// In-session `/login`: logs into the **shared** `AuthService` (so the running
/// session's clients immediately see the new token via the live cell — the
/// reactive header closure reads the same cell) and returns a status message
/// for the transcript. The authorize URL is handed to `url_sink` (shown in the
/// TUI) instead of printed. Loopback-only — the TUI owns stdin, so no paste.
pub async fn run_login_session(
    provider: Option<String>,
    url_sink: Arc<dyn Fn(String) + Send + Sync>,
) -> Result<String> {
    let name = instance_name(provider.as_deref());
    let flow = oauth_flow_for(&name)?;
    let service = shared_auth_service();
    let opts = LoginOptions {
        open_browser: true,
        paste: false,
        on_authorize_url: Some(url_sink),
        ..LoginOptions::interactive()
    };
    let status = service
        .login(&name, flow, &opts)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let who = status.email.as_deref().unwrap_or("your account");
    let plan = status.plan_type.as_deref().unwrap_or(status.display_name);
    Ok(format!(
        "✓ Logged in to `{}` ({}) as {who} ({plan}). Bind a model role to `{}` to use it.",
        status.provider_name, status.display_name, status.provider_name
    ))
}

/// In-session `/logout`: clears credentials on the **shared** `AuthService`.
pub async fn run_logout_session(provider: Option<String>) -> Result<String> {
    let name = instance_name(provider.as_deref());
    let service = shared_auth_service();
    let removed = service
        .logout(&name)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(if removed {
        format!("✓ Logged out of `{name}`.")
    } else {
        format!("No stored credentials to clear for `{name}`.")
    })
}

/// Print per-provider OAuth login state for every OAuth-configured provider in
/// `runtime_config`. Consumed by `coco status` / `coco doctor` so a logged-out
/// (or expired) subscription surfaces an actionable hint instead of a bare
/// "(mock mode)". No-op when no provider uses OAuth.
pub fn print_auth_status(runtime_config: &coco_config::RuntimeConfig) {
    let service = shared_auth_service();
    let mut printed_header = false;
    for (name, cfg) in &runtime_config.providers {
        let ProviderAuth::OAuth { flow } = cfg.auth else {
            continue;
        };
        if !printed_header {
            println!("provider login:");
            printed_header = true;
        }
        match service.status(name, flow) {
            Ok(st) => {
                let detail = st.email.as_deref().unwrap_or(st.display_name);
                let state = match st.state {
                    coco_types::AuthState::Available => "logged in",
                    coco_types::AuthState::Expired => "expired (auto-refresh failed)",
                    coco_types::AuthState::NotConfigured => "not logged in — run `coco login`",
                };
                println!("  [{name}] {state} ({detail})");
            }
            Err(e) => println!("  [{name}] status error: {e}"),
        }
    }
}
