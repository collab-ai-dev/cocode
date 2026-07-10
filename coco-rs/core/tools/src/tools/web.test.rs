//! Tests for WebSearch. Pure-function tests only (no network calls).

use super::WebSearchTool;
use super::decode_ddg_redirect;
use super::decode_html_entities;
use super::extract_host;
use super::parse_duckduckgo_html;
use super::percent_decode;
use super::strip_html_tags;
use coco_tool_runtime::DescriptionOptions;
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
fn test_websearch_description_uses_generic_agent_name() {
    let desc = <WebSearchTool as DynTool>::description(
        &WebSearchTool,
        &json!({"query": "rust async cancellation"}),
        &DescriptionOptions::default(),
    );
    assert_eq!(
        desc,
        "The agent wants to search the web for: rust async cancellation"
    );
    assert!(!desc.contains("Claude"));
}

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
fn test_webfetch_description_uses_generic_agent_name() {
    let desc = <WebFetchTool as DynTool>::description(
        &WebFetchTool,
        &json!({"url": "https://example.com/docs", "prompt": "summarize"}),
        &DescriptionOptions::default(),
    );
    assert_eq!(desc, "The agent wants to fetch content from example.com");
    assert!(!desc.contains("Claude"));
}

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

// ---------------------------------------------------------------------------
// D10: WebFetch URL cache (15-min TTL, session-scoped)
// ---------------------------------------------------------------------------

use super::CachedWebFetch;
use super::web_fetch_cache_get;
use super::web_fetch_cache_get_rendered;
use super::web_fetch_cache_insert_at;
use super::web_fetch_cache_key;
use super::web_fetch_cache_set;
use std::time::Instant;

// NOTE: the cache is a process-global static; these tests share it and run in
// parallel. Each uses a UNIQUE URL/session so entries never collide, and none
// clears the shared cache (which would race-wipe a sibling's entry).

/// Cache miss on a key not in the cache returns None.
#[test]
fn test_web_fetch_cache_miss() {
    let key = web_fetch_cache_key("miss-sess", "https://not-cached.example/", 15_000);
    assert!(web_fetch_cache_get(&key).is_none());
}

/// Cache hit returns the stored rendered envelope.
#[test]
fn test_web_fetch_cache_hit() {
    let key = web_fetch_cache_key("hit-sess", "https://cached.example/", 15_000);
    web_fetch_cache_set(
        key.clone(),
        CachedWebFetch::Rendered(json!({ "content": "cached body" })),
    );

    let hit = web_fetch_cache_get_rendered(&key).expect("cache hit");
    assert_eq!(hit["content"], "cached body");
}

/// Llm-arm entries cache the extraction SOURCE, not a rendered answer — a hit
/// must yield the source so the extraction re-runs with the live prompt.
#[test]
fn test_web_fetch_cache_llm_source_round_trips() {
    let key = web_fetch_cache_key("llm-sess", "https://llm.example/", 15_000);
    web_fetch_cache_set(
        key.clone(),
        CachedWebFetch::LlmSource {
            extract_input: "bounded markdown".into(),
            notes: String::new(),
            page_capped: false,
        },
    );
    match web_fetch_cache_get(&key) {
        Some(CachedWebFetch::LlmSource { extract_input, .. }) => {
            assert_eq!(extract_input, "bounded markdown");
        }
        other => panic!("expected LlmSource hit, got {:?}", other.is_some()),
    }
    // And the rendered-view accessor must NOT serve it as a final answer.
    assert!(web_fetch_cache_get_rendered(&key).is_none());
}

/// A budget-mismatched call is a cache miss (part of the key).
#[test]
fn test_web_fetch_cache_budget_mismatch_is_miss() {
    let k1 = web_fetch_cache_key("budget-sess", "https://u.example/", 15_000);
    web_fetch_cache_set(k1, CachedWebFetch::Rendered(json!({ "content": "a" })));
    let k2 = web_fetch_cache_key("budget-sess", "https://u.example/", 4_000);
    assert!(web_fetch_cache_get(&k2).is_none());
}

/// Writing the same key twice updates the entry (LRU dedupe).
#[test]
fn test_web_fetch_cache_dedupes_on_rewrite() {
    let key = web_fetch_cache_key("dedup-sess", "https://dedup.example/", 15_000);
    web_fetch_cache_set(
        key.clone(),
        CachedWebFetch::Rendered(json!({ "content": "v1" })),
    );
    web_fetch_cache_set(
        key.clone(),
        CachedWebFetch::Rendered(json!({ "content": "v2" })),
    );
    let hit = web_fetch_cache_get_rendered(&key).unwrap();
    assert_eq!(hit["content"], "v2", "rewrite must replace the old entry");
}

/// Expired entries (older than TTL) are skipped on lookup.
#[test]
fn test_web_fetch_cache_expires_stale_entries() {
    let key = web_fetch_cache_key("stale-sess", "https://stale.example/", 15_000);
    // Subtract 20 minutes so the entry is past the 15-min TTL.
    let stale_time = Instant::now() - std::time::Duration::from_secs(20 * 60);
    web_fetch_cache_insert_at(key.clone(), json!({ "content": "expired" }), stale_time);
    assert!(
        web_fetch_cache_get(&key).is_none(),
        "stale entries must be evicted on lookup"
    );
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

// MIME→extension mapping is shared with the session store; its table test
// lives in `coco_tool_runtime::tool_result_storage`.

#[test]
fn test_persist_binary_content_writes_file() {
    use super::persist_binary_content;
    let bytes = b"\x89PNG\r\n\x1a\nfake-image-bytes";
    let path = persist_binary_content(bytes, "image/png").unwrap();
    assert!(path.ends_with(".png"), "path should have png ext: {path}");
    let on_disk = std::fs::read(&path).unwrap();
    assert_eq!(on_disk, bytes);
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// WebFetch schema requires both url and prompt
// ---------------------------------------------------------------------------

#[test]
fn webfetch_schema_requires_only_url_prompt_optional() {
    let schema = coco_tool_runtime::Tool::runtime_validation_schema(&WebFetchTool);
    // Only the URL is mandatory now; the extraction prompt is optional and
    // inert on the windowed path (schema honesty).
    assert!(schema.validate(&json!({"prompt": "summarize"})).is_err());
    assert!(schema.validate(&json!({"url": "https://x"})).is_ok());
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

// ===========================================================================
// Integration tests — the WebFetch offload pipeline end-to-end.
//
// Two layers, because `execute()` force-upgrades `http://`→`https://` and a
// plain-HTTP wiremock server is unreachable through the full path:
//   1. `render_fetched` — the post-fetch offload pipeline (markdown → defuse →
//      wrap → dispatch → persist → cache), driven with synthetic bodies.
//   2. `fetch_url` — the HTTP boundary, driven against a wiremock server.
// Together they cover the design's §10 integration list without fighting TLS.
// ===========================================================================

mod integ {
    use super::super::FetchOutcome;
    use super::super::FetchedBody;
    use super::super::RenderInputs;
    use super::super::content_addressed_key;
    use super::super::fetch_url;
    use super::super::render_fetched;
    use super::super::web_fetch_cache_key;
    use coco_config::WebFetchExtraction;
    use coco_tool_runtime::InlineBudget;
    use coco_tool_runtime::ToolOutputStore;
    use coco_tool_runtime::ToolUseContext;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use wiremock::MockServer;
    use wiremock::matchers::method;
    use wiremock::matchers::path as wm_path;
    use wiremock::{Mock, ResponseTemplate};

    /// WebFetchTool's declared Level-1 threshold (pinned by
    /// `webfetch_declares_high_bound_for_preapproved_verbatim`).
    const WF_THRESHOLD: i64 = 102_000;

    /// A `SideQuery` double that records call count and returns a canned answer.
    #[derive(Clone)]
    struct RecordingSideQuery {
        calls: Arc<AtomicUsize>,
        answer: String,
    }

    #[async_trait::async_trait]
    impl coco_tool_runtime::side_query::SideQuery for RecordingSideQuery {
        async fn query(
            &self,
            _request: coco_tool_runtime::SideQueryRequest,
        ) -> Result<coco_tool_runtime::SideQueryResponse, coco_error::BoxedError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(coco_tool_runtime::SideQueryResponse {
                text: Some(self.answer.clone()),
                tool_uses: Vec::new(),
                stop_reason: coco_types::SideQueryStopReason::EndTurn,
                usage: coco_types::SideQueryUsage::default(),
                model_used: "stub".into(),
            })
        }
        fn model_id(&self) -> &str {
            "stub"
        }
    }

    /// Build a ToolUseContext with a recording side-query, a store rooted at
    /// `store_dir`, and the given extraction mode. Returns `(ctx, call_count)`.
    fn ctx_with(
        store_dir: &Path,
        extraction: WebFetchExtraction,
    ) -> (ToolUseContext, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut ctx = ToolUseContext::test_default();
        ctx.web_fetch_config = coco_config::WebFetchConfig {
            extraction,
            ..Default::default()
        };
        ctx.tool_output_store = Some(ToolOutputStore::new(store_dir));
        ctx.side_query = Arc::new(RecordingSideQuery {
            calls: calls.clone(),
            answer: "canned extraction answer".into(),
        });
        (ctx, calls)
    }

    fn body(content_type: &str, text: impl Into<String>) -> FetchedBody {
        FetchedBody {
            body: text.into(),
            content_type: content_type.into(),
            binary: None,
        }
    }

    /// Drive `render_fetched` with the tool's real declared threshold.
    async fn render(
        ctx: &ToolUseContext,
        url: &str,
        fetched: FetchedBody,
        live_prompt: Option<&str>,
        is_preapproved: bool,
        budget: InlineBudget,
        cache_key: &str,
    ) -> serde_json::Value {
        render_fetched(
            ctx,
            fetched,
            RenderInputs {
                url,
                live_prompt,
                is_preapproved,
                requested_budget: budget,
                threshold: WF_THRESHOLD,
                cache_key,
            },
        )
        .await
    }

    fn artifacts(dir: &Path) -> Vec<std::path::PathBuf> {
        let tr = dir.join("tool-results");
        match std::fs::read_dir(&tr) {
            Ok(rd) => rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    !p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(".tmp-")
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── render_fetched ──────────────────────────────────────────────────

    #[tokio::test]
    async fn small_page_is_verbatim_zero_sidequery_zero_persist() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, calls) = ctx_with(tmp.path(), WebFetchExtraction::Auto);
        let text = "hello world\n".repeat(200); // ~2.4K < 15K default budget
        let key = web_fetch_cache_key("s1", "https://ex.test/small", 15_000);
        let data = render(
            &ctx,
            "https://ex.test/small",
            body("text/plain", text.clone()),
            None,
            false,
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;

        assert_eq!(data["extraction_mode"], "verbatim");
        assert_eq!(data["truncated"], false);
        assert!(data["content"].as_str().unwrap().contains("hello world"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no side-query for verbatim"
        );
        assert!(artifacts(tmp.path()).is_empty(), "no artifact for verbatim");
    }

    #[tokio::test]
    async fn large_page_windows_persists_and_footer_read_is_navigable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, _calls) = ctx_with(tmp.path(), WebFetchExtraction::Auto);
        // 40K of distinct lines → clean (text/plain) → windowed at 15K.
        let text = (0..5_000)
            .map(|i| format!("row {i} lorem ipsum dolor"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.len() > 30_000);
        let key = web_fetch_cache_key("s2", "https://ex.test/big", 15_000);
        let data = render(
            &ctx,
            "https://ex.test/big",
            body("text/plain", text),
            None,
            false,
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;

        assert_eq!(data["extraction_mode"], "windowed");
        assert_eq!(data["truncated"], true);
        let content = data["content"].as_str().unwrap();
        // Windowed layout: head first, footer at the end (pointer-bearing).
        assert!(!content.starts_with("<persisted-output>"));
        assert!(content.trim_end().ends_with("</persisted-output>"));
        assert!(content.contains("limit=200"));

        // Exactly one artifact written; it is the hard-wrapped full text.
        let files = artifacts(tmp.path());
        assert_eq!(files.len(), 1);
        let stored = std::fs::read_to_string(&files[0]).unwrap();
        assert!(stored.split('\n').all(|l| l.len() <= 400), "Read-navigable");

        // The suggested Read call lands inside the artifact: parse the offset
        // from the footer and confirm it is a valid 1-based line in the file.
        let offset_tok = content.split("offset=").nth(1).unwrap();
        let offset: usize = offset_tok
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let total_lines = stored.split('\n').count();
        assert!(
            offset >= 1 && offset <= total_lines,
            "offset {offset} in 1..={total_lines}"
        );
    }

    #[tokio::test]
    async fn changed_body_gets_new_artifact_old_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, _c) = ctx_with(tmp.path(), WebFetchExtraction::Windowed);
        let url = "https://ex.test/page";
        let big = |tag: &str| {
            (0..4_000)
                .map(|i| format!("{tag} line {i} filler text"))
                .collect::<Vec<_>>()
                .join("\n")
        };

        let key1 = web_fetch_cache_key("v1sess", url, 15_000);
        render(
            &ctx,
            url,
            body("text/plain", big("v1")),
            None,
            false,
            InlineBudget::from_request(15_000),
            &key1,
        )
        .await;
        let after_first = artifacts(tmp.path());
        assert_eq!(after_first.len(), 1);
        let first_path = after_first[0].clone();
        let first_bytes = std::fs::read(&first_path).unwrap();

        // Same URL, DIFFERENT body → content-addressed name differs → new file.
        let key2 = web_fetch_cache_key("v2sess", url, 15_000);
        render(
            &ctx,
            url,
            body("text/plain", big("v2")),
            None,
            false,
            InlineBudget::from_request(15_000),
            &key2,
        )
        .await;
        let after_second = artifacts(tmp.path());
        assert_eq!(after_second.len(), 2, "changed content ⟹ new artifact");
        // The first artifact is byte-for-byte untouched.
        assert_eq!(std::fs::read(&first_path).unwrap(), first_bytes);
        assert_ne!(
            content_addressed_key(url, "v1 line 0 filler text"),
            content_addressed_key(url, "v2 line 0 filler text"),
        );
    }

    #[tokio::test]
    async fn data_uri_images_are_defused() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, _c) = ctx_with(tmp.path(), WebFetchExtraction::Auto);
        let blob = "A".repeat(40_000);
        let md = format!("intro text\n\n![a cat](data:image/png;base64,{blob})\n\nmore text\n");
        let key = web_fetch_cache_key("d", "https://ex.test/img", 15_000);
        let data = render(
            &ctx,
            "https://ex.test/img",
            body("text/plain", md),
            None,
            false,
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;
        // Verbatim (defused text is tiny), base64 blob gone, alt preserved.
        assert_eq!(data["extraction_mode"], "verbatim");
        let content = data["content"].as_str().unwrap();
        assert!(content.contains("[IMAGE: a cat]"));
        assert!(!content.contains(&blob));
    }

    #[tokio::test]
    async fn single_line_minified_json_wraps_to_read_navigable_artifact() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, _c) = ctx_with(tmp.path(), WebFetchExtraction::Auto);
        // 300KB single-line JSON → non-HTML → windowed; must be Read-navigable.
        let one_line = format!("{{\"data\":\"{}\"}}", "x".repeat(300_000));
        let key = web_fetch_cache_key("j", "https://ex.test/api.json", 15_000);
        let data = render(
            &ctx,
            "https://ex.test/api.json",
            body("application/json", one_line),
            None,
            false,
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;
        assert_eq!(data["extraction_mode"], "windowed");
        let files = artifacts(tmp.path());
        assert_eq!(files.len(), 1);
        let stored = std::fs::read_to_string(&files[0]).unwrap();
        assert!(
            stored.split('\n').all(|l| l.len() <= 400),
            "single line wrapped"
        );
        assert!(stored.split('\n').count() > 1);
    }

    #[tokio::test]
    async fn preapproved_markdown_under_budget_is_verbatim() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, _c) = ctx_with(tmp.path(), WebFetchExtraction::Auto);
        // A real preapproved host + text/markdown + 60K ≤ 100K preapproved budget.
        let url = "https://developer.mozilla.org/en-US/docs/Web/JavaScript";
        assert!(
            super::super::is_preapproved_url(url),
            "fixture must be preapproved"
        );
        let md = (0..4_000)
            .map(|i| format!("- doc line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(md.len() > 30_000 && md.len() < 100_000);
        let key = web_fetch_cache_key("pa", url, 15_000);
        let data = render(
            &ctx,
            url,
            body("text/markdown", md.clone()),
            None,
            true, // is_preapproved
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;
        // 80K > 15K default, but preapproved budget (100K) keeps it verbatim.
        assert_eq!(data["extraction_mode"], "preapproved_verbatim");
        assert_eq!(data["truncated"], false);
        assert!(artifacts(tmp.path()).is_empty(), "verbatim ⟹ no artifact");
    }

    #[tokio::test]
    async fn model_inline_bytes_widens_the_verbatim_window() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, _c) = ctx_with(tmp.path(), WebFetchExtraction::Auto);
        // 60K page: over the 15K default, but a model inline_bytes=500_000 →
        // clamped to 500K → capped_to(102K threshold) = 101K → verbatim, and
        // the emitted content stays under the tool's Level-1 threshold.
        let text = "z".repeat(60_000);
        let requested = InlineBudget::from_request(500_000).capped_to(WF_THRESHOLD);
        let key = web_fetch_cache_key("ib", "https://ex.test/wide", requested.get());
        let data = render(
            &ctx,
            "https://ex.test/wide",
            body("text/plain", text),
            None,
            false,
            requested,
            &key,
        )
        .await;
        assert_eq!(data["extraction_mode"], "verbatim");
        assert!(artifacts(tmp.path()).is_empty());
        assert!(
            (data["content"].as_str().unwrap().len() as i64) < WF_THRESHOLD,
            "verbatim output stays under Level-1 threshold (no re-persist)"
        );
    }

    #[tokio::test]
    async fn llm_arm_runs_extraction_and_caches_source_not_answer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (ctx, calls) = ctx_with(tmp.path(), WebFetchExtraction::Llm);
        let html = format!("<html><body>{}</body></html>", "content ".repeat(4_000));
        let key = web_fetch_cache_key("llm", "https://ex.test/html", 15_000);
        let data = render(
            &ctx,
            "https://ex.test/html",
            body("text/html", html),
            Some("what is this page about?"),
            false,
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;
        assert_eq!(data["extraction_mode"], "llm");
        assert_eq!(data["extracted"], "canned extraction answer");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "side model called once");

        // The cache stored the SOURCE, not the answer: a second call with a
        // DIFFERENT prompt re-runs extraction (fresh answer), not a stale hit.
        assert!(
            super::super::web_fetch_cache_get_rendered(&key).is_none(),
            "llm result must not be cached as a rendered answer"
        );
    }

    #[tokio::test]
    async fn llm_arm_falls_back_to_windowed_when_side_model_unavailable() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut ctx = ToolUseContext::test_default();
        ctx.web_fetch_config = coco_config::WebFetchConfig {
            extraction: WebFetchExtraction::Llm,
            ..Default::default()
        };
        ctx.tool_output_store = Some(ToolOutputStore::new(tmp.path()));
        // NoOpSideQuery errors → fallback to a windowed render of the full text.
        let html = format!("<html><body>{}</body></html>", "content ".repeat(4_000));
        let key = web_fetch_cache_key("fb", "https://ex.test/htmlfb", 15_000);
        let data = render(
            &ctx,
            "https://ex.test/htmlfb",
            body("text/html", html),
            Some("summary?"),
            false,
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;
        assert_eq!(data["extraction_mode"], "windowed_fallback");
        assert!(
            data["content"]
                .as_str()
                .unwrap()
                .trim_end()
                .ends_with("</persisted-output>")
        );
        assert_eq!(
            artifacts(tmp.path()).len(),
            1,
            "fallback persists the artifact"
        );
    }

    #[tokio::test]
    async fn store_absent_degrades_to_pointerless_window() {
        let mut ctx = ToolUseContext::test_default();
        ctx.web_fetch_config = coco_config::WebFetchConfig {
            extraction: WebFetchExtraction::Windowed,
            ..Default::default()
        };
        ctx.tool_output_store = None; // no store
        let text = (0..4_000)
            .map(|i| format!("line {i} text"))
            .collect::<Vec<_>>()
            .join("\n");
        let key = web_fetch_cache_key("ns", "https://ex.test/nostore", 15_000);
        let data = render(
            &ctx,
            "https://ex.test/nostore",
            body("text/plain", text),
            None,
            false,
            InlineBudget::from_request(15_000),
            &key,
        )
        .await;
        assert_eq!(data["extraction_mode"], "windowed");
        let content = data["content"].as_str().unwrap();
        assert!(
            content.contains("Full text not saved"),
            "pointerless footer"
        );
        assert!(content.trim_end().ends_with("</persisted-output>"));
    }

    // ── fetch_url (HTTP boundary, wiremock) ─────────────────────────────

    fn fetch_config(server: &MockServer) -> coco_config::WebFetchConfig {
        // Fast timeout so the timeout test doesn't linger; user_agent + caps
        // are the defaults. The URL passed to fetch_url is the raw http server
        // URL (execute's http→https upgrade is NOT applied at this layer).
        let _ = server;
        coco_config::WebFetchConfig {
            timeout_secs: 2,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn fetch_url_returns_html_body_and_content_type() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/page"))
            .respond_with(
                // `set_body_raw` sets the content-type explicitly (unlike
                // `set_body_string`, which forces text/plain).
                ResponseTemplate::new(200).set_body_raw(
                    "<html><body>hi</body></html>".as_bytes().to_vec(),
                    "text/html; charset=utf-8",
                ),
            )
            .mount(&server)
            .await;

        let out = fetch_url(&format!("{}/page", server.uri()), &fetch_config(&server))
            .await
            .unwrap();
        match out {
            FetchOutcome::Body {
                body,
                content_type,
                binary,
            } => {
                assert!(content_type.contains("text/html"));
                assert!(body.contains("<body>hi</body>"));
                assert!(binary.is_none());
            }
            other => panic!("expected Body, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_url_json_passthrough() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/api"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"ok":true}"#.as_bytes().to_vec(), "application/json"),
            )
            .mount(&server)
            .await;
        let out = fetch_url(&format!("{}/api", server.uri()), &fetch_config(&server))
            .await
            .unwrap();
        let FetchOutcome::Body {
            body,
            content_type,
            binary,
        } = out
        else {
            panic!("expected Body");
        };
        assert!(content_type.contains("application/json"));
        assert_eq!(body, r#"{"ok":true}"#);
        assert!(binary.is_none());
    }

    #[tokio::test]
    async fn fetch_url_detects_binary_bytes() {
        let server = MockServer::start().await;
        let png = b"\x89PNG\r\n\x1a\n\x00\x01\x02rawbytes".to_vec();
        Mock::given(method("GET"))
            .and(wm_path("/img.png"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(png.clone(), "image/png"))
            .mount(&server)
            .await;
        let out = fetch_url(&format!("{}/img.png", server.uri()), &fetch_config(&server))
            .await
            .unwrap();
        let FetchOutcome::Body {
            binary,
            content_type,
            ..
        } = out
        else {
            panic!("expected Body");
        };
        assert!(content_type.contains("image/png"));
        assert_eq!(binary.expect("binary bytes captured"), png);
    }

    #[tokio::test]
    async fn fetch_url_follows_same_origin_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/start"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "/final"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(wm_path("/final"))
            .respond_with(ResponseTemplate::new(200).set_body_string("arrived"))
            .mount(&server)
            .await;
        let out = fetch_url(&format!("{}/start", server.uri()), &fetch_config(&server))
            .await
            .unwrap();
        let FetchOutcome::Body { body, .. } = out else {
            panic!("expected Body");
        };
        assert_eq!(body, "arrived");
    }

    #[tokio::test]
    async fn fetch_url_blocks_cross_origin_redirect() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/leave"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "https://evil.example.com/steal"),
            )
            .mount(&server)
            .await;
        let out = fetch_url(&format!("{}/leave", server.uri()), &fetch_config(&server))
            .await
            .unwrap();
        match out {
            FetchOutcome::CrossOriginRedirect { new_url } => {
                assert_eq!(new_url, "https://evil.example.com/steal");
            }
            other => panic!("expected CrossOriginRedirect, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_url_errors_on_non_2xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let err = fetch_url(&format!("{}/missing", server.uri()), &fetch_config(&server))
            .await
            .unwrap_err();
        assert!(err.contains("404"), "got: {err}");
    }

    #[tokio::test]
    async fn fetch_url_times_out() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(wm_path("/slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(std::time::Duration::from_secs(5))
                    .set_body_string("late"),
            )
            .mount(&server)
            .await;
        let err = fetch_url(&format!("{}/slow", server.uri()), &fetch_config(&server))
            .await
            .unwrap_err();
        assert!(err.contains("[TIMEOUT]"), "got: {err}");
    }
}
