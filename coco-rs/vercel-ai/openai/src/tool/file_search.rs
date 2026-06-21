use std::collections::HashMap;

use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Create a file_search provider tool for the Responses API.
///
/// # Arguments
/// - `vector_store_ids` - Optional list of vector store IDs to search
/// - `max_num_results` - Optional maximum number of results to return
/// - `ranking_options` - Optional ranking configuration
/// - `filters` - Optional comparison or compound filter
pub fn openai_file_search_tool(
    vector_store_ids: Option<Vec<String>>,
    max_num_results: Option<u32>,
    ranking_options: Option<serde_json::Value>,
    filters: Option<serde_json::Value>,
) -> LanguageModelV4ProviderTool {
    let mut args: HashMap<String, serde_json::Value> = HashMap::new();
    if let Some(ids) = vector_store_ids {
        args.insert("vector_store_ids".into(), json!(ids));
    }
    if let Some(max) = max_num_results {
        args.insert("max_num_results".into(), json!(max));
    }
    if let Some(opts) = ranking_options {
        args.insert("ranking_options".into(), opts);
    }
    if let Some(f) = filters {
        args.insert("filters".into(), f);
    }
    LanguageModelV4ProviderTool {
        id: "openai.file_search".into(),
        name: "file_search".into(),
        args,
    }
}
