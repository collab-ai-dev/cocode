use coco_types::ModelCatalogInfo;
use pretty_assertions::assert_eq;

use super::merge;

fn entry(provider: &str, id: &str, ctx: Option<i64>) -> ModelCatalogInfo {
    ModelCatalogInfo {
        provider: provider.into(),
        provider_display: "OpenAI".into(),
        model_id: id.into(),
        display_name: id.into(),
        context_window: ctx,
        supported_efforts: Vec::new(),
        default_effort: None,
    }
}

#[test]
fn test_merge_appends_only_new_ids_and_keeps_richer_static_entry() {
    let base = vec![
        entry("openai-chatgpt", "gpt-5-5", Some(272_000)),
        entry("anthropic", "claude", Some(200_000)),
    ];
    let discovered = vec![
        // already known → skipped (static entry with 272k wins over the `1`)
        ("gpt-5-5".to_string(), Some(1)),
        // genuinely new → appended with the discovered context window
        ("gpt-5-6".to_string(), Some(300_000)),
    ];

    let merged = merge(base, "openai-chatgpt", discovered);
    assert_eq!(merged.len(), 3);

    let known = merged.iter().find(|e| e.model_id == "gpt-5-5").unwrap();
    assert_eq!(known.context_window, Some(272_000));

    let new = merged.iter().find(|e| e.model_id == "gpt-5-6").unwrap();
    assert_eq!(new.provider, "openai-chatgpt");
    assert_eq!(new.provider_display, "OpenAI");
    assert_eq!(new.display_name, "gpt-5-6");
    assert_eq!(new.context_window, Some(300_000));
}

#[test]
fn test_merge_preserves_other_providers_static_entries() {
    let base = vec![entry("anthropic", "claude", Some(200_000))];
    let discovered = vec![("gpt-5-5".to_string(), None)];

    let merged = merge(base, "openai-chatgpt", discovered);
    assert_eq!(merged.len(), 2);
    assert!(merged.iter().any(|e| e.provider == "anthropic"));
    let added = merged.iter().find(|e| e.model_id == "gpt-5-5").unwrap();
    // No static row for the provider → falls back to the instance id as label.
    assert_eq!(added.provider_display, "openai-chatgpt");
}
