use std::collections::HashMap;

use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4ProviderTool;

/// Web fetch tool arguments.
pub struct WebFetchArgs {
    pub max_uses: Option<u32>,
    pub allowed_domains: Option<Vec<String>>,
    pub blocked_domains: Option<Vec<String>>,
    pub citations: Option<bool>,
    pub max_content_tokens: Option<u32>,
}

fn build_web_fetch_args(args: &WebFetchArgs) -> HashMap<String, Value> {
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
    if let Some(citations) = args.citations {
        map.insert("citations".into(), json!({"enabled": citations}));
    }
    if let Some(max) = args.max_content_tokens {
        map.insert("maxContentTokens".into(), json!(max));
    }
    map
}

/// Web fetch tool (2025-09-10).
pub fn web_fetch_20250910(args: WebFetchArgs) -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.web_fetch_20250910".into(),
        name: "web_fetch_20250910".into(),
        args: build_web_fetch_args(&args),
    }
}

/// Web fetch tool (2026-02-09).
pub fn web_fetch_20260209(args: WebFetchArgs) -> LanguageModelV4ProviderTool {
    LanguageModelV4ProviderTool {
        id: "anthropic.web_fetch_20260209".into(),
        name: "web_fetch_20260209".into(),
        args: build_web_fetch_args(&args),
    }
}
