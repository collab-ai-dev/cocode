//! Custom-header template expansion.
//!
//! A `ProviderClientOptions.headers` value can be a literal string or a
//! `{ "template": "..." }` object (see `coco_config::HeaderValue`). Templated
//! values are expanded **here**, at provider-build time, against a set of
//! built-in variables.
//!
//! ## Syntax
//!
//! - `${NAME}` — a variable. Braces are **mandatory**.
//! - `$` not followed by `{` (or `$`) is a literal `$`. This makes a JSON
//!   document safe as a header value: `{"sid":"${SESSION_ID}"}` needs no
//!   escaping of its own `{`/`}`/`"`, and a JSON key like `"$ref"` is left
//!   untouched.
//! - `$$` is the escape for a literal `$` (use `$${X}` to emit a literal
//!   `${X}`).
//!
//! ## Variable scope
//!
//! All built-ins are **session-stable** (computable once at client-build time);
//! the design deliberately excludes per-request values (timestamps, nonces)
//! because the provider header map is a static snapshot captured at client
//! construction. Two value sources:
//!
//! - [`PerBuildVars`] — `(provider, model, api, base_url, account_kind)`, taken
//!   from `RuntimeConfig` + `ModelSpec` already in scope in `model_factory`.
//! - [`HeaderVars`] — the genuinely session-scoped values (`session_id`, `cwd`,
//!   `app_version`) that must be threaded from the app layer into
//!   `ModelRuntimeRegistry`. When absent (`None`, e.g. unit tests / prebuilt
//!   registries) these resolve to the empty string rather than erroring.
//!
//! `${OS}` / `${ARCH}` are compile-time constants; `${ENV:NAME}` reads an
//! arbitrary process env var (covers e.g. `${ENV:HOSTNAME}`).

use std::fmt;

/// Session-scoped variable values, constructed once per session in the app
/// layer and shared (`Arc`) by every `ModelRuntimeRegistry` for that session.
#[derive(Debug, Clone, Default)]
pub struct HeaderVars {
    /// `${SESSION_ID}` — current session id.
    pub session_id: String,
    /// `${CWD}` — session working directory.
    pub cwd: String,
    /// `${APP_VERSION}` — coco binary version (`CARGO_PKG_VERSION`).
    pub app_version: String,
}

impl HeaderVars {
    /// All-empty context — session vars resolve to `""`. Used by prebuilt
    /// registries and tests that don't customize headers.
    pub fn empty() -> Self {
        Self::default()
    }
}

/// Per-(provider, model) variable values, available without app-layer
/// threading because they live on `RuntimeConfig` / `ModelSpec`.
#[derive(Debug, Clone)]
pub struct PerBuildVars {
    /// `${PROVIDER}` — provider instance name (`ProviderConfig.name`).
    pub provider: String,
    /// `${MODEL_ID}` — wire model name (after `api_model_name` override).
    pub model_id: String,
    /// `${API}` — provider api family (e.g. `anthropic`, `openai`).
    pub api: &'static str,
    /// `${BASE_URL}` — provider base URL.
    pub base_url: String,
    /// `${ACCOUNT_KIND}` — `api_key` or `subscriber`.
    pub account_kind: &'static str,
}

/// A header template that could not be expanded. Converted to
/// `InferenceError::ProviderBuildFailed` at the `model_factory` boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// `${...}` referencing a name not in the built-in catalog (likely a typo).
    UnknownVariable { name: String },
    /// A `${` with no closing `}`.
    UnterminatedPlaceholder,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownVariable { name } => write!(
                f,
                "unknown variable `${{{name}}}` (use $$ for a literal `$`)"
            ),
            Self::UnterminatedPlaceholder => {
                write!(f, "unterminated `${{` — missing closing `}}`")
            }
        }
    }
}

impl std::error::Error for TemplateError {}

/// Expand a header template against the built-in variables.
///
/// `vars` is `None` when no session context is available; session-scoped
/// names then resolve to the empty string (logged at `debug`). An unknown
/// variable name is a hard error — it almost always means a config typo.
pub fn expand(
    template: &str,
    vars: Option<&HeaderVars>,
    per_build: &PerBuildVars,
) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // `$$` → literal `$`.
            Some('$') => {
                chars.next();
                out.push('$');
            }
            // `${NAME}` → variable.
            Some('{') => {
                chars.next(); // consume '{'
                let mut name = String::new();
                let mut closed = false;
                for nc in chars.by_ref() {
                    if nc == '}' {
                        closed = true;
                        break;
                    }
                    name.push(nc);
                }
                if !closed {
                    return Err(TemplateError::UnterminatedPlaceholder);
                }
                match resolve(&name, vars, per_build) {
                    Some(value) => out.push_str(&value),
                    None => return Err(TemplateError::UnknownVariable { name }),
                }
            }
            // Lone `$` (incl. end-of-string) → literal.
            _ => out.push('$'),
        }
    }
    Ok(sanitize(out))
}

/// Resolve a single variable name. `None` ⇒ unknown name (caller errors).
fn resolve(name: &str, vars: Option<&HeaderVars>, per_build: &PerBuildVars) -> Option<String> {
    // Parametric: `${ENV:NAME}` reads an arbitrary process env var.
    if let Some(env_name) = name.strip_prefix("ENV:") {
        return Some(std::env::var(env_name).unwrap_or_default());
    }
    let value = match name {
        // Per-build (always available).
        "PROVIDER" => per_build.provider.clone(),
        "MODEL_ID" => per_build.model_id.clone(),
        "API" => per_build.api.to_string(),
        "BASE_URL" => per_build.base_url.clone(),
        "ACCOUNT_KIND" => per_build.account_kind.to_string(),
        // Process-level constants.
        "OS" => std::env::consts::OS.to_string(),
        "ARCH" => std::env::consts::ARCH.to_string(),
        // Session-scoped — empty when no session context threaded in.
        "SESSION_ID" => session_var(vars, name, |v| &v.session_id),
        "CWD" => session_var(vars, name, |v| &v.cwd),
        "APP_VERSION" => session_var(vars, name, |v| &v.app_version),
        _ => return None,
    };
    Some(value)
}

/// Read a session-scoped field, defaulting to `""` (with a `debug` line) when
/// no `HeaderVars` was supplied.
fn session_var(
    vars: Option<&HeaderVars>,
    name: &str,
    pick: impl Fn(&HeaderVars) -> &str,
) -> String {
    match vars {
        Some(v) => pick(v).to_string(),
        None => {
            tracing::debug!(
                variable = name,
                "header template: no session context; resolving to \"\""
            );
            String::new()
        }
    }
}

/// Strip CR/LF — header values can never contain them, and an expanded
/// `${ENV:NAME}` could otherwise smuggle a header-injection newline.
fn sanitize(value: String) -> String {
    if value.contains(['\r', '\n']) {
        tracing::warn!("header template: stripped CR/LF from expanded value");
        return value.chars().filter(|c| *c != '\r' && *c != '\n').collect();
    }
    value
}

#[cfg(test)]
#[path = "header_template.test.rs"]
mod tests;
