//! Provider-neutral RFC 8628 device authorization.

use std::io::IsTerminal as _;
use std::time::Duration;

use serde::Deserialize;

use crate::descriptor::AccountIdSource;
use crate::descriptor::DeviceCodeGrant;
use crate::descriptor::OAuthFlowDescriptor;
use crate::descriptor::UserCodePolicy;
use crate::error::CallbackSnafu;
use crate::error::InternalSnafu;
use crate::error::NetworkSnafu;
use crate::error::Result;
use crate::flow::LoginOptions;
use crate::flow::LoginSurface;
use crate::refresh::TokenResponse;
use crate::refresh::expires_at_ms;
use crate::store::StoredCredential;

const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";
const DEFAULT_POLL_INTERVAL_SECS: i64 = 5;
const SLOW_DOWN_INCREMENT_SECS: i64 = 5;

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    verification_uri_complete: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    interval: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct DeviceTokenError {
    error: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollAction {
    Continue,
}

pub(crate) async fn login(
    descriptor: &OAuthFlowDescriptor,
    provider_name: &str,
    grant: DeviceCodeGrant,
    opts: &LoginOptions,
    http: &reqwest::Client,
) -> Result<StoredCredential> {
    let timeout = opts
        .timeout
        .unwrap_or_else(|| Duration::from_secs(grant.timeout_secs));
    let authorize_url = grant.effective_authorize_url();
    login_at(
        descriptor,
        provider_name,
        grant,
        DeviceEndpoints {
            authorize_url,
            token_url: descriptor.effective_token_url(),
        },
        opts,
        timeout,
        http,
    )
    .await
}

#[derive(Debug, Clone)]
struct DeviceEndpoints {
    authorize_url: String,
    token_url: String,
}

struct DeviceSession<'a> {
    descriptor: &'a OAuthFlowDescriptor,
    provider_name: &'a str,
    grant: DeviceCodeGrant,
    surface: LoginSurface,
    deadline: tokio::time::Instant,
    http: &'a reqwest::Client,
}

async fn login_at(
    descriptor: &OAuthFlowDescriptor,
    provider_name: &str,
    grant: DeviceCodeGrant,
    endpoints: DeviceEndpoints,
    opts: &LoginOptions,
    timeout: Duration,
    http: &reqwest::Client,
) -> Result<StoredCredential> {
    let surface = resolve_surface(opts);
    let caller_deadline = tokio::time::Instant::now() + timeout;
    let session = DeviceSession {
        descriptor,
        provider_name,
        grant,
        surface,
        deadline: caller_deadline,
        http,
    };
    let device = request_device_code(&session, &endpoints.authorize_url).await?;
    let display_url = display_url(&device)?;

    match &opts.on_authorize_url {
        Some(sink) => sink(display_url.clone()),
        None => {
            eprintln!(
                "\nTo sign in to {}, open this URL:\n{display_url}",
                descriptor.display_name
            );
            eprintln!(
                "\nConfirm this code in your browser:\n{}\n",
                device.user_code
            );
        }
    }
    if opts.open_browser {
        crate::flow::open_browser_detached(display_url);
    }

    let server_expiry = device
        .expires_in
        .filter(|seconds| *seconds > 0)
        .map(|seconds| tokio::time::Instant::now() + Duration::from_secs(seconds as u64));
    let deadline = server_expiry.map_or(caller_deadline, |expiry| expiry.min(caller_deadline));
    let session = DeviceSession {
        deadline,
        ..session
    };
    let tokens = poll_for_token(&session, &endpoints.token_url, &device).await?;
    let account_id = tokens
        .id_token
        .as_deref()
        .and_then(|jwt| match descriptor.account_id {
            AccountIdSource::IdTokenClaim { path } => crate::jwt::read_string_claim(jwt, path),
            AccountIdSource::UserInfoEndpoint { .. } | AccountIdSource::None => None,
        });
    let email = tokens
        .id_token
        .as_deref()
        .and_then(|jwt| crate::jwt::read_string_claim(jwt, &["email"]));
    let expires_at_ms = expires_at_ms(&tokens);
    let principal = crate::store::OAuthPrincipal::from_access_token(&tokens.access_token);

    Ok(StoredCredential {
        flow: descriptor.flow,
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        id_token: tokens.id_token,
        account_id,
        expires_at_ms,
        plan_type: None,
        email,
        principal,
        login_epoch: 0,
    })
}

fn resolve_surface(opts: &LoginOptions) -> LoginSurface {
    match opts.surface {
        LoginSurface::Auto if opts.on_authorize_url.is_some() => LoginSurface::Ui,
        LoginSurface::Auto if std::io::stderr().is_terminal() => LoginSurface::Cli,
        LoginSurface::Auto => LoginSurface::Headless,
        explicit => explicit,
    }
}

async fn request_device_code(
    session: &DeviceSession<'_>,
    authorize_url: &str,
) -> Result<DeviceCodeResponse> {
    let mut form = vec![
        ("client_id", session.descriptor.client_id),
        ("scope", session.descriptor.scope),
    ];
    form.extend(session.grant.request_extra.iter().copied());
    let builder = with_client_headers(
        session
            .http
            .post(authorize_url)
            .timeout(remaining(session.deadline)?)
            .form(&form),
        session.grant,
        session.surface,
    );
    let resp = builder.send().await.map_err(network_error)?;
    let status = resp.status();
    let body = resp.text().await.map_err(network_error)?;
    if !status.is_success() {
        return Err(endpoint_error("device authorization", status, &body));
    }
    let device: DeviceCodeResponse = serde_json::from_str(&body).map_err(|error| {
        InternalSnafu {
            message: format!("decode device-code response: {error}"),
        }
        .build()
    })?;
    if device.device_code.is_empty() {
        return Err(CallbackSnafu {
            message: "authorization server returned an empty device code".to_string(),
        }
        .build());
    }
    if device.expires_in.is_some_and(|seconds| seconds <= 0) {
        return Err(CallbackSnafu {
            message: "authorization server returned an expired device code".to_string(),
        }
        .build());
    }
    validate_user_code(&device.user_code, session.grant.user_code_policy)?;
    validate_verification_url(&device.verification_uri)?;
    if let Some(complete) = &device.verification_uri_complete {
        validate_verification_url(complete)?;
    }
    Ok(device)
}

async fn poll_for_token(
    session: &DeviceSession<'_>,
    token_url: &str,
    device: &DeviceCodeResponse,
) -> Result<TokenResponse> {
    let mut interval =
        Duration::from_secs(device.interval.unwrap_or(DEFAULT_POLL_INTERVAL_SECS).max(1) as u64);

    loop {
        tokio::time::sleep_until((tokio::time::Instant::now() + interval).min(session.deadline))
            .await;
        if tokio::time::Instant::now() >= session.deadline {
            return Err(CallbackSnafu {
                message: format!(
                    "device code expired before authorization completed; run `coco login {}` again",
                    session.provider_name
                ),
            }
            .build());
        }
        let form = [
            ("grant_type", DEVICE_GRANT_TYPE),
            ("device_code", device.device_code.as_str()),
            ("client_id", session.descriptor.client_id),
        ];
        let builder = with_client_headers(
            session
                .http
                .post(token_url)
                .timeout(remaining(session.deadline)?)
                .form(&form),
            session.grant,
            session.surface,
        );
        let resp = builder.send().await.map_err(network_error)?;
        let status = resp.status();
        let body = resp.text().await.map_err(network_error)?;
        if status.is_success() {
            return serde_json::from_str(&body).map_err(|error| {
                InternalSnafu {
                    message: format!("decode device token response: {error}"),
                }
                .build()
            });
        }
        let error: DeviceTokenError = serde_json::from_str(&body)
            .map_err(|_| endpoint_error("device token", status, &body))?;
        apply_poll_error(
            &error.error,
            session.descriptor.display_name,
            session.provider_name,
            &mut interval,
        )?;
    }
}

fn apply_poll_error(
    code: &str,
    display_name: &str,
    provider_name: &str,
    interval: &mut Duration,
) -> Result<PollAction> {
    match code {
        "authorization_pending" => Ok(PollAction::Continue),
        "slow_down" => {
            *interval += Duration::from_secs(SLOW_DOWN_INCREMENT_SECS as u64);
            Ok(PollAction::Continue)
        }
        "access_denied" => Err(CallbackSnafu {
            message: format!("{display_name} authorization was denied"),
        }
        .build()),
        "expired_token" => Err(CallbackSnafu {
            message: format!("device code expired; run `coco login {provider_name}` again"),
        }
        .build()),
        code => Err(CallbackSnafu {
            message: format!("device token exchange failed: {}", safe_error_code(code)),
        }
        .build()),
    }
}

fn with_client_headers(
    mut builder: reqwest::RequestBuilder,
    grant: DeviceCodeGrant,
    surface: LoginSurface,
) -> reqwest::RequestBuilder {
    if let Some(name) = grant.client_version_header {
        builder = builder.header(name, env!("CARGO_PKG_VERSION"));
    }
    if let Some(name) = grant.client_surface_header {
        builder = builder.header(name, surface.as_str());
    }
    builder
}

fn remaining(deadline: tokio::time::Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        return Err(CallbackSnafu {
            message: "device authorization timed out".to_string(),
        }
        .build());
    }
    Ok(remaining)
}

fn network_error(error: reqwest::Error) -> crate::error::ProviderAuthError {
    NetworkSnafu {
        message: error.to_string(),
    }
    .build()
}

fn endpoint_error(
    phase: &str,
    status: reqwest::StatusCode,
    body: &str,
) -> crate::error::ProviderAuthError {
    let code = serde_json::from_str::<DeviceTokenError>(body)
        .ok()
        .map(|error| safe_error_code(&error.error));
    CallbackSnafu {
        message: match code {
            Some(code) => format!("{phase} failed (HTTP {}): {code}", status.as_u16()),
            None => format!("{phase} failed (HTTP {})", status.as_u16()),
        },
    }
    .build()
}

fn safe_error_code(code: &str) -> String {
    let safe: String = code
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        .take(64)
        .collect();
    if safe.is_empty() {
        "unknown_error".to_string()
    } else {
        safe
    }
}

fn display_url(device: &DeviceCodeResponse) -> Result<String> {
    match &device.verification_uri_complete {
        Some(url) => {
            validate_verification_url(url)?;
            Ok(url.clone())
        }
        None => {
            validate_verification_url(&device.verification_uri)?;
            let mut url = url::Url::parse(&device.verification_uri).map_err(|_| {
                CallbackSnafu {
                    message: "authorization server returned an invalid verification URL"
                        .to_string(),
                }
                .build()
            })?;
            url.query_pairs_mut()
                .append_pair("user_code", &device.user_code);
            Ok(url.to_string())
        }
    }
}

fn validate_verification_url(raw: &str) -> Result<()> {
    if raw.chars().any(|character| character.is_ascii_control()) {
        return Err(CallbackSnafu {
            message: "authorization server returned an invalid verification URL".to_string(),
        }
        .build());
    }
    let url = url::Url::parse(raw).map_err(|_| {
        CallbackSnafu {
            message: "authorization server returned an invalid verification URL".to_string(),
        }
        .build()
    })?;
    let loopback = matches!(
        url.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("::1")
    );
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(CallbackSnafu {
            message: "authorization server returned an unsupported verification URL".to_string(),
        }
        .build());
    }
    Ok(())
}

fn validate_user_code(code: &str, policy: UserCodePolicy) -> Result<()> {
    let valid = match policy {
        UserCodePolicy::AsciiAlphanumericDash => {
            !code.is_empty()
                && code
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        }
    };
    if !valid {
        return Err(CallbackSnafu {
            message: "authorization server returned an invalid user code".to_string(),
        }
        .build());
    }
    Ok(())
}

impl LoginSurface {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Cli => "cli",
            Self::Headless | Self::Auto => "headless",
        }
    }
}

#[cfg(test)]
#[path = "device.test.rs"]
mod tests;
