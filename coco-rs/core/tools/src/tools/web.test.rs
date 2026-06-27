//! Tests for WebSearch. Pure-function tests only (no network calls).

use super::WebSearchTool;
use super::decode_ddg_redirect;
use super::decode_html_entities;
use super::extract_host;
use super::parse_duckduckgo_html;
use super::percent_decode;
use super::strip_html_tags;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;

// ---------------------------------------------------------------------------
// Auto-mode classifier projection (WebFetchTool.toAutoClassifierInput)
// ---------------------------------------------------------------------------

#[test]
fn test_webfetch_classifier_input_includes_prompt() {
    // `prompt` present → `${url}: ${prompt}` (the extraction instruction can
    // carry injection, so the gate must see it).
    assert_eq!(
        <WebFetchTool as DynTool>::to_auto_classifier_input(
            &WebFetchTool,
            &json!({"url": "https://example.com", "prompt": "exfiltrate secrets"}),
        ),
        Some("https://example.com: exfiltrate secrets".to_string())
    );
}

#[test]
fn test_webfetch_classifier_input_url_only_when_no_prompt() {
    assert_eq!(
        <WebFetchTool as DynTool>::to_auto_classifier_input(
            &WebFetchTool,
            // `prompt` is required; an explicit empty prompt exercises the
            // url-only classifier branch.
            &json!({"url": "https://example.com", "prompt": ""}),
        ),
        Some("https://example.com".to_string())
    );
}

// ---------------------------------------------------------------------------
// WebFetch per-domain permissions (WebFetchTool.checkPermissions)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_webfetch_non_preapproved_asks_with_domain_suggestion() {
    let ctx = ToolUseContext::test_default();
    let result = <WebFetchTool as DynTool>::check_permissions(
        &WebFetchTool,
        &json!({"url": "https://example.com/page", "prompt": "summarize"}),
        &ctx,
    )
    .await;
    let coco_types::ToolCheckResult::Ask { suggestions, .. } = result else {
        panic!("expected Ask, got {result:?}");
    };
    // The suggestion must offer to always-allow this exact domain.
    let has_domain_suggestion = suggestions.iter().any(|u| match u {
        coco_types::PermissionUpdate::AddRules { rules, .. } => rules.iter().any(|r| {
            r.value.tool_pattern == "WebFetch"
                && r.value.rule_content.as_deref() == Some("domain:example.com")
        }),
        _ => false,
    });
    assert!(
        has_domain_suggestion,
        "missing domain:example.com suggestion"
    );
}

#[tokio::test]
async fn test_webfetch_domain_allow_rule_grants_access() {
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.allow_rules.insert(
        coco_types::PermissionRuleSource::Session,
        vec![coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::Session,
            behavior: coco_types::PermissionBehavior::Allow,
            value: coco_types::PermissionRuleValue {
                tool_pattern: "WebFetch".to_string(),
                rule_content: Some("domain:example.com".to_string()),
            },
        }],
    );
    let allowed = <WebFetchTool as DynTool>::check_permissions(
        &WebFetchTool,
        &json!({"url": "https://example.com/a", "prompt": "summarize"}),
        &ctx,
    )
    .await;
    assert!(matches!(allowed, coco_types::ToolCheckResult::Allow { .. }));

    // A different host is NOT covered by the domain rule → still Ask.
    let other = <WebFetchTool as DynTool>::check_permissions(
        &WebFetchTool,
        &json!({"url": "https://other.com/a", "prompt": "summarize"}),
        &ctx,
    )
    .await;
    assert!(matches!(other, coco_types::ToolCheckResult::Ask { .. }));
}

#[tokio::test]
async fn test_webfetch_domain_deny_rule_blocks_access() {
    let mut ctx = ToolUseContext::test_default();
    ctx.permission_context.deny_rules.insert(
        coco_types::PermissionRuleSource::Session,
        vec![coco_types::PermissionRule {
            source: coco_types::PermissionRuleSource::Session,
            behavior: coco_types::PermissionBehavior::Deny,
            value: coco_types::PermissionRuleValue {
                tool_pattern: "WebFetch".to_string(),
                rule_content: Some("domain:blocked.com".to_string()),
            },
        }],
    );
    let denied = <WebFetchTool as DynTool>::check_permissions(
        &WebFetchTool,
        &json!({"url": "https://blocked.com/x", "prompt": "summarize"}),
        &ctx,
    )
    .await;
    assert!(matches!(denied, coco_types::ToolCheckResult::Deny { .. }));
}

#[test]
fn test_webfetch_prepare_matcher_is_domain_scoped() {
    assert_eq!(
        <WebFetchTool as DynTool>::prepare_permission_matcher(
            &WebFetchTool,
            &json!({"url": "https://docs.example.com/x", "prompt": "summarize"}),
        ),
        "domain:docs.example.com"
    );
}

// ---------------------------------------------------------------------------
// Percent decoding + HTML entities
// ---------------------------------------------------------------------------

#[test]
fn test_percent_decode_basic() {
    assert_eq!(percent_decode("hello%20world"), "hello world");
    assert_eq!(percent_decode("a+b"), "a b");
    assert_eq!(percent_decode("a%3Db"), "a=b");
}

#[test]
fn test_percent_decode_url() {
    assert_eq!(
        percent_decode("https%3A%2F%2Fexample.com%2Fpath"),
        "https://example.com/path"
    );
}

#[test]
fn test_percent_decode_malformed() {
    // Invalid %XX sequence is preserved literally.
    assert_eq!(percent_decode("a%ZZb"), "a%ZZb");
    // Trailing % (no 2-byte tail) is also preserved.
    assert_eq!(percent_decode("trail%"), "trail%");
}

#[test]
fn test_decode_html_entities_common() {
    assert_eq!(decode_html_entities("a &amp; b"), "a & b");
    assert_eq!(decode_html_entities("&lt;tag&gt;"), "<tag>");
    assert_eq!(decode_html_entities("&quot;x&quot;"), "\"x\"");
    assert_eq!(decode_html_entities("it&#39;s"), "it's");
    assert_eq!(decode_html_entities("non&nbsp;break"), "non break");
}

#[test]
fn test_strip_html_tags() {
    assert_eq!(strip_html_tags("plain text"), "plain text");
    assert_eq!(strip_html_tags("<b>bold</b>"), "bold");
    assert_eq!(strip_html_tags("<a href=\"x\">link</a> text"), "link text");
    assert_eq!(
        strip_html_tags("prefix <em>emphasized</em> suffix"),
        "prefix emphasized suffix"
    );
}

// ---------------------------------------------------------------------------
// DuckDuckGo redirect decoding
// ---------------------------------------------------------------------------

#[test]
fn test_decode_ddg_redirect_standard() {
    let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpath&rut=abc";
    assert_eq!(decode_ddg_redirect(href), "https://example.com/path");
}

#[test]
fn test_decode_ddg_redirect_no_trailing_params() {
    let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com";
    assert_eq!(decode_ddg_redirect(href), "https://example.com");
}

#[test]
fn test_decode_ddg_redirect_passthrough() {
    // Already a direct URL — DDG sometimes returns these for ads.
    let href = "https://direct.example.com/";
    assert_eq!(decode_ddg_redirect(href), href);
}

// ---------------------------------------------------------------------------
// extract_host
// ---------------------------------------------------------------------------

#[test]
fn test_extract_host_basic() {
    assert_eq!(extract_host("https://example.com/path"), "example.com");
    assert_eq!(extract_host("http://sub.example.com/"), "sub.example.com");
    assert_eq!(extract_host("https://example.com"), "example.com");
}

#[test]
fn test_extract_host_with_port() {
    assert_eq!(extract_host("https://example.com:8080/path"), "example.com");
}

#[test]
fn test_extract_host_with_query() {
    assert_eq!(extract_host("https://example.com?q=foo"), "example.com");
}

// ---------------------------------------------------------------------------
// parse_duckduckgo_html
// ---------------------------------------------------------------------------

#[test]
fn test_parse_ddg_html_with_one_result() {
    // Minimal fixture reproducing DDG's result__a / result__snippet structure.
    let html = r##"
<html>
<body>
<div class="result">
  <h2 class="result__title">
    <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%2F">
      The Rust Programming Language
    </a>
  </h2>
  <a class="result__snippet" href="...">A language empowering everyone to build reliable &amp; efficient software.</a>
</div>
</body>
</html>"##;

    let results = parse_duckduckgo_html(html, 10);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].url, "https://rust-lang.org/");
    assert!(results[0].title.contains("Rust"));
    let snippet = results[0].snippet.as_deref().unwrap_or("");
    assert!(snippet.contains("language empowering"));
    // HTML entity should be decoded.
    assert!(snippet.contains('&'));
    assert!(!snippet.contains("&amp;"));
}

#[test]
fn test_parse_ddg_html_empty_returns_empty() {
    let html = "<html><body>no results here</body></html>";
    let results = parse_duckduckgo_html(html, 10);
    assert!(results.is_empty());
}

/// Regression: the previous parser hard-capped at `SEARCH_MAX_RESULTS *
/// 2 = 16` regardless of the caller's `max_results`. With the schema
/// allowing up to 20, that under-fetched on requests beyond 16. The
/// dynamic cap should now scale the over-fetch with `max_results`.
#[test]
fn test_parse_ddg_html_respects_max_results_above_old_cap() {
    // 20 results in the HTML; ask for max_results=20 (schema ceiling).
    // Parser over-fetches by 2x but is bounded by SEARCH_MAX_RESULTS_CEILING
    // (=20) so we cap at min(20, 20) * 2 = 40 — well above 20.
    let mut html = String::new();
    for i in 0..20 {
        html.push_str(&format!(
            "<a class=\"result__a\" href=\"//duckduckgo.com/l/?uddg=https%3A%2F%2Fhost{i}.com\">Title {i}</a>\n\
             <a class=\"result__snippet\" href=\"x\">snip {i}</a>\n"
        ));
    }
    let results = parse_duckduckgo_html(&html, 20);
    // We expect at least the requested count to make it through the parser.
    // Caller (`execute()`) does the final `.take(max_results)`.
    assert!(
        results.len() >= 20,
        "parser under-fetched: got {} results for max_results=20",
        results.len()
    );
}

#[test]
fn test_parse_ddg_html_small_max_results_caps_early() {
    // 10 results in the HTML; request max_results=2 → over-fetch cap
    // is 4. Anything past that is wasted parsing work.
    let mut html = String::new();
    for i in 0..10 {
        html.push_str(&format!(
            "<a class=\"result__a\" href=\"//duckduckgo.com/l/?uddg=https%3A%2F%2Fhost{i}.com\">Title {i}</a>\n\
             <a class=\"result__snippet\" href=\"x\">snip {i}</a>\n"
        ));
    }
    let results = parse_duckduckgo_html(&html, 2);
    // Over-fetch is 2x → up to 4. May be exactly 4 (loop break checks
    // *after* push) or fewer if HTML was thin; never more than 4.
    assert!(
        results.len() <= 4,
        "parser over-fetched: got {} for max_results=2",
        results.len()
    );
    assert!(!results.is_empty());
}

#[test]
fn test_parse_ddg_html_multiple_results() {
    // Two results to verify title/snippet pairing is by index.
    let html = r##"
<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.com">First</a>
<a class="result__snippet" href="x">snippet one</a>
<a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fb.com">Second</a>
<a class="result__snippet" href="x">snippet two</a>
"##;
    let results = parse_duckduckgo_html(html, 10);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].url, "https://a.com");
    assert_eq!(results[1].url, "https://b.com");
    assert_eq!(results[0].snippet.as_deref(), Some("snippet one"));
    assert_eq!(results[1].snippet.as_deref(), Some("snippet two"));
}

// ---------------------------------------------------------------------------
// WebSearchTool trait contract
// ---------------------------------------------------------------------------

#[test]
fn test_websearch_is_read_only() {
    assert!(<WebSearchTool as DynTool>::is_read_only(
        &WebSearchTool,
        &json!({"query": "x"})
    ));
}

#[test]
fn test_websearch_is_concurrency_safe() {
    assert!(<WebSearchTool as DynTool>::is_concurrency_safe(
        &WebSearchTool,
        &json!({"query": "x"})
    ));
}

#[tokio::test]
async fn test_websearch_rejects_short_query() {
    let ctx = ToolUseContext::test_default();
    let vr =
        <WebSearchTool as DynTool>::validate_input(&WebSearchTool, &json!({"query": "a"}), &ctx);
    assert!(matches!(
        vr,
        coco_tool_runtime::ValidationResult::Invalid { .. }
    ));
}

#[tokio::test]
async fn test_websearch_rejects_both_filters() {
    let ctx = ToolUseContext::test_default();
    let vr = <WebSearchTool as DynTool>::validate_input(
        &WebSearchTool,
        &json!({
            "query": "rust",
            "allowed_domains": ["rust-lang.org"],
            "blocked_domains": ["example.com"],
        }),
        &ctx,
    );
    assert!(matches!(
        vr,
        coco_tool_runtime::ValidationResult::Invalid { .. }
    ));
}

#[tokio::test]
async fn test_websearch_accepts_valid_query() {
    let ctx = ToolUseContext::test_default();
    let vr = <WebSearchTool as DynTool>::validate_input(
        &WebSearchTool,
        &json!({"query": "rust lang"}),
        &ctx,
    );
    assert!(matches!(vr, coco_tool_runtime::ValidationResult::Valid));
}

#[tokio::test]
async fn test_websearch_accepts_allowed_domains_alone() {
    let ctx = ToolUseContext::test_default();
    let vr = <WebSearchTool as DynTool>::validate_input(
        &WebSearchTool,
        &json!({"query": "rust", "allowed_domains": ["rust-lang.org"]}),
        &ctx,
    );
    assert!(matches!(vr, coco_tool_runtime::ValidationResult::Valid));
}

// ── R7-T25: websearch prompt content checks ──
//
// The tool's `prompt()` includes a "CRITICAL REQUIREMENT" block that the
// model MUST follow (always include a Sources section). Also injects the
// current month/year so the model uses the right year for recent-events
// queries.
#[tokio::test]
async fn test_websearch_prompt_includes_sources_requirement() {
    use coco_tool_runtime::PromptOptions;
    let desc = <WebSearchTool as DynTool>::prompt(&WebSearchTool, &PromptOptions::default()).await;
    assert!(
        desc.contains("CRITICAL REQUIREMENT"),
        "WebSearch prompt must include the CRITICAL REQUIREMENT block"
    );
    assert!(
        desc.contains("Sources:"),
        "WebSearch prompt must instruct model to add a Sources section"
    );
    assert!(
        desc.contains("MANDATORY"),
        "WebSearch prompt must mark the sources requirement as MANDATORY"
    );
}

#[tokio::test]
async fn test_websearch_prompt_includes_current_year() {
    use coco_tool_runtime::PromptOptions;
    let desc = <WebSearchTool as DynTool>::prompt(&WebSearchTool, &PromptOptions::default()).await;
    // Today's date is 2026 — the dynamic month/year injection should
    // include "2026" (or whatever year chrono::Local::now() reports).
    let now_year = chrono::Datelike::year(&chrono::Local::now());
    assert!(
        desc.contains(&now_year.to_string()),
        "WebSearch prompt must contain the current year ({now_year}) for date-aware queries, got:\n{desc}"
    );
}

// ---------------------------------------------------------------------------
// B2.5: WebFetch HTML→markdown + content-type detection
// ---------------------------------------------------------------------------

use super::WebFetchTool;
use super::html_to_markdown;
use super::is_html_content_type;

#[test]
fn test_is_html_content_type_positive() {
    assert!(is_html_content_type("text/html"));
    assert!(is_html_content_type("text/html; charset=utf-8"));
    assert!(is_html_content_type("application/xhtml+xml"));
    assert!(is_html_content_type("TEXT/HTML")); // case-insensitive
}

#[test]
fn test_is_html_content_type_negative() {
    assert!(!is_html_content_type("application/json"));
    assert!(!is_html_content_type("text/plain"));
    assert!(!is_html_content_type("text/markdown"));
    assert!(!is_html_content_type("")); // empty
}

#[test]
fn test_html_to_markdown_basic() {
    let html = "<html><body><h1>Title</h1><p>Hello <b>world</b>.</p></body></html>";
    let md = html_to_markdown(html);
    assert!(md.contains("Title"), "should include heading: {md}");
    assert!(md.contains("Hello"), "should include body: {md}");
    assert!(md.contains("world"), "should include body: {md}");
    // Tags should be stripped.
    assert!(!md.contains("<h1>"));
    assert!(!md.contains("<b>"));
}

#[test]
fn test_html_to_markdown_preserves_structure() {
    let html = "<ul><li>first</li><li>second</li></ul>";
    let md = html_to_markdown(html);
    assert!(md.contains("first"));
    assert!(md.contains("second"));
}

#[test]
fn test_html_to_markdown_decodes_entities() {
    let html = "<p>a &amp; b &lt; c</p>";
    let md = html_to_markdown(html);
    assert!(md.contains("a & b"), "entities decoded: {md}");
    assert!(!md.contains("&amp;"));
}

// ---------------------------------------------------------------------------
// WebFetchTool trait contract
// ---------------------------------------------------------------------------

#[test]
fn test_webfetch_is_read_only() {
    assert!(<WebFetchTool as DynTool>::is_read_only(
        &WebFetchTool,
        &json!({"url": "https://example.com", "prompt": "x"})
    ));
}

#[test]
fn test_webfetch_is_concurrency_safe() {
    assert!(<WebFetchTool as DynTool>::is_concurrency_safe(
        &WebFetchTool,
        &json!({"url": "https://example.com", "prompt": "x"})
    ));
}

#[tokio::test]
async fn test_webfetch_rejects_empty_url() {
    let ctx = ToolUseContext::test_default();
    let result = <WebFetchTool as DynTool>::execute(
        &WebFetchTool,
        json!({"url": "", "prompt": "what is this"}),
        &ctx,
    )
    .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_webfetch_rejects_empty_prompt() {
    let ctx = ToolUseContext::test_default();
    let result = <WebFetchTool as DynTool>::execute(
        &WebFetchTool,
        json!({"url": "https://example.com", "prompt": ""}),
        &ctx,
    )
    .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// D10: WebFetch URL cache (15-min TTL, session-scoped)
// ---------------------------------------------------------------------------

use super::CachedWebFetch;
use super::clear_web_fetch_cache;
use super::web_fetch_cache_get;
use super::web_fetch_cache_set;
use std::time::Instant;

/// Cache miss on a URL not in the cache returns None.
#[test]
fn test_web_fetch_cache_miss() {
    clear_web_fetch_cache();
    assert!(web_fetch_cache_get("https://not-cached.example/").is_none());
}

/// Cache hit returns the stored entry.
#[test]
fn test_web_fetch_cache_hit() {
    clear_web_fetch_cache();
    let entry = CachedWebFetch {
        markdown: "cached body".into(),
        content_type: "text/html".into(),
        was_truncated: false,
        inserted_at: Instant::now(),
    };
    web_fetch_cache_set("https://cached.example/".into(), entry);

    let hit = web_fetch_cache_get("https://cached.example/").expect("cache hit");
    assert_eq!(hit.markdown, "cached body");
    assert_eq!(hit.content_type, "text/html");
    assert!(!hit.was_truncated);
}

/// Writing the same URL twice updates the entry (LRU dedupe).
#[test]
fn test_web_fetch_cache_dedupes_on_rewrite() {
    clear_web_fetch_cache();
    web_fetch_cache_set(
        "https://dedup.example/".into(),
        CachedWebFetch {
            markdown: "v1".into(),
            content_type: "text/html".into(),
            was_truncated: false,
            inserted_at: Instant::now(),
        },
    );
    web_fetch_cache_set(
        "https://dedup.example/".into(),
        CachedWebFetch {
            markdown: "v2".into(),
            content_type: "text/html".into(),
            was_truncated: false,
            inserted_at: Instant::now(),
        },
    );
    let hit = web_fetch_cache_get("https://dedup.example/").unwrap();
    assert_eq!(hit.markdown, "v2", "rewrite must replace the old entry");
}

/// Expired entries (older than TTL) are skipped on lookup. We simulate
/// this by setting `inserted_at` to Instant::now() minus a long duration.
#[test]
fn test_web_fetch_cache_expires_stale_entries() {
    clear_web_fetch_cache();
    // Subtract 20 minutes so the entry is past the 15-min TTL.
    let stale_time = Instant::now() - std::time::Duration::from_secs(20 * 60);
    web_fetch_cache_set(
        "https://stale.example/".into(),
        CachedWebFetch {
            markdown: "expired body".into(),
            content_type: "text/html".into(),
            was_truncated: false,
            inserted_at: stale_time,
        },
    );
    // Next lookup should prune the stale entry and return None.
    let hit = web_fetch_cache_get("https://stale.example/");
    assert!(hit.is_none(), "stale entries must be evicted on lookup");
}

// ---------------------------------------------------------------------------
// B2.6: SSRF redirect guard + preapproved hosts + redirect resolution
// ---------------------------------------------------------------------------

use super::RedirectDecision;
use super::check_redirect;
use super::is_preapproved_host;
use super::is_preapproved_url;
use super::resolve_redirect_url;

/// Exact-hostname entries match only their exact host (not subdomains,
/// not parent domains).
#[test]
fn test_is_preapproved_host_exact_hostname_match() {
    // `docs.python.org` is in the list as an exact entry.
    assert!(is_preapproved_url(
        "https://docs.python.org/3/library/os.html"
    ));
    assert!(is_preapproved_url("https://react.dev/learn"));
    assert!(is_preapproved_url("https://go.dev/tour"));
    // Bare host (no path) also matches.
    assert!(is_preapproved_url("https://huggingface.co"));
}

/// Subdomains of exact-hostname entries must NOT match — the lookup is
/// exact, not a suffix check.
#[test]
fn test_is_preapproved_host_rejects_subdomains() {
    // `docs.python.org` is in the list but `sub.docs.python.org` is NOT.
    assert!(!is_preapproved_url("https://sub.docs.python.org/"));
    // `huggingface.co` is in the list but `attacker.huggingface.co` is NOT.
    // This is a security-critical test: huggingface.co allows user
    // uploads, so matching arbitrary subdomains would enable exfiltration.
    assert!(!is_preapproved_url("https://attacker.huggingface.co/"));
    // `nuget.org` is in the list; evil subdomain is not.
    assert!(!is_preapproved_url("https://evil.nuget.org/upload"));
}

/// Path-scoped entries (host + path-prefix) enforce segment boundary.
/// `github.com/anthropics` must match `github.com/anthropics` and
/// `github.com/anthropics/claude-code` but NOT `github.com/anthropics-evil`.
#[test]
fn test_is_preapproved_host_path_scoped_exact() {
    assert!(is_preapproved_url("https://github.com/anthropics"));
}

#[test]
fn test_is_preapproved_host_path_scoped_segment() {
    // `.../anthropics/claude-code` must match because the next char after
    // `/anthropics` is `/`.
    assert!(is_preapproved_url(
        "https://github.com/anthropics/claude-code"
    ));
    assert!(is_preapproved_url(
        "https://github.com/anthropics/claude-code/pull/42"
    ));
}

#[test]
fn test_is_preapproved_host_path_scoped_rejects_sibling() {
    // SECURITY: path segment boundary. `github.com/anthropics-evil` must
    // NOT match the `github.com/anthropics` entry — attacker could register
    // that org and exfiltrate data if we naively did a prefix match.
    assert!(!is_preapproved_url("https://github.com/anthropics-evil"));
    assert!(!is_preapproved_url(
        "https://github.com/anthropics-evil/malware"
    ));
}

#[test]
fn test_is_preapproved_host_path_scoped_rejects_unrelated_host() {
    // `github.com/anthropics` is path-scoped; `github.com/other-org` must
    // NOT match (the host matches but the path doesn't).
    assert!(!is_preapproved_url("https://github.com/other-org"));
    assert!(!is_preapproved_url("https://github.com"));
}

#[test]
fn test_is_preapproved_host_rejects_unknown() {
    assert!(!is_preapproved_url("https://example.com/"));
    assert!(!is_preapproved_url("https://malicious.tld/"));
    // Suffix-match trick: "docs.python.org.evil.tld" — must not match.
    assert!(!is_preapproved_url("https://docs.python.org.evil.tld/"));
}

#[test]
fn test_is_preapproved_host_malformed_returns_false() {
    assert!(!is_preapproved_url(""));
    // "not a url" has no scheme → extract_host returns the whole string,
    // no entry matches.
    assert!(!is_preapproved_host("", "/"));
}

#[test]
fn test_is_preapproved_host_direct_args() {
    // Direct 2-arg form — useful for the permission layer when it has
    // already parsed the URL.
    assert!(is_preapproved_host("docs.python.org", "/3/library/os.html"));
    assert!(is_preapproved_host("github.com", "/anthropics/claude-code"));
    assert!(!is_preapproved_host("github.com", "/anthropics-evil"));
}

/// Vercel.com has a path-scoped entry `vercel.com/docs`.
#[test]
fn test_is_preapproved_host_vercel_docs_path_scoped() {
    assert!(is_preapproved_url("https://vercel.com/docs"));
    assert!(is_preapproved_url("https://vercel.com/docs/deployments"));
    // Non-docs paths must NOT match.
    assert!(!is_preapproved_url("https://vercel.com/pricing"));
    assert!(!is_preapproved_url("https://vercel.com/docs-evil"));
}

/// WebFetchTool::check_permissions auto-allows a preapproved docs host,
/// short-circuiting the approval prompt.
#[tokio::test]
async fn test_webfetch_check_permissions_allows_preapproved_host() {
    let ctx = ToolUseContext::test_default();
    let result = <WebFetchTool as DynTool>::check_permissions(
        &WebFetchTool,
        &json!({"url": "https://docs.python.org/3/library/os.html", "prompt": "x"}),
        &ctx,
    )
    .await;
    assert!(matches!(result, coco_types::ToolCheckResult::Allow { .. }));
}

/// A non-preapproved host with no matching rule prompts per-domain
/// (`Ask` carrying the always-allow-this-domain suggestion).
#[tokio::test]
async fn test_webfetch_check_permissions_asks_for_other_host() {
    let ctx = ToolUseContext::test_default();
    let result = <WebFetchTool as DynTool>::check_permissions(
        &WebFetchTool,
        &json!({"url": "https://example.com/", "prompt": "x"}),
        &ctx,
    )
    .await;
    assert!(matches!(result, coco_types::ToolCheckResult::Ask { .. }));
}

// ---------------------------------------------------------------------------
// check_redirect — the core SSRF guard
// ---------------------------------------------------------------------------

#[test]
fn test_check_redirect_same_origin_allowed() {
    assert_eq!(
        check_redirect("https://example.com/a", "https://example.com/b"),
        RedirectDecision::Allow
    );
}

#[test]
fn test_check_redirect_www_toggle_allowed() {
    assert_eq!(
        check_redirect("https://example.com/", "https://www.example.com/"),
        RedirectDecision::Allow
    );
    assert_eq!(
        check_redirect("https://www.example.com/", "https://example.com/"),
        RedirectDecision::Allow
    );
}

#[test]
fn test_check_redirect_cross_origin_blocked() {
    let decision = check_redirect("https://example.com/", "https://attacker.com/drop");
    match decision {
        RedirectDecision::CrossOrigin { new_url } => {
            assert_eq!(new_url, "https://attacker.com/drop");
        }
        _ => panic!("cross-origin must be blocked, got Allow"),
    }
}

/// SSRF: redirect to a metadata service must be blocked as cross-origin.
/// This is the attack scenario the explicit guard exists to prevent.
#[test]
fn test_check_redirect_metadata_service_blocked() {
    let decision = check_redirect(
        "https://example.com/",
        "http://169.254.169.254/latest/meta-data/",
    );
    assert!(matches!(decision, RedirectDecision::CrossOrigin { .. }));
}

// ---------------------------------------------------------------------------
// R1 regression guards — `isPermittedRedirect` has FOUR checks, not one.
// The round-2 verification found that the earlier check_redirect only
// implemented the host-equivalence check (#4). These tests lock in #1-#3.
// ---------------------------------------------------------------------------

/// R1-a: Protocol downgrade must be blocked. `https://example.com/` →
/// `http://example.com/` is a clear TLS downgrade attempt.
#[test]
fn test_check_redirect_protocol_downgrade_blocked() {
    let decision = check_redirect("https://example.com/", "http://example.com/");
    assert!(
        matches!(decision, RedirectDecision::CrossOrigin { .. }),
        "https → http downgrade must be blocked"
    );
}

/// R1-a: Protocol upgrade is also treated as a cross-origin change.
/// Less dangerous than downgrade but still something to surface to the
/// model, not silently follow.
#[test]
fn test_check_redirect_protocol_upgrade_blocked() {
    let decision = check_redirect("http://example.com/", "https://example.com/");
    assert!(matches!(decision, RedirectDecision::CrossOrigin { .. }));
}

/// R1-b: Port change on the same host must be blocked. Without the port
/// check, a malicious server at `example.com:443` could redirect to
/// `example.com:9999` and bypass same-origin.
#[test]
fn test_check_redirect_port_change_blocked() {
    let decision = check_redirect("https://example.com:443/", "https://example.com:9999/");
    assert!(
        matches!(decision, RedirectDecision::CrossOrigin { .. }),
        "port change must be blocked as cross-origin"
    );
}

/// R1-b: Default port vs explicit default port compare equal. For HTTPS
/// that's `example.com` == `example.com:443`.
#[test]
fn test_check_redirect_default_port_equals_implicit() {
    let decision = check_redirect("https://example.com/", "https://example.com:443/path");
    assert_eq!(decision, RedirectDecision::Allow);
}

#[test]
fn test_check_redirect_http_default_port_equals_implicit() {
    let decision = check_redirect("http://example.com/", "http://example.com:80/");
    assert_eq!(decision, RedirectDecision::Allow);
}

/// R1-b: Non-default port specified on both sides is allowed if equal.
#[test]
fn test_check_redirect_same_explicit_port_allowed() {
    let decision = check_redirect(
        "https://example.com:8443/old",
        "https://example.com:8443/new",
    );
    assert_eq!(decision, RedirectDecision::Allow);
}

/// R1-c: Redirect with userinfo (`user:pass@host`) must be blocked —
/// the server is attempting credential injection.
#[test]
fn test_check_redirect_with_userinfo_blocked() {
    let decision = check_redirect("https://example.com/", "https://attacker:pwd@example.com/");
    assert!(
        matches!(decision, RedirectDecision::CrossOrigin { .. }),
        "userinfo in redirect target must be blocked"
    );
}

/// R1-c: Plain `user@host` (no password) is still userinfo and blocked.
#[test]
fn test_check_redirect_username_only_blocked() {
    let decision = check_redirect("https://example.com/", "https://admin@example.com/");
    assert!(matches!(decision, RedirectDecision::CrossOrigin { .. }));
}

// ---------------------------------------------------------------------------
// split_host_port + has_userinfo + extract_scheme direct tests
// ---------------------------------------------------------------------------

use super::extract_scheme;
use super::has_userinfo;
use super::split_host_port;

#[test]
fn test_extract_scheme_basic() {
    assert_eq!(extract_scheme("https://example.com/"), "https");
    assert_eq!(extract_scheme("HTTP://example.com/"), "http");
    assert_eq!(extract_scheme("ftp://example.com"), "ftp");
    assert_eq!(extract_scheme("no-scheme-here"), "");
}

#[test]
fn test_split_host_port_no_explicit_port() {
    let (host, port) = split_host_port("https://example.com/path", "https");
    assert_eq!(host, "example.com");
    assert_eq!(port, None);
}

#[test]
fn test_split_host_port_explicit_default() {
    // Explicit default port normalizes to None so it compares equal to
    // the implicit-default case.
    let (host, port) = split_host_port("https://example.com:443/", "https");
    assert_eq!(host, "example.com");
    assert_eq!(port, None);
}

#[test]
fn test_split_host_port_explicit_custom() {
    let (host, port) = split_host_port("https://example.com:8443/", "https");
    assert_eq!(host, "example.com");
    assert_eq!(port, Some(8443));
}

#[test]
fn test_split_host_port_with_userinfo() {
    let (host, port) = split_host_port("https://user:pass@example.com:9000/", "https");
    assert_eq!(host, "example.com");
    assert_eq!(port, Some(9000));
}

// ---------------------------------------------------------------------------
// T1 regression guards — IPv6 brackets + port-parse failure + userinfo in
// extract_host. Round-3 verification found all three were broken.
// ---------------------------------------------------------------------------

/// T1-a: Bracketed IPv6 literal with explicit port. The naive
/// `rsplit_once(':')` would split on a colon inside the address; the
/// bracket fast path finds `]` first and only parses text after `]:`.
#[test]
fn test_split_host_port_ipv6_bracketed_with_port() {
    let (host, port) = split_host_port("https://[::1]:8080/path", "https");
    assert_eq!(host, "[::1]");
    assert_eq!(port, Some(8080));
}

/// T1-a: Bracketed IPv6 literal without port. Should preserve the full
/// `[::1]` bracketed form as the host.
#[test]
fn test_split_host_port_ipv6_bracketed_no_port() {
    let (host, port) = split_host_port("https://[::1]/path", "https");
    assert_eq!(host, "[::1]");
    assert_eq!(port, None);
}

/// T1-a: Bracketed IPv6 with default HTTPS port normalizes to None.
#[test]
fn test_split_host_port_ipv6_default_port_normalized() {
    let (host, port) = split_host_port("https://[::1]:443/", "https");
    assert_eq!(host, "[::1]");
    assert_eq!(port, None, "default https port 443 normalizes to None");
}

/// T1-a: Longer IPv6 literal with a non-trivial address.
#[test]
fn test_split_host_port_ipv6_full_address() {
    let (host, port) = split_host_port("https://[2001:db8::1]:8443/foo", "https");
    assert_eq!(host, "[2001:db8::1]");
    assert_eq!(port, Some(8443));
}

/// T1-b: Port > 65535 must strip the bad suffix and return `port=None`.
/// Previously returned `host=example.com:99999 port=None` (silent
/// corruption). Now returns `host=example.com port=None`, matching
/// the RFC notion of an invalid port.
#[test]
fn test_split_host_port_port_over_u16_max() {
    let (host, port) = split_host_port("https://example.com:99999/", "https");
    assert_eq!(
        host, "example.com",
        "host must NOT include the bad :99999 suffix"
    );
    assert_eq!(port, None, "unparseable port becomes None");
}

/// T1-b: Non-numeric text after `:` isn't a port at all — could be
/// anything (e.g. a typo), so we leave the host as-is and return None.
/// This is the `rsplit_once` path where the digits check fails.
#[test]
fn test_split_host_port_non_numeric_after_colon() {
    let (host, port) = split_host_port("https://example.com:abc/", "https");
    // The colon isn't followed by digits, so we don't treat it as a port.
    // The whole `example.com:abc` becomes the host (as before T1 — this
    // path wasn't affected by T1 because we only changed the digit branch).
    assert_eq!(host, "example.com:abc");
    assert_eq!(port, None);
}

/// T1-c: `extract_host` must strip userinfo. Previously it kept the
/// `user@` prefix, causing the host-comparison path to see different
/// hosts depending on whether the URL had userinfo.
#[test]
fn test_extract_host_strips_userinfo() {
    use super::extract_host;
    assert_eq!(
        extract_host("https://user@example.com/"),
        "example.com",
        "userinfo must be stripped from extract_host result"
    );
    assert_eq!(
        extract_host("https://user:pass@example.com:8080/path"),
        "example.com"
    );
}

/// T1-c: `extract_host` must also handle bracketed IPv6.
#[test]
fn test_extract_host_ipv6_bracketed() {
    use super::extract_host;
    assert_eq!(extract_host("https://[::1]:8080/"), "[::1]");
    assert_eq!(extract_host("https://[2001:db8::1]/path"), "[2001:db8::1]");
}

/// T1-c: `extract_host` with userinfo + IPv6 + port — everything at once.
#[test]
fn test_extract_host_ipv6_with_userinfo_and_port() {
    use super::extract_host;
    assert_eq!(
        extract_host("https://user:pass@[::1]:9000/path"),
        "[::1]",
        "userinfo stripped, IPv6 brackets preserved, port removed"
    );
}

// ---------------------------------------------------------------------------
// T1: check_redirect regression — the port-change attack test from R1
// must still pass after the IPv6 fix, AND IPv6 same-host redirects must
// be correctly recognized as same-origin.
// ---------------------------------------------------------------------------

/// IPv6 same-origin redirect must be allowed.
#[test]
fn test_check_redirect_ipv6_same_origin_allowed() {
    let decision = check_redirect("https://[::1]:8080/a", "https://[::1]:8080/b");
    assert_eq!(decision, RedirectDecision::Allow);
}

/// IPv6 port-change attack must be blocked.
#[test]
fn test_check_redirect_ipv6_port_change_blocked() {
    let decision = check_redirect("https://[::1]:8080/", "https://[::1]:9999/");
    assert!(
        matches!(decision, RedirectDecision::CrossOrigin { .. }),
        "IPv6 port change must be blocked like IPv4 port change"
    );
}

/// IPv6 default port normalization: `[::1]` and `[::1]:443` under https
/// must compare equal.
#[test]
fn test_check_redirect_ipv6_default_port_equivalent() {
    let decision = check_redirect("https://[::1]/", "https://[::1]:443/path");
    assert_eq!(decision, RedirectDecision::Allow);
}

#[test]
fn test_has_userinfo_positive() {
    assert!(has_userinfo("https://user@example.com/"));
    assert!(has_userinfo("https://user:pass@example.com/"));
    assert!(has_userinfo("https://admin@example.com:8080/"));
}

#[test]
fn test_has_userinfo_negative() {
    assert!(!has_userinfo("https://example.com/"));
    assert!(!has_userinfo("https://example.com/path"));
    // `@` in path or query is not userinfo.
    assert!(!has_userinfo("https://example.com/users/@me"));
    assert!(!has_userinfo("https://example.com/?q=a@b"));
}

#[test]
fn test_check_redirect_subdomain_is_cross_origin() {
    // docs.example.com != example.com — we only allow www. toggling.
    let decision = check_redirect("https://example.com/", "https://docs.example.com/");
    assert!(matches!(decision, RedirectDecision::CrossOrigin { .. }));
}

// ---------------------------------------------------------------------------
// resolve_redirect_url
// ---------------------------------------------------------------------------

#[test]
fn test_resolve_redirect_absolute() {
    assert_eq!(
        resolve_redirect_url("https://example.com/a", "https://other.com/b"),
        "https://other.com/b"
    );
}

#[test]
fn test_resolve_redirect_protocol_relative() {
    assert_eq!(
        resolve_redirect_url("https://example.com/a", "//cdn.example.com/asset.js"),
        "https://cdn.example.com/asset.js"
    );
}

#[test]
fn test_resolve_redirect_absolute_path() {
    assert_eq!(
        resolve_redirect_url("https://example.com/old/path", "/new/path"),
        "https://example.com/new/path"
    );
}

#[test]
fn test_resolve_redirect_relative_path() {
    // Relative to current directory — last `/` is the dir boundary.
    assert_eq!(
        resolve_redirect_url("https://example.com/dir/old", "new"),
        "https://example.com/dir/new"
    );
}

// ---------------------------------------------------------------------------
// render_for_model — picks the right body field per execution branch
// ---------------------------------------------------------------------------

#[test]
fn webfetch_render_picks_extracted_when_llm_extraction_succeeded() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({
        "url": "https://example.com",
        "prompt": "what is the API?",
        "extracted": "The API is documented at /docs.",
        "truncated": false,
        "extraction_mode": "llm",
    });
    let parts = <WebFetchTool as DynTool>::render_for_model(&WebFetchTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "The API is documented at /docs.");
}

#[test]
fn webfetch_render_falls_back_to_content_when_extraction_unavailable() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({
        "url": "https://example.com",
        "prompt": "what?",
        "content": "# Page Title\n\nRaw markdown body.",
        "truncated": false,
        "extraction_mode": "raw",
    });
    let parts = <WebFetchTool as DynTool>::render_for_model(&WebFetchTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "# Page Title\n\nRaw markdown body.");
}

#[test]
fn webfetch_render_emits_redirect_blocked_message() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!({
        "url": "https://example.com",
        "prompt": "what?",
        "redirect_blocked": true,
        "new_url": "https://other.example.com/",
        "message": "The URL redirected to a different origin (https://other.example.com/). Please use WebFetch again.",
    });
    let parts = <WebFetchTool as DynTool>::render_for_model(&WebFetchTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert!(text.contains("redirected"), "got: {text}");
    assert!(text.contains("other.example.com"), "got: {text}");
}

// ---------------------------------------------------------------------------
// #57 — binary content persistence helpers
// ---------------------------------------------------------------------------

#[test]
fn test_mime_to_extension() {
    use super::mime_to_extension;
    assert_eq!(mime_to_extension("image/png"), "png");
    assert_eq!(mime_to_extension("image/jpeg; charset=binary"), "jpg");
    assert_eq!(mime_to_extension("application/zip"), "zip");
    assert_eq!(mime_to_extension("application/octet-stream"), "bin");
}

#[test]
fn test_persist_binary_content_writes_file() {
    use super::persist_binary_content;
    let bytes = b"\x89PNG\r\n\x1a\nfake-image-bytes";
    let (path, size) = persist_binary_content(bytes, "image/png").unwrap();
    assert_eq!(size, bytes.len());
    assert!(path.ends_with(".png"), "path should have png ext: {path}");
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(on_disk, bytes);
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// WebFetch schema requires both url and prompt
// ---------------------------------------------------------------------------

#[test]
fn webfetch_schema_requires_url_and_prompt() {
    let schema = coco_tool_runtime::Tool::runtime_validation_schema(&WebFetchTool);
    assert!(schema.validate(&json!({"prompt": "summarize"})).is_err());
    assert!(schema.validate(&json!({"url": "https://x"})).is_err());
    assert!(
        schema
            .validate(&json!({"url": "https://x", "prompt": "y"}))
            .is_ok()
    );
    // additionalProperties:false still rejects unknown keys.
    assert!(
        schema
            .validate(&json!({"url": "https://x", "prompt": "y", "extra": 1}))
            .is_err()
    );
}
