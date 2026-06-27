use super::*;
use pretty_assertions::assert_eq;

#[test]
fn bundled_catalog_parses_openrouter_snapshot() {
    let catalog = bundled_catalog().expect("bundled snapshot should parse");
    assert!(!catalog.is_empty());
    assert!(catalog.len() > 100);
}

#[test]
fn canonical_openrouter_id_lookup() {
    let card = lookup("anthropic/claude-sonnet-4.5").expect("canonical id should resolve");
    assert_eq!(card.canonical_id, "anthropic/claude-sonnet-4.5");
    assert_eq!(
        card.knowledge_cutoff.as_ref().map(|c| c.display.as_str()),
        Some("January 2025")
    );
}

#[test]
fn providerless_alias_resolves_to_same_card() {
    let canonical = lookup("anthropic/claude-sonnet-4.5").unwrap();
    let providerless = lookup("claude-sonnet-4-5").unwrap();
    let slug = lookup("anthropic/claude-4.5-sonnet-20250929").unwrap();

    assert_eq!(canonical.canonical_id, providerless.canonical_id);
    assert_eq!(canonical.canonical_id, slug.canonical_id);
}

#[test]
fn unknown_model_returns_none() {
    assert!(lookup("claude-opus-9-9").is_none());
    assert!(lookup("gpt-99").is_none());
    assert!(lookup("").is_none());
}

#[test]
fn no_substring_matching_for_claude_siblings() {
    let sonnet4 = lookup("anthropic/claude-sonnet-4").expect("sonnet 4 should resolve");
    assert_eq!(sonnet4.canonical_id, "anthropic/claude-sonnet-4");

    let sonnet45 = lookup("anthropic/claude-sonnet-4.5").expect("sonnet 4.5 should resolve");
    assert_eq!(sonnet45.canonical_id, "anthropic/claude-sonnet-4.5");

    assert_ne!(sonnet4.canonical_id, sonnet45.canonical_id);
}

#[test]
fn fast_anthropic_variant_does_not_steal_base_alias() {
    let base = lookup("claude-opus-4-7").expect("base opus alias should resolve");
    assert_eq!(base.canonical_id, "anthropic/claude-opus-4.7");

    let fast = lookup("claude-opus-4-7-fast").expect("fast opus alias should resolve");
    assert_eq!(fast.canonical_id, "anthropic/claude-opus-4.7-fast");
    assert_ne!(base.canonical_id, fast.canonical_id);
}

#[test]
fn no_substring_matching_for_gpt_siblings() {
    let gpt41 = lookup("openai/gpt-4.1").expect("gpt-4.1 should resolve");
    assert_eq!(gpt41.canonical_id, "openai/gpt-4.1");

    let gpt41mini = lookup("openai/gpt-4.1-mini").expect("gpt-4.1-mini should resolve");
    assert_eq!(gpt41mini.canonical_id, "openai/gpt-4.1-mini");

    assert_ne!(gpt41.canonical_id, gpt41mini.canonical_id);
}

#[test]
fn free_variant_keeps_distinct_pricing() {
    let free = lookup("qwen/qwen3-next-80b-a3b-instruct:free")
        .expect("free variant should resolve exactly");
    assert_eq!(free.canonical_id, "qwen/qwen3-next-80b-a3b-instruct:free");
    assert_eq!(
        free.pricing.as_ref().map(|p| p.input_per_million_usd),
        Some(0.0)
    );

    let paid =
        lookup("qwen/qwen3-next-80b-a3b-instruct").expect("paid variant should resolve exactly");
    assert_eq!(paid.canonical_id, "qwen/qwen3-next-80b-a3b-instruct");
    assert_ne!(free.canonical_id, paid.canonical_id);
}

#[test]
fn exact_dated_id_wins_before_stripped_family_alias() {
    let dated = lookup("openai/gpt-4o-2024-11-20").expect("dated gpt-4o should resolve exactly");
    assert_eq!(dated.canonical_id, "openai/gpt-4o-2024-11-20");

    let base = lookup("openai/gpt-4o").expect("base gpt-4o should resolve exactly");
    assert_eq!(base.canonical_id, "openai/gpt-4o");
}

#[test]
fn provider_aware_lookup_handles_short_ids() {
    let result = lookup_with_provider(Some("openai"), "gpt-5-codex");
    let LookupResult::Found(card) = result else {
        panic!("expected provider-aware match");
    };
    assert_eq!(card.canonical_id, "openai/gpt-5-codex");
}

#[test]
fn knowledge_cutoff_helper_omits_for_unknown() {
    assert_eq!(
        knowledge_cutoff("anthropic/claude-sonnet-4.5"),
        Some("January 2025".to_string())
    );
    assert_eq!(knowledge_cutoff("claude-opus-9-9"), None);
}

#[test]
fn curated_cutoff_covers_active_builtin_models() {
    assert_eq!(
        knowledge_cutoff("claude-opus-4-7"),
        Some("January 2026".to_string())
    );
    assert_eq!(
        knowledge_cutoff("claude-sonnet-4-6"),
        Some("August 2025".to_string())
    );
    assert_eq!(
        knowledge_cutoff("claude-haiku-4-5"),
        Some("February 2025".to_string())
    );
    assert_eq!(knowledge_cutoff("gpt-5-4"), Some("August 2025".to_string()));
    assert_eq!(
        knowledge_cutoff("gpt-5-5"),
        Some("December 2025".to_string())
    );
}

#[test]
fn openrouter_pricing_resolves_for_known_models() {
    let anthropic =
        pricing(Some("anthropic"), "claude-sonnet-4-5").expect("anthropic pricing should resolve");
    assert_eq!(anthropic.input_per_million_usd, 3.0);
    assert_eq!(anthropic.output_per_million_usd, 15.0);

    let openai = pricing(Some("openai"), "gpt-5-codex").expect("openai pricing should resolve");
    assert_eq!(openai.input_per_million_usd, 1.25);
    assert_eq!(openai.output_per_million_usd, 10.0);
}

#[test]
fn dynamic_snapshot_parser_builds_catalog() {
    let json = r#"{
        "data": [{
            "id": "test-provider/test-model-1",
            "canonical_slug": "test-provider/test-model-1-20260102",
            "name": "Test Model",
            "context_length": 12345,
            "pricing": {
                "prompt": "0.000001",
                "completion": "0.000002",
                "input_cache_read": "0.0000001",
                "input_cache_write": "0.0000003"
            },
            "top_provider": { "context_length": 54321 },
            "knowledge_cutoff": "2025-12-31"
        }]
    }"#;

    let catalog = ModelCardCatalog::from_openrouter_json(json)
        .expect("dynamic snapshot should build a catalog");

    let LookupResult::Found(card) = catalog.lookup("test-model-1") else {
        panic!("installed snapshot should resolve");
    };
    assert_eq!(card.canonical_id, "test-provider/test-model-1");
    assert_eq!(card.vendor_context_window, Some(54321));
    assert_eq!(
        card.knowledge_cutoff.as_ref().map(|c| c.display.as_str()),
        Some("December 2025")
    );
}

#[test]
fn display_model_name_strips_provider_preserving_spelling() {
    // Provider-prefixed → bare, spelling preserved (no canonical rewrite).
    assert_eq!(
        display_model_name("anthropic/claude-sonnet-4.5"),
        "claude-sonnet-4.5"
    );
    assert_eq!(display_model_name("acme/self-hosted-x"), "self-hosted-x");
    // Bare id (the common case) passes through verbatim — the agent is
    // told the exact model it runs, not a re-canonicalized slug.
    assert_eq!(display_model_name("claude-opus-4-7"), "claude-opus-4-7");
    assert_eq!(
        display_model_name("totally-custom-model"),
        "totally-custom-model"
    );
    // Empty input stays empty so the env line is omitted.
    assert_eq!(display_model_name(""), "");
}
