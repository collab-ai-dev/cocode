use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::SideQueryRequest;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// Typed input for [`WebFetchTool`]. Manual `input_schema()` is the
/// model-facing source of truth — this struct is the boundary
/// deserialiser.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WebFetchInput {
    /// The URL to fetch content from. Required — the runtime schema declares
    /// `required: ["url", "prompt"]`.
    pub url: String,
    /// The prompt to run on the fetched content. Required (see `url`).
    pub prompt: String,
}

/// Typed input for [`WebSearchTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WebSearchInput {
    /// The search query. Required — the runtime schema declares
    /// `required: ["query"]` with `minLength: 2`.
    pub query: String,
    /// Maximum number of results to return. Clamped to
    /// `[1, SEARCH_MAX_RESULTS_CEILING]`.
    #[serde(default)]
    pub max_results: Option<i64>,
    /// Only include results from these domains (client-side filter).
    #[serde(default)]
    pub allowed_domains: Option<Vec<String>>,
    /// Never include results from these domains (client-side filter).
    #[serde(default)]
    pub blocked_domains: Option<Vec<String>>,
}

// Max-fetch-length (100K chars), fetch timeout (60s), and user-agent
// now live on `coco_config::WebFetchConfig` — consumed from
// `ctx.web_fetch_config` in [`WebFetchTool::execute`]. The crate-local
// constants below are extraction-pipeline invariants that aren't
// user-configurable (HTTP body byte cap, html2text line width, the
// extraction prompt, etc.).

/// Max width in columns for html2text's line wrapping.
///
/// Set to 120 so the extracted markdown renders consistently.
const HTML2TEXT_LINE_WIDTH: usize = 120;

/// System prompt for the WebFetch extraction side-query.
///
/// The secondary model receives the markdown body + user's question and
/// extracts the relevant answer.
const WEB_FETCH_EXTRACT_SYSTEM: &str = "\
You are a helpful assistant extracting answers from web page content. \
The user will provide a web page (in markdown format) followed by a \
specific prompt. Your job is to answer the prompt using ONLY information \
from the provided content. \
If the content does not contain enough information to answer the prompt, \
say so clearly rather than guessing. Be concise.";

/// Model-facing WebFetch tool description body. Includes the MCP-
/// preference hint, the 15-minute cache note, and the cross-origin
/// redirect handling guidance — all of which inform model behavior when
/// the fetch encounters edge cases. Surfaced to the model via
/// [`WebFetchTool::prompt`] (prepended with the auth-warning prefix).
const WEB_FETCH_DESCRIPTION: &str = "
- Fetches content from a specified URL and processes it using an AI model
- Takes a URL and a prompt as input
- Fetches the URL content, converts HTML to markdown
- Processes the content with the prompt using a small, fast model
- Returns the model's response about the content
- Use this tool when you need to retrieve and analyze web content

Usage notes:
  - IMPORTANT: If an MCP-provided web fetch tool is available, prefer using that tool instead of this one, as it may have fewer restrictions.
  - The URL must be a fully-formed valid URL
  - HTTP URLs will be automatically upgraded to HTTPS
  - The prompt should describe what information you want to extract from the page
  - This tool is read-only and does not modify any files
  - Results may be summarized if the content is very large
  - Includes a self-cleaning 15-minute cache for faster responses when repeatedly accessing the same URL
  - When a URL redirects to a different host, the tool will inform you and provide the redirect URL in a special format. You should then make a new WebFetch request with the redirect URL to fetch the content.
  - For GitHub URLs, prefer using the gh CLI via Bash instead (e.g., gh pr view, gh issue view, gh api).
";

/// Response guidelines appended to the extraction prompt, selected by
/// whether the fetched URL is a preapproved documentation host.
///
/// Preapproved docs get relaxed guidance (include code examples /
/// excerpts); everything else gets the strict copyright/quoting rules.
fn extract_guidelines(is_preapproved: bool) -> &'static str {
    if is_preapproved {
        "Provide a concise response based on the content above. Include relevant \
         details, code examples, and documentation excerpts as needed."
    } else {
        "Provide a concise response based only on the content above. In your response:\n\
         - Enforce a strict 125-character maximum for quotes from any source document. \
         Open Source Software is ok as long as we respect the license.\n\
         - Use quotation marks for exact language from articles; any language outside \
         of the quotation should never be word-for-word the same.\n\
         - You are not a lawyer and never comment on the legality of your own prompts \
         and responses.\n\
         - Never produce or reproduce exact song lyrics."
    }
}

/// Max HTTP response body size: 10 MB. Prevents a misbehaving server
/// from flooding memory via a huge response.
const MAX_HTTP_CONTENT_LENGTH: u64 = 10 * 1024 * 1024;

/// WebFetch URL cache TTL: 15 minutes.
const WEB_FETCH_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// WebFetch URL cache max entries. Uses entry count rather than byte
/// budget: 128 entries at up to 100K chars each gives a ~12.8MB upper
/// bound on cache content.
const WEB_FETCH_CACHE_MAX_ENTRIES: usize = 128;

/// Cached WebFetch entry. Stores the extracted markdown + its freshness
/// timestamp.
#[derive(Clone)]
struct CachedWebFetch {
    /// Extracted markdown (after turndown conversion).
    markdown: String,
    /// Content-Type from the upstream response.
    content_type: String,
    /// Whether the markdown was truncated at `web_fetch.max_content_length`.
    was_truncated: bool,
    /// When the entry was inserted — used for TTL check.
    inserted_at: std::time::Instant,
}

/// In-process WebFetch URL cache. Parallel to the `SEARCH_CACHE` used
/// by WebSearch — same `LazyLock<Mutex<Vec<...>>>` LRU pattern.
/// Session-scoped only.
static WEB_FETCH_CACHE: std::sync::LazyLock<std::sync::Mutex<Vec<(String, CachedWebFetch)>>> =
    std::sync::LazyLock::new(|| {
        std::sync::Mutex::new(Vec::with_capacity(WEB_FETCH_CACHE_MAX_ENTRIES))
    });

fn web_fetch_cache_get(url: &str) -> Option<CachedWebFetch> {
    let mut cache = WEB_FETCH_CACHE.lock().ok()?;
    // Expire old entries on every access (cheap since the cache is small).
    let now = std::time::Instant::now();
    cache.retain(|(_, entry)| now.duration_since(entry.inserted_at) < WEB_FETCH_CACHE_TTL);
    cache
        .iter()
        .find_map(|(k, v)| if k == url { Some(v.clone()) } else { None })
}

fn web_fetch_cache_set(url: String, entry: CachedWebFetch) {
    if let Ok(mut cache) = WEB_FETCH_CACHE.lock() {
        // LRU eviction: if at capacity, drop the oldest entry.
        if cache.len() >= WEB_FETCH_CACHE_MAX_ENTRIES {
            cache.remove(0);
        }
        // Deduplicate: if an entry for the same URL exists, remove it
        // so the refreshed entry is the newest.
        cache.retain(|(k, _)| k != &url);
        cache.push((url, entry));
    }
}

/// Test-only cache reset. Isolates cached fixtures between tests.
#[cfg(test)]
pub(super) fn clear_web_fetch_cache() {
    if let Ok(mut cache) = WEB_FETCH_CACHE.lock() {
        cache.clear();
    }
}

// Custom User-Agent header.
//
// The `Claude-User` prefix is Anthropic's publicly documented agent for
// user-initiated fetches — site operators match this in robots.txt to
// distinguish CLI traffic from server-side automation.
//
// User agent now lives on `WebFetchConfig::user_agent` (default matches
// Callers can override via settings.json when they need a different
// robots.txt contract.

/// Preapproved hosts — URLs whose (hostname, pathname) match this list
/// skip the optional permission gate.
///
/// **Matching semantics** (stricter than a suffix match):
///
/// - Entries without `/` are **exact hostname match only**. `docs.python.org`
///   does NOT match `subdomain.docs.python.org` — we only allow the entry
///   itself. This is intentional: some entries (e.g. `nuget.org`,
///   `huggingface.co`, `www.kaggle.com`) allow user uploads, and allowing
///   arbitrary subdomains could enable data exfiltration to attacker-
///   controlled subdomains.
///
/// - Entries containing `/` are **host + path-prefix** entries. For example
///   `github.com/anthropics` matches `https://github.com/anthropics` and
///   `https://github.com/anthropics/claude-code` but NOT
///   `https://github.com/anthropics-evil/malware`. Segment boundary is
///   enforced (exact match or `prefix + /`).
///
/// These entries are for WebFetch GET only — the sandbox does NOT inherit
/// this list for network restrictions.
///
/// Consulted by [`WebFetchTool::check_permissions`] (approval bypass), the
/// `execute` verbatim-passthrough branch, and the extraction-prompt
/// guideline selector.
const PREAPPROVED_WEB_HOSTS: &[&str] = &[
    // Anthropic (5)
    "platform.claude.com",
    "code.claude.com",
    "modelcontextprotocol.io",
    "github.com/anthropics",
    "agentskills.io",
    // Top Programming Languages (13)
    "docs.python.org",
    "en.cppreference.com",
    "docs.oracle.com",
    "learn.microsoft.com",
    "developer.mozilla.org",
    "go.dev",
    "pkg.go.dev",
    "www.php.net",
    "docs.swift.org",
    "kotlinlang.org",
    "ruby-doc.org",
    "doc.rust-lang.org",
    "www.typescriptlang.org",
    // Web & JavaScript Frameworks/Libraries (16)
    "react.dev",
    "angular.io",
    "vuejs.org",
    "nextjs.org",
    "expressjs.com",
    "nodejs.org",
    "bun.sh",
    "jquery.com",
    "getbootstrap.com",
    "tailwindcss.com",
    "d3js.org",
    "threejs.org",
    "redux.js.org",
    "webpack.js.org",
    "jestjs.io",
    "reactrouter.com",
    // Python Frameworks & Libraries (11)
    "docs.djangoproject.com",
    "flask.palletsprojects.com",
    "fastapi.tiangolo.com",
    "pandas.pydata.org",
    "numpy.org",
    "www.tensorflow.org",
    "pytorch.org",
    "scikit-learn.org",
    "matplotlib.org",
    "requests.readthedocs.io",
    "jupyter.org",
    // PHP Frameworks (3)
    "laravel.com",
    "symfony.com",
    "wordpress.org",
    // Java Frameworks & Libraries (5)
    "docs.spring.io",
    "hibernate.org",
    "tomcat.apache.org",
    "gradle.org",
    "maven.apache.org",
    // .NET & C# Frameworks (4)
    "asp.net",
    "dotnet.microsoft.com",
    "nuget.org",
    "blazor.net",
    // Mobile Development (4)
    "reactnative.dev",
    "docs.flutter.dev",
    "developer.apple.com",
    "developer.android.com",
    // Data Science & Machine Learning (4)
    "keras.io",
    "spark.apache.org",
    "huggingface.co",
    "www.kaggle.com",
    // Databases (7)
    "www.mongodb.com",
    "redis.io",
    "www.postgresql.org",
    "dev.mysql.com",
    "www.sqlite.org",
    "graphql.org",
    "prisma.io",
    // Cloud & DevOps (10)
    "docs.aws.amazon.com",
    "cloud.google.com",
    "kubernetes.io",
    "www.docker.com",
    "www.terraform.io",
    "www.ansible.com",
    "vercel.com/docs",
    "docs.netlify.com",
    "devcenter.heroku.com",
    // Testing & Monitoring (2)
    "cypress.io",
    "selenium.dev",
    // Game Development (2)
    "docs.unity.com",
    "docs.unrealengine.com",
    // Other Essential Tools (3)
    "git-scm.com",
    "nginx.org",
    "httpd.apache.org",
];

/// Check whether a `(hostname, pathname)` pair matches the preapproved
/// list. Uses exact hostname match plus path-prefix matching with segment
/// boundary enforcement.
///
/// `hostname` should be the URL's host (lowercased), `pathname` should
/// be the path portion starting with `/` (or empty for root).
pub(super) fn is_preapproved_host(hostname: &str, pathname: &str) -> bool {
    if hostname.is_empty() {
        return false;
    }
    let host_lower = hostname.to_lowercase();
    for entry in PREAPPROVED_WEB_HOSTS {
        match entry.split_once('/') {
            None => {
                // Hostname-only entry: exact match required.
                if host_lower == *entry {
                    return true;
                }
            }
            Some((host, path_rest)) => {
                // Path-scoped entry: host exact, path with segment boundary.
                if host_lower != host {
                    continue;
                }
                let path_prefix = format!("/{path_rest}");
                // Segment boundary: exact match OR prefix followed by '/'.
                // Prevents "/anthropics-evil/malware" from matching
                // "/anthropics".
                if pathname == path_prefix || pathname.starts_with(&format!("{path_prefix}/")) {
                    return true;
                }
            }
        }
    }
    false
}

/// Convenience wrapper that extracts hostname + pathname from a full
/// URL and calls [`is_preapproved_host`]. Consulted by the permission
/// bypass, the verbatim-passthrough branch, and the extraction-prompt
/// guideline selector.
pub(super) fn is_preapproved_url(url: &str) -> bool {
    let host = extract_host(url);
    let pathname = extract_pathname(url);
    is_preapproved_host(&host, &pathname)
}

/// True if `pattern` targets the WebFetch tool (exact name or `*` wildcard).
fn web_fetch_tool_pattern(pattern: &str) -> bool {
    pattern == ToolName::WebFetch.as_str() || pattern == "*"
}

/// Find a WebFetch rule that applies to `host`: a tool-wide `WebFetch` rule
/// (no content) or one scoped to `domain:<host>`.
fn matching_web_fetch_rule<'a>(
    rules: &'a coco_types::PermissionRulesBySource,
    host: &str,
) -> Option<&'a coco_types::PermissionRule> {
    let domain_rule = format!("domain:{host}");
    rules.values().flatten().find(|r| {
        web_fetch_tool_pattern(&r.value.tool_pattern)
            && match r.value.rule_content.as_deref() {
                None => true,
                Some(content) => content == domain_rule,
            }
    })
}

/// "Always allow this domain" suggestion attached to a WebFetch `Ask`:
/// an `addRules` allow for `domain:<host>`.
fn web_fetch_domain_suggestions(host: &str) -> Vec<coco_types::PermissionUpdate> {
    vec![coco_types::PermissionUpdate::AddRules {
        rules: vec![coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::Session,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: ToolName::WebFetch.as_str().to_string(),
                rule_content: Some(format!("domain:{host}")),
            },
        }],
        destination: coco_types::PermissionUpdateDestination::Session,
    }]
}

/// Extract the pathname portion of a URL (everything from the first `/`
/// after the host, up to `?` or `#`). Returns `"/"` when the URL has no
/// explicit path (bare host).
fn extract_pathname(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Strip userinfo if present (user:pass@host).
    let after_userinfo = without_scheme
        .split_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(without_scheme);
    // Find the path start.
    match after_userinfo.find('/') {
        Some(start) => {
            // Stop at query/fragment.
            let path = &after_userinfo[start..];
            let end = path.find(['?', '#']).unwrap_or(path.len());
            path[..end].to_string()
        }
        None => "/".to_string(),
    }
}

/// Redirect handling decision for the cross-origin SSRF guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RedirectDecision {
    /// No redirect (or same-origin redirect we can auto-follow).
    Allow,
    /// Cross-origin redirect — the model should re-fetch explicitly with
    /// the new URL. Prevents open-redirect exploitation of the fetcher.
    CrossOrigin { new_url: String },
}

/// Decide whether a redirect from `from_url` to `to_url` is safe to
/// auto-follow. A redirect is only permitted when ALL FOUR checks pass:
///
/// 1. **Protocol match** — `https://...` → `http://...` is blocked.
///    Otherwise an attacker could downgrade a TLS-secured fetch to
///    plaintext and MITM it.
///
/// 2. **Port match** — `example.com:443` → `example.com:9999` is blocked.
///    Without this check, a malicious server could redirect to a non-default
///    port on the same host and coco-rs would follow it blindly.
///
/// 3. **No redirect userinfo** — `https://user:pass@example.com/`
///    shouldn't appear in a Location header; if it does, the server is
///    attempting credential injection.
///
/// 4. **Host equivalence** (after stripping `www.`) — allows
///    `example.com` ↔ `www.example.com` toggling, blocks subdomain
///    redirects like `example.com` → `docs.example.com`.
///
/// Any violation returns `CrossOrigin { new_url }` so the tool surfaces
/// a structured message asking the model to re-fetch with the new URL.
/// Automatic redirect following is intentionally disabled to prevent
/// open-redirect exploitation of the fetcher.
pub(super) fn check_redirect(from_url: &str, to_url: &str) -> RedirectDecision {
    let reject = || RedirectDecision::CrossOrigin {
        new_url: to_url.to_string(),
    };

    // Check 1: protocol must match. Extract scheme from both URLs.
    let from_scheme = extract_scheme(from_url);
    let to_scheme = extract_scheme(to_url);
    if from_scheme != to_scheme || from_scheme.is_empty() {
        return reject();
    }

    // Check 2: port must match. Compare normalized host:port tuples.
    let (from_host, from_port) = split_host_port(from_url, &from_scheme);
    let (to_host, to_port) = split_host_port(to_url, &to_scheme);
    if from_port != to_port {
        return reject();
    }

    // Check 3: redirect target must not carry userinfo (`user:pass@host`).
    // This catches credential-injection attempts via the Location header.
    if has_userinfo(to_url) {
        return reject();
    }

    // Check 4: host equivalence after stripping `www.`.
    if from_host.is_empty() || to_host.is_empty() {
        return reject();
    }
    let from_lower = from_host.to_lowercase();
    let to_lower = to_host.to_lowercase();
    let normalize = |h: &str| -> String { h.strip_prefix("www.").unwrap_or(h).to_string() };
    if normalize(&from_lower) == normalize(&to_lower) {
        return RedirectDecision::Allow;
    }

    reject()
}

/// Extract the scheme (e.g. `"https"`) from a URL. Empty string if
/// there's no explicit `scheme://` prefix.
pub(super) fn extract_scheme(url: &str) -> String {
    url.split_once("://")
        .map(|(scheme, _)| scheme.to_lowercase())
        .unwrap_or_default()
}

/// Split a URL into its host and explicit port (if any).
///
/// Returns `(host, port)` where port is `None` when not specified.
/// Port is derived from the URL text — we do NOT fall back to the
/// scheme's default (443/80), because two URLs `https://example.com`
/// and `https://example.com:443` should compare equal. We achieve this
/// by normalizing both to `None`.
///
/// # IPv6 literal handling (T1 fix)
///
/// Per RFC 3986, IPv6 addresses in URLs MUST be enclosed in brackets:
/// `http://[::1]:8080/`. The brackets are part of the host, and any
/// port comes AFTER the closing bracket. A naive `rsplit_once(':')`
/// would split on a colon inside the IPv6 address, producing a
/// malformed host like `[::` and a bogus port.
///
/// This function detects bracketed IPv6 explicitly:
/// - `[::1]` → host=`[::1]`, port=None
/// - `[::1]:8080` → host=`[::1]`, port=Some(8080)
/// - `[::1]:443` under `https` → host=`[::1]`, port=None (default)
///
/// Bare (unbracketed) IPv6 like `::1` is technically not a valid URL
/// host — we'll still try to parse it with the standard rsplit path
/// which will produce wrong results, but the input was malformed to
/// begin with.
///
/// # Port parse failure (T1 fix)
///
/// When the trailing `:N` suffix is all digits but fails to parse as
/// `u16` (e.g. `:99999`, over the 65535 cap), we strip the bad port
/// and return `port=None`. Previously we returned the whole
/// `host:99999` string as the host, causing silent corruption and
/// false host-comparison results.
pub(super) fn split_host_port(url: &str, scheme: &str) -> (String, Option<u16>) {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Strip any userinfo segment before we look for host:port.
    let after_userinfo = without_scheme
        .split_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(without_scheme);
    // Host portion stops at the first `/`, `?`, or `#`.
    let host_end = after_userinfo
        .find(['/', '?', '#'])
        .unwrap_or(after_userinfo.len());
    let host_with_port = &after_userinfo[..host_end];

    // Bracketed IPv6 literal fast path. The host is `[...]` (including
    // brackets) and any port comes after `]:`.
    let (raw_host, explicit_port) = if host_with_port.starts_with('[') {
        if let Some(bracket_end) = host_with_port.find(']') {
            // Host is everything up to and including the `]`.
            let host = &host_with_port[..=bracket_end];
            let after_bracket = &host_with_port[bracket_end + 1..];
            let port = if let Some(port_str) = after_bracket.strip_prefix(':') {
                // T1: on parse failure (e.g. port > 65535), return None
                // instead of keeping the bad suffix attached to the host.
                port_str.parse::<u16>().ok()
            } else {
                // No `:port` after the bracket — just the bracketed
                // host. Anything else after the bracket that isn't a
                // colon is malformed; we ignore it.
                None
            };
            (host, port)
        } else {
            // Unterminated `[` — malformed URL, pass through as-is.
            (host_with_port, None)
        }
    } else {
        // Non-bracketed host. Split on the LAST `:` for the port. We
        // only accept the split as a port if the text after `:` is
        // all ASCII digits — this prevents misinterpretation in bare
        // IPv6 or other colon-containing junk.
        match host_with_port.rsplit_once(':') {
            Some((h, port_str))
                if !port_str.is_empty() && port_str.bytes().all(|b| b.is_ascii_digit()) =>
            {
                // T1: if the all-digit suffix fails to parse as u16
                // (e.g. `:99999`, above the 65535 cap), the host is
                // still stripped back to `h` and port is None — NOT
                // host=`h:99999`. This avoids silent corruption.
                (h, port_str.parse::<u16>().ok())
            }
            // No `:port` suffix OR the text after the last `:` isn't
            // all digits (e.g. a `::` inside a bare IPv6 literal).
            Some(_) | None => (host_with_port, None),
        }
    };

    // Normalize explicit default-port to None so that `example.com` and
    // `example.com:443` under `https` compare equal.
    let normalized_port = match (scheme, explicit_port) {
        ("https", Some(443)) => None,
        ("http", Some(80)) => None,
        other => other.1,
    };

    (raw_host.to_string(), normalized_port)
}

/// Check whether a URL contains a userinfo segment like `user:pass@`.
/// Redirects with userinfo are rejected to prevent credential-injection
/// attacks via the Location header.
pub(super) fn has_userinfo(url: &str) -> bool {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    // Userinfo, if present, comes before the first `@` and before the
    // host path boundary. `@` after a `/` is in the path, not userinfo.
    let boundary = without_scheme
        .find(['/', '?', '#'])
        .unwrap_or(without_scheme.len());
    let authority = &without_scheme[..boundary];
    authority.contains('@')
}

pub struct WebFetchTool;

#[async_trait::async_trait]
impl Tool for WebFetchTool {
    type Input = WebFetchInput;
    // Static schema from a literal `json!`; a parse failure means the literal
    // is malformed (a programmer error), so panicking on first build is correct.
    #[allow(clippy::expect_used)]
    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        static SCHEMA: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
            std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            coco_tool_runtime::ToolInputSchema::from_static_value(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "url": {"type": "string", "description": "The URL to fetch content from"},
                    "prompt": {"type": "string", "description": "The prompt to run on the fetched content"}
                },
                // Both keys are mandatory.
                "required": ["url", "prompt"]
            }))
        })
    }
    /// Multi-shape output (cached/fresh extraction, cross-origin
    /// redirect envelope, raw markdown fallback) — keep `Value` as the
    /// escape hatch (see `BashTool` for the same rationale).
    type Output = serde_json::Value;

    fn to_auto_classifier_input(&self, input: &WebFetchInput) -> Option<String> {
        // The fetch prompt can carry injected extraction instructions, so the
        // gate sees it when present.
        Some(if input.prompt.is_empty() {
            input.url.clone()
        } else {
            format!("{}: {}", input.url, input.prompt)
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::WebFetch)
    }
    fn name(&self) -> &str {
        ToolName::WebFetch.as_str()
    }
    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::WebFetch)
    }
    /// Short UI label: `Claude wants to fetch content from ${hostname}`, or
    /// a generic fallback when the URL can't be parsed. The long model-
    /// facing guidance lives in [`Self::prompt`].
    fn description(&self, input: &WebFetchInput, _options: &DescriptionOptions) -> String {
        let host = extract_host(&input.url);
        if host.is_empty() {
            "Claude wants to fetch content from this URL".into()
        } else {
            format!("Claude wants to fetch content from {host}")
        }
    }
    /// Model-facing tool description. Always prepends the authenticated/private
    /// URL warning to `DESCRIPTION` unconditionally to avoid prompt-cache
    /// invalidation from ToolSearch flicker.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        format!(
            "IMPORTANT: WebFetch WILL FAIL for authenticated or private URLs. Before using this \
             tool, check if the URL points to an authenticated service (e.g. Google Docs, \
             Confluence, Jira, GitHub). If so, look for a specialized MCP tool that provides \
             authenticated access.\n{WEB_FETCH_DESCRIPTION}"
        )
    }
    fn is_read_only(&self, _input: &WebFetchInput) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &WebFetchInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("fetch and extract content from a URL")
    }

    /// Per-domain matcher so persisted `domain:<host>` rules apply to the
    /// right host.
    fn prepare_permission_matcher(&self, input: &WebFetchInput) -> String {
        let host = extract_host(&input.url);
        if host.is_empty() {
            self.name().to_string()
        } else {
            format!("domain:{host}")
        }
    }

    /// Permission check: preapproved documentation hosts skip the dialog
    /// (`Allow`). Otherwise the request is gated **per domain** — `deny` →
    /// `ask` → `allow` rules keyed on `domain:<host>` (plus any tool-wide
    /// `WebFetch` rule), then a default `Ask` carrying an "always allow this
    /// domain" suggestion so the user can persist per-domain trust instead of
    /// an over-broad tool-wide allow. A malformed URL (empty host) falls
    /// through to `Passthrough`.
    async fn check_permissions(
        &self,
        input: &WebFetchInput,
        ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        if is_preapproved_url(&input.url) {
            return ToolCheckResult::Allow {
                updated_input: None,
                feedback: Some("Preapproved host".into()),
            };
        }
        let host = extract_host(&input.url);
        if host.is_empty() {
            return ToolCheckResult::Passthrough;
        }
        let pc = &ctx.permission_context;
        if matching_web_fetch_rule(&pc.deny_rules, &host).is_some() {
            return ToolCheckResult::Deny {
                message: format!("WebFetch denied access to domain:{host}."),
            };
        }
        if matching_web_fetch_rule(&pc.ask_rules, &host).is_some() {
            return ToolCheckResult::Ask {
                message: format!("Allow WebFetch to access {host}?"),
                suggestions: web_fetch_domain_suggestions(&host),
                choices: None,
                detail: None,
            };
        }
        if matching_web_fetch_rule(&pc.allow_rules, &host).is_some() {
            return ToolCheckResult::Allow {
                updated_input: None,
                feedback: None,
            };
        }
        ToolCheckResult::Ask {
            message: format!("Allow WebFetch to access {host}?"),
            suggestions: web_fetch_domain_suggestions(&host),
            choices: None,
            detail: None,
        }
    }

    fn get_activity_description(&self, input: &WebFetchInput) -> Option<String> {
        if input.url.is_empty() {
            return None;
        }
        let url = input.url.as_str();
        let truncated: String = url.chars().take(47).collect();
        let display = if truncated.len() < url.len() {
            format!("Fetching {truncated}...")
        } else {
            format!("Fetching {url}")
        };
        Some(display)
    }

    /// Pick the user-facing payload from the structured `data` envelope:
    /// - `extracted` (LLM extraction succeeded): the model's answer
    /// - `content` (raw fallback when LLM extraction failed): the
    ///   original markdown body
    /// - `message` (cross-origin redirect blocked): the SSRF guard
    ///   suggesting the model issue a fresh fetch with the new URL
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let text = data
            .get("extracted")
            .or_else(|| data.get("content"))
            .or_else(|| data.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string(data).unwrap_or_default());
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: WebFetchInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let url = input.url.trim();

        if url.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "url parameter is required".into(),
                error_code: None,
            });
        }

        // Validate up-front so a typo like "https//example.com" (missing the
        // colon) fails clearly and synchronously rather than surfacing as a
        // confusing `[NETWORK_ERROR]`. Also enforces a scheme — `file://` and
        // `data://` are caught here because they're not http/https.
        let parsed = reqwest::Url::parse(url).map_err(|e| ToolError::InvalidInput {
            message: format!("invalid url '{url}': {e}"),
            error_code: None,
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ToolError::InvalidInput {
                message: format!(
                    "url must use http or https scheme, got '{}://'",
                    parsed.scheme()
                ),
                error_code: None,
            });
        }

        // Upgrade `http://` to `https://` before fetching (the description
        // advertises this). The cache key and display still use the original `url`.
        let fetch_target: String = if parsed.scheme() == "http" {
            let mut upgraded = parsed.clone();
            let _ = upgraded.set_scheme("https");
            upgraded.to_string()
        } else {
            url.to_string()
        };

        let prompt = input.prompt.trim();

        if prompt.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "prompt parameter is required".into(),
                error_code: None,
            });
        }

        // Resolve the per-fetch config once so Stage 1 + Stage 3 see
        // the same values.
        let fetch_config = &ctx.web_fetch_config;
        let max_fetch_len = fetch_config.max_content_length.max(0) as usize;

        // Stage 0: cache lookup. Short-circuits if a fresh entry exists,
        // preventing redundant network + LLM extraction work when the model
        // re-fetches the same URL within 15 minutes (e.g. revisiting a docs
        // page after a tool-use loop).
        //
        // Note: the cache key is the ORIGINAL url parameter, not the
        // post-redirect URL.
        let (extraction_input, was_truncated, content_type, binary_note) = if let Some(cached) =
            web_fetch_cache_get(url)
        {
            // Cache hit: skip Stage 1+2+3 and use the stored markdown.
            tracing::debug!("WebFetch cache hit for {url}");
            (
                cached.markdown,
                cached.was_truncated,
                cached.content_type,
                None,
            )
        } else {
            // Cache miss: run the full fetch → markdown → truncate pipeline.
            //
            // Stage 1: HTTP fetch. `fetch_url` returns one of:
            //   - `Body { body, content_type }` — normal 2xx response
            //   - `CrossOriginRedirect { new_url }` — the origin
            //     redirected somewhere outside the same-origin
            //     allowlist; we surface this to the model so it can
            //     decide whether to fetch the new URL.
            let (body, content_type, binary_note) =
                match fetch_url(&fetch_target, fetch_config).await {
                    Ok(FetchOutcome::Body {
                        body,
                        content_type,
                        binary_note,
                    }) => (body, content_type, binary_note),
                    Ok(FetchOutcome::CrossOriginRedirect { new_url }) => {
                        return Ok(ToolResult {
                            data: serde_json::json!({
                                "url": url,
                                "prompt": prompt,
                                "redirect_blocked": true,
                                "new_url": new_url,
                                "message": format!(
                                    "The URL redirected to a different origin ({new_url}). \
                                     For security, WebFetch does not automatically follow cross-\
                                     origin redirects. If you want the redirected content, issue \
                                     a new WebFetch call with url={new_url}."
                                ),
                            }),
                            new_messages: vec![],
                            app_state_patch: None,
                            permission_updates: Vec::new(),
                            display_data: None,
                        });
                    }
                    Err(e) => {
                        return Err(ToolError::ExecutionFailed {
                            message: format!("Failed to fetch {url}: {e}"),
                            display_data: None,
                            source: None,
                        });
                    }
                };

            // Stage 2: HTML → markdown via `html2text`. Plain-text bodies
            // (JSON, text/plain, unknown) bypass the conversion and are
            // passed through as-is.
            let markdown = if is_html_content_type(&content_type) {
                html_to_markdown(&body)
            } else {
                body
            };

            // Stage 3: truncate to the extraction budget (100K chars
            // by default; configurable via `web_fetch.max_content_length`).
            let (extraction_input, was_truncated) = if markdown.len() > max_fetch_len {
                // char_indices-safe truncation to avoid splitting mid-UTF-8.
                let cut = markdown
                    .char_indices()
                    .take(max_fetch_len)
                    .last()
                    .map(|(i, c)| i + c.len_utf8())
                    .unwrap_or(markdown.len());
                (markdown[..cut].to_string(), true)
            } else {
                (markdown, false)
            };

            // Populate the cache so subsequent fetches of the same
            // URL within the 15-min TTL are zero-cost. We cache the
            // POST-truncation markdown so we don't waste bytes on
            // content we'd only throw away again.
            web_fetch_cache_set(
                url.to_string(),
                CachedWebFetch {
                    markdown: extraction_input.clone(),
                    content_type: content_type.clone(),
                    was_truncated,
                    inserted_at: std::time::Instant::now(),
                },
            );

            (extraction_input, was_truncated, content_type, binary_note)
        };

        // A preapproved docs host serving raw markdown that fits the budget
        // is returned VERBATIM — the main model reads the real page instead
        // of a lossy side-model summary. `!was_truncated` means content is
        // within the budget threshold.
        // The binary-saved note is appended verbatim to whatever content the
        // model ultimately receives.
        let binary_note_str = binary_note.unwrap_or_default();

        let is_preapproved = is_preapproved_url(url);
        if is_preapproved && content_type.to_lowercase().contains("text/markdown") && !was_truncated
        {
            return Ok(ToolResult {
                data: serde_json::json!({
                    "url": url,
                    "prompt": prompt,
                    "content": format!("{extraction_input}{binary_note_str}"),
                    "truncated": false,
                    "extraction_mode": "preapproved_verbatim",
                }),
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            });
        }

        // Stage 4: LLM extraction pass via side-query. Delegates to
        // `ctx.side_query` so the extraction works with whichever provider
        // the user configured.
        //
        // When side_query is the `NoOpSideQuery` stub (e.g. unit tests),
        // the extraction call errors out. We fall back to returning the
        // markdown directly with the user prompt attached to keep tests green.
        let user_message = format!(
            "{prompt}\n\n---\n\nWeb page content (markdown):\n\n{extraction_input}\n\n{guidelines}",
            guidelines = extract_guidelines(is_preapproved)
        );
        let request =
            SideQueryRequest::simple(WEB_FETCH_EXTRACT_SYSTEM, &user_message, "web_fetch_extract");

        let extracted = match ctx.side_query.query(request).await {
            Ok(response) => response.text.unwrap_or_default(),
            Err(e) => {
                tracing::debug!(
                    "WebFetch extraction side-query unavailable ({e}); returning raw markdown"
                );
                // Fallback: surface the raw markdown so the main model can
                // do the extraction itself. No information is lost.
                return Ok(ToolResult {
                    data: serde_json::json!({
                        "url": url,
                        "prompt": prompt,
                        "content": format!("{extraction_input}{binary_note_str}"),
                        "truncated": was_truncated,
                        "extraction_mode": "raw",
                    }),
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                });
            }
        };

        Ok(ToolResult {
            data: serde_json::json!({
                "url": url,
                "prompt": prompt,
                "extracted": format!("{extracted}{binary_note_str}"),
                "truncated": was_truncated,
                "extraction_mode": "llm",
            }),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Check whether a Content-Type header indicates HTML.
fn is_html_content_type(content_type: &str) -> bool {
    let lower = content_type.to_lowercase();
    lower.contains("text/html") || lower.contains("application/xhtml")
}

/// Convert HTML to plain-text markdown via `html2text`. Wraps at 120
/// columns. Separated so it can be unit-tested against inline HTML fixtures.
pub(super) fn html_to_markdown(html: &str) -> String {
    html2text::from_read(html.as_bytes(), HTML2TEXT_LINE_WIDTH).unwrap_or_else(|e| {
        tracing::debug!("html2text failed ({e}); falling back to tag-stripped text");
        // html2text can fail on pathological HTML. Use the existing
        // strip_html_tags + decode_html_entities pipeline as a graceful
        // fallback so the tool still returns something readable.
        decode_html_entities(&strip_html_tags(html))
    })
}

/// Result of a WebFetch network call — `(body, content_type)` on success,
/// or a structured redirect notice when the response redirects cross-origin.
enum FetchOutcome {
    Body {
        body: String,
        content_type: String,
        /// When the body was binary, the note "[Binary content (…) also
        /// saved to <path>]" is appended to the final result so the model
        /// knows the real bytes are on disk. `None` for normal text responses.
        binary_note: Option<String>,
    },
    /// Cross-origin redirect was blocked. The caller should surface this
    /// to the model so it can issue a new fetch with the new URL.
    CrossOriginRedirect { new_url: String },
}

/// Fetch URL content using reqwest. Applies:
/// - `config.timeout_secs` (default 60s)
/// - `config.user_agent`
/// - Manual redirect policy (SSRF guard — no auto-follow, same-origin
///   allowed, cross-origin surfaces to the model)
/// - 10MB Content-Length pre-check
/// - Binary MIME detection (note appended to result so model can reference
///   the bytes on disk)
async fn fetch_url(
    url: &str,
    config: &coco_config::WebFetchConfig,
) -> Result<FetchOutcome, String> {
    let timeout_secs = config.timeout_secs.max(1) as u64;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .user_agent(config.user_agent.as_str())
        // Critical SSRF guard: disable automatic redirect following.
        // We handle redirects manually via `check_redirect` so cross-
        // origin hops go back through the model's decision loop.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("[NETWORK_ERROR] failed to build HTTP client: {e}"))?;

    // Manual redirect loop. Max 10 hops: most real-world chains are 1–2,
    // but some legitimate cases (shortener → auth → final content) need
    // 4–5. A cap of 10 avoids rejecting valid chains while bounding the loop.
    let mut current_url = url.to_string();
    for _ in 0..10 {
        let response = client.get(&current_url).send().await.map_err(|e| {
            let tag = if e.is_timeout() {
                "[TIMEOUT]"
            } else {
                "[NETWORK_ERROR]"
            };
            format!("{tag} HTTP request failed: {e}")
        })?;

        let status = response.status();

        // Redirect response → check origin policy.
        if status.is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if location.is_empty() {
                return Err(format!(
                    "[NETWORK_ERROR] {status} redirect without Location"
                ));
            }
            // Resolve relative redirects against the current URL.
            let next_url = resolve_redirect_url(&current_url, location);
            match check_redirect(&current_url, &next_url) {
                RedirectDecision::Allow => {
                    current_url = next_url;
                    continue;
                }
                RedirectDecision::CrossOrigin { new_url } => {
                    return Ok(FetchOutcome::CrossOriginRedirect { new_url });
                }
            }
        }

        if !status.is_success() {
            return Err(format!("[NETWORK_ERROR] HTTP {status}"));
        }

        // Content-Length pre-check (fast path). Handles the optimistic case
        // where the server advertises size up-front. The streaming check below
        // handles chunked responses and servers that lie about Content-Length.
        if let Some(length) = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            && length > MAX_HTTP_CONTENT_LENGTH
        {
            return Err(format!(
                "[NETWORK_ERROR] response too large: {length} bytes > {MAX_HTTP_CONTENT_LENGTH} \
                 byte limit (Content-Length header). Use a more specific URL or a different tool."
            ));
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_lowercase();

        // Binary bodies are persisted to disk (mime-derived extension) and
        // still UTF-8 decoded + run through the pipeline, with a
        // "[Binary content … also saved to <path>]" note appended to the result.
        let is_binary = content_type.contains("image/")
            || content_type.contains("audio/")
            || content_type.contains("video/")
            || content_type.contains("application/octet-stream")
            || content_type.contains("application/zip");

        // Streaming read with per-chunk byte-count enforcement.
        //
        // **SSRF safety (D9)**: if we used `response.text().await` we'd
        // buffer the entire body before any limit check ran, so a
        // malicious server sending an infinite chunked response (no
        // Content-Length) could exhaust memory before we noticed. The
        // stream loop below accumulates bytes in a `Vec<u8>` and hard-
        // fails the moment we cross `MAX_HTTP_CONTENT_LENGTH`, regardless
        // of whether Content-Length was advertised.
        //
        // We collect bytes (not string) during streaming because chunks
        // may split UTF-8 code points; we decode once at the end.
        use futures::StreamExt as _;
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk
                .map_err(|e| format!("[NETWORK_ERROR] Failed to read response body chunk: {e}"))?;
            if (buffer.len() as u64).saturating_add(chunk.len() as u64) > MAX_HTTP_CONTENT_LENGTH {
                return Err(format!(
                    "[NETWORK_ERROR] response too large: streaming body exceeded \
                     {MAX_HTTP_CONTENT_LENGTH} bytes before completion. The server did \
                     not advertise Content-Length or lied about it. Use a more \
                     specific URL or a different tool."
                ));
            }
            buffer.extend_from_slice(&chunk);
        }
        // #57: persist binary bodies before the lossy decode so the model
        // can reference the real bytes on disk.
        let binary_note = if is_binary {
            match persist_binary_content(&buffer, &content_type) {
                Ok((path, size)) => Some(format!(
                    "\n\n[Binary content ({content_type}, {size} bytes) also saved to {path}]"
                )),
                Err(e) => {
                    tracing::debug!(
                        "WebFetch binary persist failed ({e}); continuing without note"
                    );
                    None
                }
            }
        } else {
            None
        };
        let body = String::from_utf8_lossy(&buffer).into_owned();

        return Ok(FetchOutcome::Body {
            body,
            content_type,
            binary_note,
        });
    }

    Err("[NETWORK_ERROR] too many redirects (exceeded 10 hops)".into())
}

/// Persist a binary WebFetch body to a temp file with a mime-derived
/// extension. Returns `(absolute_path, byte_len)`.
fn persist_binary_content(bytes: &[u8], content_type: &str) -> std::io::Result<(String, usize)> {
    let ext = mime_to_extension(content_type);
    // A content-addressed name avoids `Math.random`/clock use (forbidden
    // in some build contexts) while staying collision-resistant.
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    let dir = std::env::temp_dir().join("coco-web-fetch");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{hash:016x}.{ext}"));
    std::fs::write(&path, bytes)?;
    Ok((path.to_string_lossy().into_owned(), bytes.len()))
}

/// Map a binary content-type to a file extension. Falls back to `bin`.
fn mime_to_extension(content_type: &str) -> &'static str {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    match ct {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "video/mp4" => "mp4",
        _ => "bin",
    }
}

/// Resolve a possibly-relative Location header against the base URL.
///
/// Handles three cases:
/// - Absolute URL (`https://...`) → use as-is
/// - Protocol-relative (`//host/path`) → inherit scheme
/// - Path-relative (`/path` or `path`) → inherit scheme + host
pub(super) fn resolve_redirect_url(base: &str, location: &str) -> String {
    if location.starts_with("http://") || location.starts_with("https://") {
        return location.to_string();
    }
    if let Some(stripped) = location.strip_prefix("//") {
        // Protocol-relative: inherit scheme from base.
        let scheme = base.split_once("://").map(|(s, _)| s).unwrap_or("https");
        return format!("{scheme}://{stripped}");
    }
    // Path-relative: inherit scheme + host from base.
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let host_end = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..host_end];
    if location.starts_with('/') {
        format!("{scheme}://{host}{location}")
    } else {
        // Rare form — relative to current directory. Strip to last `/`.
        let base_dir = &rest[..rest.rfind('/').unwrap_or(host_end)];
        format!("{scheme}://{base_dir}/{location}")
    }
}

// ---------------------------------------------------------------------------
// WebSearchTool — third-party search backend
// ---------------------------------------------------------------------------
//
// Design: a **provider-agnostic** local backend. The default is DuckDuckGo
// HTML scraping (no API key, no rate limits, no ToS surprises), with a
// Tavily REST fallback for users who opt in via env vars. Both run entirely
// on the client side so the tool works identically regardless of which LLM
// provider the user selected.
//
// # Cache
//
// 15-min TTL in-process cache. Prevents redundant DuckDuckGo traffic when
// the model retries the same query within a turn. Session-scoped only.
//
// # Error classification
//
// Explicit error tags (`[TIMEOUT]`, `[NETWORK_ERROR]`, `[RATE_LIMITED]`,
// `[PARSE_ERROR]`) let the model react appropriately — e.g. retrying with a
// different query after a parse error vs. backing off on rate limits.

use serde::Serialize;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// Cache TTL for search results: 15 minutes.
const SEARCH_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

/// Max cached entries before LRU eviction. Keeps memory bounded in long
/// sessions where the model issues many unrelated queries.
const SEARCH_CACHE_MAX_ENTRIES: usize = 64;

/// HTTP client timeout for search requests.
const SEARCH_TIMEOUT_SECS: u64 = 15;

/// Hard upper bound for `max_results` accepted from the model. Matches
/// the schema (`maximum: 20`) and the `clamp(1, 20)` in `execute()`.
/// The over-fetch factor in `parse_duckduckgo_html` doubles this for
/// post-filter slack — domain filters reject some results, so we want
/// candidates left after filtering even at the schema ceiling.
const SEARCH_MAX_RESULTS_CEILING: usize = 20;

/// Minimum query length to accept.
const SEARCH_MIN_QUERY_LEN: usize = 2;

/// Truncate DuckDuckGo response bodies before regex parsing. A normal
/// SERP is ~80–150 KB; cap at 512 KB so an adversarial or oversized
/// response can't drive regex backtracking on megabytes of HTML.
const DDG_HTML_PARSE_CAP: usize = 512 * 1024;

/// Shared HTTP client for both backends. `LazyLock<reqwest::Client>`
/// reuses the connection pool across calls — building a fresh client
/// per request rebuilds TLS state for every search.
///
/// The User-Agent here is the DuckDuckGo browser-mimic string; Tavily
/// is JSON-only and ignores it. If we ever need backend-specific UAs,
/// switch to per-request `header()` overrides.
#[allow(clippy::expect_used)]
static SEARCH_HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(SEARCH_TIMEOUT_SECS))
        .user_agent(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .build()
        .expect("build static reqwest::Client for WebSearch")
});

/// Cached search entry. Stored with insertion time so expired entries are
/// skipped on lookup.
#[derive(Clone)]
struct CachedSearch {
    results: Vec<SearchResult>,
    inserted_at: Instant,
}

/// One search result — title + URL + optional snippet.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchResult {
    title: String,
    url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snippet: Option<String>,
}

/// Tagged error classification — `[TAG] message` lets the model
/// distinguish retryable (`TIMEOUT`, `NETWORK_ERROR`) from non-retryable
/// (`API_KEY_MISSING`, `PARSE_ERROR`) failures without parsing prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebSearchErrorType {
    ProviderError,
    NetworkError,
    Timeout,
    RateLimited,
    ApiKeyMissing,
    ParseError,
}

impl WebSearchErrorType {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProviderError => "PROVIDER_ERROR",
            Self::NetworkError => "NETWORK_ERROR",
            Self::Timeout => "TIMEOUT",
            Self::RateLimited => "RATE_LIMITED",
            Self::ApiKeyMissing => "API_KEY_MISSING",
            Self::ParseError => "PARSE_ERROR",
        }
    }

    fn into_tool_err(self, message: impl AsRef<str>) -> ToolError {
        ToolError::ExecutionFailed {
            message: format!("[{}] {}", self.as_str(), message.as_ref()),
            display_data: None,
            source: None,
        }
    }
}

/// Both backends map `send()` errors the same way.
fn classify_reqwest_err(e: &reqwest::Error) -> WebSearchErrorType {
    if e.is_timeout() {
        WebSearchErrorType::Timeout
    } else {
        WebSearchErrorType::NetworkError
    }
}

/// In-process LRU cache. `LazyLock<Mutex<...>>` gives us a zero-config
/// singleton that initializes on first access. Cache key includes the
/// provider + max_results so a DuckDuckGo miss doesn't poison a later
/// Tavily hit (and vice versa). Domain filters are applied post-cache
/// so a single fetch can serve multiple filter permutations.
static SEARCH_CACHE: LazyLock<Mutex<Vec<(String, CachedSearch)>>> =
    LazyLock::new(|| Mutex::new(Vec::with_capacity(SEARCH_CACHE_MAX_ENTRIES)));

fn cache_key(provider: coco_config::WebSearchProvider, max_results: usize, query: &str) -> String {
    format!("{}:{max_results}:{query}", provider.as_str())
}

fn cache_get(
    provider: coco_config::WebSearchProvider,
    max_results: usize,
    query: &str,
) -> Option<Vec<SearchResult>> {
    let key = cache_key(provider, max_results, query);
    let cache = SEARCH_CACHE.lock().ok()?;
    let now = Instant::now();
    cache.iter().find_map(|(k, v)| {
        if k == &key && now.duration_since(v.inserted_at) < SEARCH_CACHE_TTL {
            Some(v.results.clone())
        } else {
            None
        }
    })
}

fn cache_set(
    provider: coco_config::WebSearchProvider,
    max_results: usize,
    query: &str,
    results: Vec<SearchResult>,
) {
    let key = cache_key(provider, max_results, query);
    if let Ok(mut cache) = SEARCH_CACHE.lock() {
        let now = Instant::now();
        // Drop entries that match the new key OR have aged out — keeps
        // expiry work off the read path while still bounding memory.
        cache.retain(|(k, v)| k != &key && now.duration_since(v.inserted_at) < SEARCH_CACHE_TTL);
        if cache.len() >= SEARCH_CACHE_MAX_ENTRIES {
            cache.remove(0);
        }
        cache.push((
            key,
            CachedSearch {
                results,
                inserted_at: now,
            },
        ));
    }
}

/// Format the current local month and year as `"Month YYYY"`.
///
/// Injected into the search prompt so the model knows the actual current
/// date when interpreting "latest", "recent", or "current" search queries.
fn current_month_year_local() -> String {
    use chrono::Datelike;
    let now = chrono::Local::now();
    const MONTH_NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let month_name = MONTH_NAMES
        .get((now.month() as usize).saturating_sub(1))
        .copied()
        .unwrap_or("January");
    format!("{month_name} {}", now.year())
}

/// Model-facing WebSearch tool description. The CRITICAL REQUIREMENT block
/// mandates that the model add a `Sources:` section to every response. The
/// current month/year is injected at request time so the model uses the
/// right year for recent-events queries. Surfaced via [`WebSearchTool::prompt`].
fn web_search_prompt_text() -> String {
    let current_month_year = current_month_year_local();
    format!(
        "
- Allows Claude to search the web and use the results to inform responses
- Provides up-to-date information for current events and recent data
- Returns search result information formatted as search result blocks, including links as markdown hyperlinks
- Use this tool for accessing information beyond Claude's knowledge cutoff
- Searches are performed automatically within a single API call

CRITICAL REQUIREMENT - You MUST follow this:
  - After answering the user's question, you MUST include a \"Sources:\" section at the end of your response
  - In the Sources section, list all relevant URLs from the search results as markdown hyperlinks: [Title](URL)
  - This is MANDATORY - never skip including sources in your response
  - Example format:

    [Your answer here]

    Sources:
    - [Source Title 1](https://example.com/1)
    - [Source Title 2](https://example.com/2)

Usage notes:
  - Domain filtering is supported to include or block specific websites
  - Web search is only available in the US

IMPORTANT - Use the correct year in search queries:
  - The current month is {current_month_year}. You MUST use this year when searching for recent information, documentation, or current events.
  - Example: If the user asks for \"latest React docs\", search for \"React documentation\" with the current year, NOT last year
"
    )
}

pub struct WebSearchTool;

#[async_trait::async_trait]
impl Tool for WebSearchTool {
    type Input = WebSearchInput;
    // Static schema from a literal `json!`; a parse failure means the literal
    // is malformed (a programmer error), so panicking on first build is correct.
    #[allow(clippy::expect_used)]
    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        static SCHEMA: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
            std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            coco_tool_runtime::ToolInputSchema::from_static_value(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query to use",
                        "minLength": 2
                    },
                    // Per-call override for `web_search.max_results` in settings —
                    // lets the model widen for broad surveys or narrow for
                    // precision. Clamped to `[1, SEARCH_MAX_RESULTS_CEILING]` at
                    // execute-time.
                    "max_results": {
                        "type": "integer",
                        "description": format!(
                            "Maximum number of results to return (1-{SEARCH_MAX_RESULTS_CEILING}). \
                             Overrides the configured default."
                        ),
                        "minimum": 1,
                        "maximum": SEARCH_MAX_RESULTS_CEILING
                    },
                    "allowed_domains": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Only include search results from these domains (post-filtered client-side)"
                    },
                    "blocked_domains": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Never include search results from these domains (post-filtered client-side)"
                    }
                },
                // `query` is the only required field. `max_results` stays
                // optional. `allowed_domains`/`blocked_domains` are optional.
                "required": ["query"]
            }))
        })
    }
    /// Wire shape carries both prebuilt `formatted` markdown and a
    /// downstream-consumer `results` array; staying on `Value` keeps
    /// the consumer flexibility without forcing a typed result envelope.
    type Output = serde_json::Value;

    fn to_auto_classifier_input(&self, input: &WebSearchInput) -> Option<String> {
        Some(input.query.clone())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::WebSearch)
    }
    fn name(&self) -> &str {
        ToolName::WebSearch.as_str()
    }
    fn is_enabled(&self, ctx: &coco_tool_runtime::ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::WebSearch)
    }
    /// Short UI label: `Claude wants to search the web for: ${input.query}`.
    /// The long model-facing guidance lives in [`Self::prompt`].
    fn description(&self, input: &WebSearchInput, _options: &DescriptionOptions) -> String {
        format!("Claude wants to search the web for: {}", input.query)
    }
    /// Model-facing tool description.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        web_search_prompt_text()
    }
    fn is_read_only(&self, _input: &WebSearchInput) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &WebSearchInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("search the web for current information")
    }

    fn validate_input(
        &self,
        input: &WebSearchInput,
        _ctx: &ToolUseContext,
    ) -> coco_tool_runtime::ValidationResult {
        if input.query.trim().len() < SEARCH_MIN_QUERY_LEN {
            return coco_tool_runtime::ValidationResult::invalid(
                "query must be at least 2 characters long",
            );
        }
        // Mutual exclusivity: caller should not set both filters. If a domain
        // is in both lists, which wins? We reject at validation time to force
        // a clear policy.
        let has_allowed = input
            .allowed_domains
            .as_ref()
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let has_blocked = input
            .blocked_domains
            .as_ref()
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        if has_allowed && has_blocked {
            return coco_tool_runtime::ValidationResult::invalid(
                "Specify either allowed_domains or blocked_domains, not both",
            );
        }
        coco_tool_runtime::ValidationResult::Valid
    }

    fn get_activity_description(&self, input: &WebSearchInput) -> Option<String> {
        if input.query.is_empty() {
            return None;
        }
        Some(format!("Searching for \"{}\"", input.query))
    }

    /// Render the prebuilt `formatted` field — the structured `results`
    /// array is for downstream consumers; the model only needs the
    /// markdown-shaped digest.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let text = data
            .get("formatted")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string(data).unwrap_or_default());
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: WebSearchInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let query = input.query.trim().to_string();

        let allowed: Vec<String> = input
            .allowed_domains
            .as_ref()
            .map(|a| a.iter().map(|s| s.to_lowercase()).collect())
            .unwrap_or_default();
        let blocked: Vec<String> = input
            .blocked_domains
            .as_ref()
            .map(|a| a.iter().map(|s| s.to_lowercase()).collect())
            .unwrap_or_default();

        // max_results precedence: input override > config default.
        // Clamped to [1, 20] regardless of source so a misconfigured
        // settings file or a hostile input can't force us to fetch
        // thousands of pages.
        let max_results = input
            .max_results
            .map(|n| n as usize)
            .unwrap_or_else(|| ctx.web_search_config.max_results.max(1) as usize)
            .clamp(1, SEARCH_MAX_RESULTS_CEILING);

        let provider = effective_search_provider(ctx.web_search_config.provider);

        // Cache hit → return immediately with filters re-applied.
        let results = if let Some(cached) = cache_get(provider, max_results, &query) {
            cached
        } else {
            let fresh =
                search_by_provider(provider, &query, max_results, &ctx.web_search_config).await?;
            cache_set(provider, max_results, &query, fresh.clone());
            fresh
        };

        // Apply domain filters. Matches host suffix so "github.com" blocks
        // both github.com/foo and gist.github.com/foo.
        let filtered: Vec<SearchResult> = results
            .into_iter()
            .filter(|r| {
                let host = extract_host(&r.url).to_lowercase();
                if !allowed.is_empty() && !allowed.iter().any(|d| host.ends_with(d)) {
                    return false;
                }
                if blocked.iter().any(|d| host.ends_with(d)) {
                    return false;
                }
                true
            })
            .take(max_results)
            .collect();

        let formatted = format_results(&query, &filtered);
        Ok(ToolResult {
            data: serde_json::json!({
                "query": query,
                "results": filtered,
                "formatted": formatted,
            }),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// OpenAI native search is not implemented — falls back to DuckDuckGo so
/// the tool stays functional, with a one-time warning so the user knows the
/// configured provider isn't being used. Resolving here (not in
/// `search_by_provider`) keeps the cache key aligned with the actual backend.
fn effective_search_provider(
    configured: coco_config::WebSearchProvider,
) -> coco_config::WebSearchProvider {
    if configured == coco_config::WebSearchProvider::OpenAi {
        static WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        WARNED.get_or_init(|| {
            tracing::warn!(
                target: "coco_tools::web_search",
                "WebSearchConfig.provider=OpenAi has no native backend in coco-rs; \
                 falling back to DuckDuckGo. Set provider=duckduckgo or provider=tavily \
                 explicitly to silence this warning."
            );
        });
        return coco_config::WebSearchProvider::DuckDuckGo;
    }
    configured
}

/// Dispatch to the configured search backend. Callers must pre-resolve
/// `OpenAi` via `effective_search_provider` — reaching that arm here is
/// a bug, not a fallback.
async fn search_by_provider(
    provider: coco_config::WebSearchProvider,
    query: &str,
    max_results: usize,
    config: &coco_config::WebSearchConfig,
) -> Result<Vec<SearchResult>, ToolError> {
    match provider {
        coco_config::WebSearchProvider::DuckDuckGo => duckduckgo_search(query, max_results).await,
        coco_config::WebSearchProvider::Tavily => tavily_search(query, max_results, config).await,
        coco_config::WebSearchProvider::OpenAi => Err(WebSearchErrorType::ProviderError
            .into_tool_err(
                "OpenAi provider must be resolved via effective_search_provider before dispatch",
            )),
    }
}

/// Format results as a markdown block the model can read naturally.
fn format_results(query: &str, results: &[SearchResult]) -> String {
    if results.is_empty() {
        return format!("No results for \"{query}\".");
    }
    let mut out = format!(
        "Search results for \"{query}\" ({} hit(s)):\n\n",
        results.len()
    );
    // No trailing reminder — `WebSearchTool::prompt()` already
    // mandates the `Sources:` section as a CRITICAL REQUIREMENT;
    // duplicating it here competes for model attention.
    for (i, r) in results.iter().enumerate() {
        out.push_str(&format!("[{}] {}\n  {}\n", i + 1, r.title, r.url));
        if let Some(snippet) = &r.snippet
            && !snippet.is_empty()
        {
            out.push_str(&format!("  {snippet}\n"));
        }
    }
    out
}

/// Extract the host portion of a URL (without scheme, port, or path). Used
/// for domain-filter matching. Returns empty string on malformed URLs.
/// Extract the host portion of a URL (without scheme, userinfo, port,
/// path, query, or fragment). Handles bracketed IPv6 literals correctly
/// and does NOT leave a bogus `:99999` suffix on port-parse failure.
///
/// T1: Previously this used a naive `rsplit_once(':')` that broke on
/// bracketed IPv6 (`[::1]` → host=`[::`) and silently corrupted hosts
/// with out-of-range ports (`example.com:99999` → host stayed intact).
/// Delegating to `split_host_port` ensures the host extraction logic
/// stays consistent between `check_redirect`'s port comparison path
/// and `is_preapproved_url`'s hostname-match path.
///
/// The `scheme` argument is passed through from the URL's scheme prefix
/// (or `""` if absent) so that default-port normalization works
/// (e.g. `example.com:443` under https → port=None, host=example.com).
fn extract_host(url: &str) -> String {
    let scheme = extract_scheme(url);
    split_host_port(url, &scheme).0
}

/// DuckDuckGo HTML backend. Scrapes the non-JS result page at
/// `html.duckduckgo.com` — this endpoint returns plain HTML with a stable
/// structure that's easier to parse than the JS-rendered homepage.
///
/// Regex-based parsing is intentionally simple: the DDG HTML result page
/// uses a `<a class="result__a"` for titles, `<a class="result__snippet"`
/// for snippets, and embeds the target URL in a `uddg=` parameter of a
/// redirect link. We decode the redirect, not the visible `href`.
async fn duckduckgo_search(
    query: &str,
    max_results: usize,
) -> Result<Vec<SearchResult>, ToolError> {
    // POST to html.duckduckgo.com/html with `q=<query>` — GET also works
    // but POST avoids leaking the query in any proxy logs along the path.
    let response = SEARCH_HTTP_CLIENT
        .post("https://html.duckduckgo.com/html/")
        .form(&[("q", query)])
        .send()
        .await
        .map_err(|e| {
            classify_reqwest_err(&e).into_tool_err(format!("DuckDuckGo request failed: {e}"))
        })?;

    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(WebSearchErrorType::RateLimited
            .into_tool_err("DuckDuckGo returned HTTP 429 — back off and retry"));
    }
    if !response.status().is_success() {
        return Err(WebSearchErrorType::ProviderError
            .into_tool_err(format!("DuckDuckGo returned HTTP {}", response.status())));
    }

    let mut html = response.text().await.map_err(|e| {
        WebSearchErrorType::NetworkError
            .into_tool_err(format!("failed to read DuckDuckGo response: {e}"))
    })?;

    // Cap parser input. Bounds regex backtracking against an oversized
    // (or adversarial) response so a hostile upstream can't burn CPU.
    if html.len() > DDG_HTML_PARSE_CAP {
        html.truncate(html.floor_char_boundary(DDG_HTML_PARSE_CAP));
    }

    Ok(parse_duckduckgo_html(&html, max_results))
}

/// Tavily REST search backend. Requires an API key in
/// `WebSearchConfig.api_key` or the `TAVILY_API_KEY` environment
/// variable. The config value wins so per-project settings don't get
/// overridden by a stale shell env.
async fn tavily_search(
    query: &str,
    max_results: usize,
    config: &coco_config::WebSearchConfig,
) -> Result<Vec<SearchResult>, ToolError> {
    #[derive(Serialize)]
    struct TavilyRequest<'a> {
        api_key: &'a str,
        query: &'a str,
        max_results: usize,
        search_depth: &'static str,
        include_answer: bool,
        include_raw_content: bool,
    }
    #[derive(Debug, Deserialize)]
    struct TavilyResponse {
        results: Vec<TavilyResult>,
    }
    #[derive(Debug, Deserialize)]
    struct TavilyResult {
        title: String,
        url: String,
        content: String,
    }

    let api_key = config
        .api_key
        .clone()
        .or_else(|| std::env::var("TAVILY_API_KEY").ok())
        .ok_or_else(|| {
            WebSearchErrorType::ApiKeyMissing.into_tool_err(
                "TAVILY_API_KEY not set. Configure `[web_search] api_key` in \
                 settings.json or set the TAVILY_API_KEY env var. \
                 Get a key at https://tavily.com",
            )
        })?;

    let body = TavilyRequest {
        api_key: &api_key,
        query,
        max_results,
        search_depth: "basic",
        include_answer: false,
        include_raw_content: false,
    };

    let response = SEARCH_HTTP_CLIENT
        .post("https://api.tavily.com/search")
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            classify_reqwest_err(&e).into_tool_err(format!("Tavily request failed: {e}"))
        })?;

    let status = response.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(WebSearchErrorType::RateLimited.into_tool_err("Tavily API rate limit exceeded"));
    }
    if !status.is_success() {
        return Err(WebSearchErrorType::ProviderError
            .into_tool_err(format!("Tavily API returned status {status}")));
    }

    let parsed: TavilyResponse = response.json().await.map_err(|e| {
        WebSearchErrorType::ParseError
            .into_tool_err(format!("failed to parse Tavily response: {e}"))
    })?;

    Ok(parsed
        .results
        .into_iter()
        .map(|r| SearchResult {
            title: r.title,
            url: r.url,
            snippet: (!r.content.is_empty()).then_some(r.content),
        })
        .collect())
}

/// `<a class="result__a" href="...">TITLE</a>` — the href is a redirect
/// URL (decoded by `decode_ddg_redirect`). Snippets follow in a sibling
/// `<a class="result__snippet">SNIPPET</a>`. Compiled once via
/// `LazyLock` — both patterns are static.
#[allow(clippy::expect_used)]
static DDG_TITLE_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?s)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)
        .expect("DDG title regex is statically valid")
});
#[allow(clippy::expect_used)]
static DDG_SNIPPET_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(?s)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#)
        .expect("DDG snippet regex is statically valid")
});

/// Parse DuckDuckGo HTML response into `SearchResult` list. Separated so
/// it can be unit-tested against recorded fixtures.
///
/// `max_results` is the caller's requested ceiling; we over-fetch by 2x
/// so post-fetch domain filtering still leaves candidates after rejecting
/// blocked hosts. Caller is responsible for the final `.take(max_results)`.
fn parse_duckduckgo_html(html: &str, max_results: usize) -> Vec<SearchResult> {
    let fetch_cap = max_results
        .min(SEARCH_MAX_RESULTS_CEILING)
        .saturating_mul(2);

    let mut results = Vec::new();
    let title_matches: Vec<_> = DDG_TITLE_PATTERN.captures_iter(html).collect();
    let snippet_matches: Vec<_> = DDG_SNIPPET_PATTERN.captures_iter(html).collect();

    for (i, title_cap) in title_matches.iter().enumerate() {
        let raw_href = title_cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let raw_title = title_cap.get(2).map(|m| m.as_str()).unwrap_or("");

        let url = decode_ddg_redirect(raw_href);
        if url.is_empty() {
            continue;
        }

        let title = decode_html_entities(&strip_html_tags(raw_title));
        let snippet = snippet_matches
            .get(i)
            .and_then(|c| c.get(1).map(|m| m.as_str()))
            .map(|s| decode_html_entities(&strip_html_tags(s)))
            .filter(|s| !s.is_empty());

        results.push(SearchResult {
            title,
            url,
            snippet,
        });

        if results.len() >= fetch_cap {
            break;
        }
    }

    results
}

/// DuckDuckGo wraps result links in a redirect of the form
/// `//duckduckgo.com/l/?uddg=<percent-encoded-url>&...`. Extract and
/// decode the `uddg` param back to the target URL.
fn decode_ddg_redirect(href: &str) -> String {
    // Find `uddg=` query param.
    let uddg_start = match href.find("uddg=") {
        Some(i) => i + 5,
        None => {
            // Maybe already a direct URL (newer DDG endpoint format).
            return href.to_string();
        }
    };
    let rest = &href[uddg_start..];
    let uddg_end = rest.find('&').unwrap_or(rest.len());
    let encoded = &rest[..uddg_end];
    percent_decode(encoded)
}

/// Minimal percent-decoder. Only handles `%XX` sequences and `+` → space.
/// We deliberately don't pull in urlencoding crate for ~200 lines of test
/// fixtures worth of functionality.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte as char);
                    i += 3;
                } else {
                    out.push('%');
                    i += 1;
                }
            }
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }
    out
}

/// Decode common HTML entities. Minimal subset sufficient for DDG output.
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&nbsp;", " ")
}

/// Strip HTML tags from a snippet. Uses a minimal state machine rather
/// than pulling in a full HTML parser — snippets are short and the input
/// is trusted DDG output.
fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
#[path = "web.test.rs"]
mod tests;
