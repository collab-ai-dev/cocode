//! Interactive login orchestration: PKCE + loopback callback (the
//! `tiny_http` pattern from `services/rmcp-client/src/perform_oauth_login.rs`)
//! → authorization-code exchange → `StoredCredential`.
//!
//! Implements the `RedirectStrategy::Loopback` path with an optional **paste
//! fallback** (`LoginOptions.paste`): for SSH/headless logins it races the
//! loopback callback against a redirect-URL/code pasted on stdin. The
//! descriptor-level `RedirectStrategy::LoopbackOrPaste` variant (a *hosted*
//! paste page, for a future Claude flow) is still unimplemented.

use std::time::Duration;
use std::time::Instant;

use crate::descriptor::AccountIdSource;
use crate::descriptor::OAuthFlowDescriptor;
use crate::descriptor::RedirectStrategy;
use crate::descriptor::StateStrategy;
use crate::error::CallbackSnafu;
use crate::error::InternalSnafu;
use crate::error::LoopbackBindSnafu;
use crate::error::Result;
use crate::error::StateMismatchSnafu;
use crate::pkce::PkceCodes;
use crate::pkce::generate_pkce;
use crate::pkce::generate_state;
use crate::refresh::account_id_from_id_token;
use crate::refresh::expires_at_ms;
use crate::refresh::post_token;
use crate::store::StoredCredential;

/// Sink for the authorize URL — set by in-session (`/login`) callers so the URL
/// can be surfaced in the TUI transcript instead of `eprintln`.
pub type AuthorizeUrlSink = std::sync::Arc<dyn Fn(String) + Send + Sync>;

/// Options for an interactive login. Construct via [`LoginOptions::interactive`]
/// (there is intentionally no `Default` — a zero `timeout` would expire at once).
#[derive(Clone)]
pub struct LoginOptions {
    /// Open the system browser to the authorize URL.
    pub open_browser: bool,
    /// Enable the paste fallback: in addition to the loopback listener, accept
    /// the redirect URL (or bare code) pasted on stdin. Needed when the browser
    /// runs on a different machine than the CLI (SSH / headless), where the
    /// loopback callback can't be delivered.
    pub paste: bool,
    /// How long to wait for the loopback callback before giving up.
    pub timeout: Duration,
    /// When set, the authorize URL is handed here instead of printed to stderr
    /// (used by the `/login` slash command to show it in the transcript).
    pub on_authorize_url: Option<AuthorizeUrlSink>,
}

impl LoginOptions {
    /// Interactive CLI defaults: open the browser, loopback-only, 5-min timeout.
    pub fn interactive() -> Self {
        Self {
            open_browser: true,
            paste: false,
            timeout: Duration::from_secs(300),
            on_authorize_url: None,
        }
    }
}

impl std::fmt::Debug for LoginOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginOptions")
            .field("open_browser", &self.open_browser)
            .field("paste", &self.paste)
            .field("timeout", &self.timeout)
            .field("on_authorize_url", &self.on_authorize_url.is_some())
            .finish()
    }
}

struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Run the interactive login flow for `descriptor`. `provider_name` is the
/// configured provider-instance name (used only in error messages).
pub async fn login(
    descriptor: &OAuthFlowDescriptor,
    provider_name: &str,
    opts: &LoginOptions,
    http: &reqwest::Client,
) -> Result<StoredCredential> {
    let pkce = generate_pkce();
    let state = match descriptor.state {
        StateStrategy::SeparateRandom => generate_state(),
        StateStrategy::VerifierAsState => pkce.code_verifier.clone(),
    };

    let (default_port, fallback_port, callback_path) = match descriptor.redirect {
        RedirectStrategy::Loopback {
            default_port,
            fallback_port,
            callback_path,
        } => (default_port, fallback_port, callback_path),
        RedirectStrategy::LoopbackOrPaste { .. } => {
            return Err(InternalSnafu {
                message: "loopback-or-paste login is not implemented in this build",
            }
            .build());
        }
    };

    let (server, redirect_uri) = bind_loopback(default_port, fallback_port, callback_path)?;
    let auth_url = build_authorize_url(descriptor, &pkce, &state, &redirect_uri);

    if opts.open_browser {
        let _ = webbrowser::open(&auth_url);
    }
    // Surface the URL: to the in-session sink (TUI transcript) when set,
    // otherwise to stderr for the CLI.
    match &opts.on_authorize_url {
        Some(sink) => sink(auth_url.clone()),
        None => {
            eprintln!("\nIf your browser didn't open, visit this URL to sign in:\n{auth_url}\n")
        }
    }

    // When paste is enabled, accept EITHER the loopback callback OR a pasted
    // redirect URL/code — whichever arrives first. Otherwise loopback only.
    let params = if opts.paste {
        tokio::select! {
            r = wait_for_callback(server, callback_path, opts.timeout) => r?,
            r = read_pasted_callback() => r?,
        }
    } else {
        wait_for_callback(server, callback_path, opts.timeout).await?
    };

    if let Some(err) = params.error {
        return Err(CallbackSnafu { message: err }.build());
    }
    // The loopback callback always carries `state` (strict CSRF check). A pasted
    // bare code may omit it — accepted only in paste mode (a deliberate,
    // user-initiated paste). A returned state, if present, must always match.
    let state_ok = match params.state.as_deref() {
        Some(returned) => returned == state,
        None => opts.paste,
    };
    if !state_ok {
        return Err(StateMismatchSnafu {
            provider: provider_name.to_string(),
        }
        .build());
    }
    let code = params.code.ok_or_else(|| {
        CallbackSnafu {
            message: "callback missing authorization code".to_string(),
        }
        .build()
    })?;

    // Authorization-code → token exchange.
    let mut exchange_params: Vec<(&str, String)> = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", descriptor.client_id.to_string()),
        ("code_verifier", pkce.code_verifier.clone()),
    ];
    if let Some(secret) = descriptor.client_secret {
        exchange_params.push(("client_secret", secret.to_string()));
    }
    let tr = post_token(
        http,
        &descriptor.effective_token_url(),
        descriptor.exchange_encoding,
        &exchange_params,
    )
    .await?;

    let account_id = tr
        .id_token
        .as_ref()
        .and_then(|jwt| account_id_from_id_token(descriptor, jwt));
    // Account email: from a userinfo endpoint (Google) when configured; for
    // JWT-claim providers it stays `None` (their account id is the routing key).
    let email = match descriptor.account_id {
        AccountIdSource::UserInfoEndpoint { url, field } => {
            fetch_userinfo_field(http, url, &tr.access_token, field).await
        }
        _ => None,
    };
    let expires = expires_at_ms(&tr);

    Ok(StoredCredential {
        flow: descriptor.flow,
        access_token: tr.access_token,
        refresh_token: tr.refresh_token,
        id_token: tr.id_token,
        account_id,
        expires_at_ms: expires,
        plan_type: None,
        email,
        login_epoch: 0,
    })
}

/// Best-effort GET of a string field from a userinfo endpoint (bearer auth).
/// Failure is non-fatal: login still succeeds, the field is just left unset.
async fn fetch_userinfo_field(
    http: &reqwest::Client,
    url: &str,
    access_token: &str,
    field: &str,
) -> Option<String> {
    let resp = http.get(url).bearer_auth(access_token).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get(field)?.as_str().map(str::to_string)
}

fn build_authorize_url(
    descriptor: &OAuthFlowDescriptor,
    pkce: &PkceCodes,
    state: &str,
    redirect_uri: &str,
) -> String {
    let enc = urlencoding::encode;
    let mut url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&code_challenge={}&code_challenge_method=S256&state={}",
        descriptor.authorize_url,
        enc(descriptor.client_id),
        enc(redirect_uri),
        enc(descriptor.scope),
        enc(&pkce.code_challenge),
        enc(state),
    );
    for (k, v) in descriptor.authorize_extra {
        url.push('&');
        url.push_str(&format!("{}={}", enc(k), enc(v)));
    }
    url
}

/// Bind a loopback HTTP server, preferring `default_port`, then `fallback_port`,
/// then an ephemeral port. Returns the server + the matching `redirect_uri`.
fn bind_loopback(
    default_port: u16,
    fallback_port: Option<u16>,
    callback_path: &str,
) -> Result<(tiny_http::Server, String)> {
    let mut candidates = vec![default_port];
    if let Some(fp) = fallback_port {
        candidates.push(fp);
    }
    candidates.push(0); // ephemeral

    let mut last_err = String::new();
    for port in candidates {
        match tiny_http::Server::http(format!("127.0.0.1:{port}")) {
            Ok(server) => {
                let bound = server
                    .server_addr()
                    .to_ip()
                    .map(|a| a.port())
                    .unwrap_or(port);
                let redirect_uri = format!("http://localhost:{bound}{callback_path}");
                return Ok((server, redirect_uri));
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(LoopbackBindSnafu { message: last_err }.build())
}

/// Block (on a worker thread) until the callback arrives or the deadline passes.
async fn wait_for_callback(
    server: tiny_http::Server,
    callback_path: &str,
    timeout: Duration,
) -> Result<CallbackParams> {
    let callback_path = callback_path.to_string();
    let join = tokio::task::spawn_blocking(move || -> Result<CallbackParams> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(CallbackSnafu {
                    message: "timed out waiting for the OAuth callback".to_string(),
                }
                .build());
            }
            match server.recv_timeout(Duration::from_millis(250)) {
                Ok(Some(request)) => {
                    let raw = request.url().to_string();
                    let path = raw.split('?').next().unwrap_or("");
                    if path != callback_path {
                        let _ = request.respond(
                            tiny_http::Response::from_string("Not found").with_status_code(404),
                        );
                        continue;
                    }
                    let parsed = parse_callback_query(&raw);
                    let mut response =
                        tiny_http::Response::from_string(SUCCESS_HTML).with_status_code(200);
                    if let Ok(header) =
                        "Content-Type: text/html; charset=utf-8".parse::<tiny_http::Header>()
                    {
                        response.add_header(header);
                    }
                    let _ = request.respond(response);
                    return Ok(parsed);
                }
                Ok(None) => continue, // timeout tick — re-check deadline
                Err(e) => {
                    return Err(CallbackSnafu {
                        message: format!("callback server error: {e}"),
                    }
                    .build());
                }
            }
        }
    });
    match join.await {
        Ok(res) => res,
        Err(e) => Err(InternalSnafu {
            message: format!("callback task join error: {e}"),
        }
        .build()),
    }
}

fn parse_callback_query(raw_url: &str) -> CallbackParams {
    let mut code = None;
    let mut state = None;
    let mut error = None;
    if let Ok(url) = url::Url::parse(&format!("http://localhost{raw_url}")) {
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                "error" => error = Some(v.into_owned()),
                _ => {}
            }
        }
    }
    CallbackParams { code, state, error }
}

/// Block (on a worker thread) reading one line from stdin — the paste fallback.
/// Bounded by the loopback arm's timeout in the `select!` (no internal timeout,
/// so it doesn't pre-empt a loopback callback that's about to arrive).
async fn read_pasted_callback() -> Result<CallbackParams> {
    let join = tokio::task::spawn_blocking(|| -> Result<CallbackParams> {
        use std::io::BufRead;
        eprintln!(
            "If your browser is on another machine, paste the redirected URL \
             (or the code) here and press Enter:"
        );
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).map_err(|e| {
            CallbackSnafu {
                message: format!("reading pasted callback: {e}"),
            }
            .build()
        })?;
        Ok(parse_pasted_callback(line.trim()))
    });
    match join.await {
        Ok(res) => res,
        Err(e) => Err(InternalSnafu {
            message: format!("paste task join error: {e}"),
        }
        .build()),
    }
}

/// Parse a pasted callback: a full redirect URL, a bare `code=…&state=…` query,
/// or a bare authorization code.
fn parse_pasted_callback(input: &str) -> CallbackParams {
    let pairs = |url: &url::Url| {
        let mut p = CallbackParams {
            code: None,
            state: None,
            error: None,
        };
        for (k, v) in url.query_pairs() {
            match k.as_ref() {
                "code" => p.code = Some(v.into_owned()),
                "state" => p.state = Some(v.into_owned()),
                "error" => p.error = Some(v.into_owned()),
                _ => {}
            }
        }
        p
    };
    // Full redirect URL (e.g. `http://localhost:1455/auth/callback?code=…`).
    if let Ok(url) = url::Url::parse(input) {
        let p = pairs(&url);
        if p.code.is_some() || p.error.is_some() {
            return p;
        }
    }
    // Bare query (e.g. `code=…&state=…`).
    if input.contains('=')
        && let Ok(url) = url::Url::parse(&format!("http://localhost/?{input}"))
    {
        let p = pairs(&url);
        if p.code.is_some() || p.error.is_some() {
            return p;
        }
    }
    // Bare authorization code.
    CallbackParams {
        code: Some(input.to_string()),
        state: None,
        error: None,
    }
}

const SUCCESS_HTML: &str = "<!doctype html><html><head><meta charset=utf-8>\
<title>coco — signed in</title></head><body style=\"font-family:system-ui;\
padding:3rem;text-align:center\"><h2>✓ Signed in</h2>\
<p>You can close this tab and return to your terminal.</p></body></html>";

#[cfg(test)]
#[path = "flow.test.rs"]
mod tests;
