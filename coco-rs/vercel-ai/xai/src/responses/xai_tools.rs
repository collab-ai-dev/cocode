use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

// Argument structs for the xAI provider-defined (server-side) tools.
//
// Each mirrors the corresponding `*ArgsSchema` in `tool/*.ts`. The arguments
// arrive camelCase on a provider tool's `args` map; each struct deserializes
// that map and renders the snake_case wire object via `to_wire`. Only present
// fields are emitted (the TS drops `undefined` during `JSON.stringify`).

/// `xai.web_search` → `{ "type": "web_search", … }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct XaiWebSearchArgs {
    #[serde(rename = "allowedDomains")]
    pub allowed_domains: Option<Vec<String>>,
    #[serde(rename = "excludedDomains")]
    pub excluded_domains: Option<Vec<String>>,
    #[serde(rename = "enableImageSearch")]
    pub enable_image_search: Option<bool>,
    #[serde(rename = "enableImageUnderstanding")]
    pub enable_image_understanding: Option<bool>,
}

impl XaiWebSearchArgs {
    pub fn to_wire(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("type".into(), Value::String("web_search".into()));
        if let Some(v) = &self.allowed_domains {
            m.insert("allowed_domains".into(), json!(v));
        }
        if let Some(v) = &self.excluded_domains {
            m.insert("excluded_domains".into(), json!(v));
        }
        if let Some(v) = self.enable_image_search {
            m.insert("enable_image_search".into(), Value::Bool(v));
        }
        if let Some(v) = self.enable_image_understanding {
            m.insert("enable_image_understanding".into(), Value::Bool(v));
        }
        Value::Object(m)
    }
}

/// `xai.x_search` → `{ "type": "x_search", … }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct XaiXSearchArgs {
    #[serde(rename = "allowedXHandles")]
    pub allowed_x_handles: Option<Vec<String>>,
    #[serde(rename = "excludedXHandles")]
    pub excluded_x_handles: Option<Vec<String>>,
    #[serde(rename = "fromDate")]
    pub from_date: Option<String>,
    #[serde(rename = "toDate")]
    pub to_date: Option<String>,
    #[serde(rename = "enableImageUnderstanding")]
    pub enable_image_understanding: Option<bool>,
    #[serde(rename = "enableVideoUnderstanding")]
    pub enable_video_understanding: Option<bool>,
}

impl XaiXSearchArgs {
    pub fn to_wire(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("type".into(), Value::String("x_search".into()));
        if let Some(v) = &self.allowed_x_handles {
            m.insert("allowed_x_handles".into(), json!(v));
        }
        if let Some(v) = &self.excluded_x_handles {
            m.insert("excluded_x_handles".into(), json!(v));
        }
        if let Some(v) = &self.from_date {
            m.insert("from_date".into(), Value::String(v.clone()));
        }
        if let Some(v) = &self.to_date {
            m.insert("to_date".into(), Value::String(v.clone()));
        }
        if let Some(v) = self.enable_image_understanding {
            m.insert("enable_image_understanding".into(), Value::Bool(v));
        }
        if let Some(v) = self.enable_video_understanding {
            m.insert("enable_video_understanding".into(), Value::Bool(v));
        }
        Value::Object(m)
    }
}

/// `xai.file_search` → `{ "type": "file_search", … }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct XaiFileSearchArgs {
    #[serde(rename = "vectorStoreIds")]
    pub vector_store_ids: Option<Vec<String>>,
    #[serde(rename = "maxNumResults")]
    pub max_num_results: Option<u64>,
}

impl XaiFileSearchArgs {
    pub fn to_wire(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("type".into(), Value::String("file_search".into()));
        if let Some(v) = &self.vector_store_ids {
            m.insert("vector_store_ids".into(), json!(v));
        }
        if let Some(v) = self.max_num_results {
            m.insert("max_num_results".into(), json!(v));
        }
        Value::Object(m)
    }
}

/// `xai.mcp` → `{ "type": "mcp", … }`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct XaiMcpServerArgs {
    #[serde(rename = "serverUrl")]
    pub server_url: Option<String>,
    #[serde(rename = "serverLabel")]
    pub server_label: Option<String>,
    #[serde(rename = "serverDescription")]
    pub server_description: Option<String>,
    #[serde(rename = "allowedTools")]
    pub allowed_tools: Option<Vec<String>>,
    pub headers: Option<std::collections::HashMap<String, String>>,
    pub authorization: Option<String>,
}

impl XaiMcpServerArgs {
    pub fn to_wire(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("type".into(), Value::String("mcp".into()));
        if let Some(v) = &self.server_url {
            m.insert("server_url".into(), Value::String(v.clone()));
        }
        if let Some(v) = &self.server_label {
            m.insert("server_label".into(), Value::String(v.clone()));
        }
        if let Some(v) = &self.server_description {
            m.insert("server_description".into(), Value::String(v.clone()));
        }
        if let Some(v) = &self.allowed_tools {
            m.insert("allowed_tools".into(), json!(v));
        }
        if let Some(v) = &self.headers {
            m.insert("headers".into(), json!(v));
        }
        if let Some(v) = &self.authorization {
            m.insert("authorization".into(), Value::String(v.clone()));
        }
        Value::Object(m)
    }
}

/// Parse a provider tool's `args` map into a typed arg struct, falling back to
/// the default (all-absent) shape when the map is missing or malformed.
pub fn parse_tool_args<T: Default + for<'de> Deserialize<'de>>(
    args: &std::collections::HashMap<String, Value>,
) -> T {
    serde_json::to_value(args)
        .ok()
        .and_then(|v| serde_json::from_value::<T>(v).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "xai_tools.test.rs"]
mod tests;
