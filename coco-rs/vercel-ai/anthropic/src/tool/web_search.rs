use std::collections::HashMap;

use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Web search tool arguments.
pub struct WebSearchArgs {
    pub max_uses: Option<u32>,
    pub allowed_domains: Option<Vec<String>>,
    pub blocked_domains: Option<Vec<String>>,
    pub user_location: Option<WebSearchUserLocation>,
}

/// User location for web search.
pub struct WebSearchUserLocation {
    pub city: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    pub timezone: Option<String>,
}

fn build_web_search_args(args: &WebSearchArgs) -> HashMap<String, Value> {
    let mut map = HashMap::new();
    if let Some(max) = args.max_uses {
        map.insert("maxUses".into(), json!(max));
    }
    if let Some(ref domains) = args.allowed_domains {
        map.insert("allowedDomains".into(), json!(domains));
    }
    if let Some(ref domains) = args.blocked_domains {
        map.insert("blockedDomains".into(), json!(domains));
    }
    if let Some(ref loc) = args.user_location {
        let mut loc_val = json!({"type": "approximate"});
        if let Some(ref c) = loc.city {
            loc_val["city"] = Value::String(c.clone());
        }
        if let Some(ref r) = loc.region {
            loc_val["region"] = Value::String(r.clone());
        }
        if let Some(ref c) = loc.country {
            loc_val["country"] = Value::String(c.clone());
        }
        if let Some(ref t) = loc.timezone {
            loc_val["timezone"] = Value::String(t.clone());
        }
        map.insert("userLocation".into(), loc_val);
    }
    map
}

/// Web search tool (2025-03-05).
pub fn web_search_20250305(args: WebSearchArgs) -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.web_search_20250305".into(),
        name: "web_search_20250305".into(),
        args: build_web_search_args(&args),
    }
}

/// Web search tool (2026-02-09).
pub fn web_search_20260209(args: WebSearchArgs) -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.web_search_20260209".into(),
        name: "web_search_20260209".into(),
        args: build_web_search_args(&args),
    }
}
