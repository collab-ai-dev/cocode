use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use vercel_ai_provider::ProviderMetadata;

/// Build provider metadata for the Responses API response.
/// Nests all fields under the `"openai"` key.
pub fn build_responses_provider_metadata(
    response_id: Option<&str>,
    service_tier: Option<&str>,
) -> Option<ProviderMetadata> {
    let mut openai_obj = serde_json::Map::new();

    if let Some(id) = response_id {
        openai_obj.insert("responseId".into(), Value::String(id.into()));
    }
    if let Some(tier) = service_tier {
        openai_obj.insert("serviceTier".into(), Value::String(tier.into()));
    }

    if openai_obj.is_empty() {
        None
    } else {
        let mut meta = ProviderMetadata::default();
        meta.0.insert("openai".into(), Value::Object(openai_obj));
        Some(meta)
    }
}

/// Key under the `"openai"` namespace carrying the encrypted reasoning blob
/// (the store=false chain-of-thought carrier). Single source of truth so the
/// stream/non-stream writers and the sendback reader can never drift.
pub const REASONING_ENCRYPTED_CONTENT_KEY: &str = "encryptedContent";

/// Build reasoning `provider_metadata` carrying the encrypted reasoning blob
/// under `openai.encryptedContent`. Returns `None` when there is no blob —
/// plain (store=true / non-reasoning) reasoning segments stay metadata-free.
///
/// The blob is preserved as a raw `Value` (the wire shape is a string, but we
/// do not force-stringify). On sendback it round-trips verbatim so the model
/// keeps its chain of thought across tool-call turns — the coco-rs equivalent
/// of codex re-serializing `ResponseItem::Reasoning.encrypted_content`.
pub fn build_reasoning_provider_metadata(
    encrypted_content: Option<&Value>,
) -> Option<ProviderMetadata> {
    let ec = encrypted_content.filter(|v| !v.is_null())?;
    let mut openai_obj = serde_json::Map::new();
    openai_obj.insert(REASONING_ENCRYPTED_CONTENT_KEY.into(), ec.clone());
    let mut meta = ProviderMetadata::default();
    meta.0.insert("openai".into(), Value::Object(openai_obj));
    Some(meta)
}

/// Read the encrypted reasoning blob back out of reasoning `provider_metadata`
/// (`openai.encryptedContent`), if present. Pairs with
/// [`build_reasoning_provider_metadata`].
pub fn reasoning_encrypted_content(meta: &ProviderMetadata) -> Option<&Value> {
    meta.0
        .get("openai")
        .and_then(|o| o.get(REASONING_ENCRYPTED_CONTENT_KEY))
        .filter(|v| !v.is_null())
}

/// Key under `openai` discriminating the raw reasoning channel (value
/// `"text"`) from the default condensed summary channel. Raw reasoning is
/// display-only and stripped on sendback (the server rehydrates it from
/// `encrypted_content`).
pub const REASONING_TYPE_KEY: &str = "reasoningType";

/// Suffix appended to a reasoning item id to namespace its raw `reasoning_text`
/// channel segment, keeping it distinct from the summary segment of the same
/// item. Single source of truth for the stream start/delta/end sites.
pub const RAW_REASONING_ID_SUFFIX: &str = "::content";

/// Accumulator segment id for the raw `reasoning_text` channel of a reasoning
/// item. Pairs the streaming start/delta/end emits to one segment.
pub fn raw_reasoning_segment_id(item_id: &str) -> String {
    format!("{item_id}{RAW_REASONING_ID_SUFFIX}")
}

/// Metadata marking a reasoning segment as the raw `reasoning_text` channel.
pub fn reasoning_text_marker() -> ProviderMetadata {
    let mut openai_obj = serde_json::Map::new();
    openai_obj.insert(REASONING_TYPE_KEY.into(), Value::String("text".into()));
    let mut meta = ProviderMetadata::default();
    meta.0.insert("openai".into(), Value::Object(openai_obj));
    meta
}

/// True when reasoning `provider_metadata` marks the raw `reasoning_text`
/// channel — such a part is display-only and must NOT round-trip as input.
pub fn is_raw_reasoning(meta: &ProviderMetadata) -> bool {
    meta.0
        .get("openai")
        .and_then(|o| o.get(REASONING_TYPE_KEY))
        .and_then(|v| v.as_str())
        == Some("text")
}

fn default_compaction_meta_type() -> String {
    "compaction".to_string()
}

/// Compaction metadata nested under the `"openai"` key. The single source of
/// truth for the wire keys (`type` / `itemId` / `encryptedContent`): both
/// [`build_compaction_provider_metadata`] (write) and
/// [`read_compaction_provider_metadata`] (read) go through this struct, so the
/// capture and sendback sides can never drift. Read-side defaults keep a
/// partial map decodable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponsesCompactionProviderMetadata {
    #[serde(rename = "type", default = "default_compaction_meta_type")]
    pub meta_type: String,
    #[serde(default)]
    pub item_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted_content: Option<String>,
}

/// Build a `ProviderMetadata` for a compaction item.
pub fn build_compaction_provider_metadata(
    item_id: &str,
    encrypted_content: Option<&str>,
) -> ProviderMetadata {
    let meta = ResponsesCompactionProviderMetadata {
        meta_type: default_compaction_meta_type(),
        item_id: item_id.into(),
        encrypted_content: encrypted_content.map(String::from),
    };
    // The struct's fields (String / Option<String>) always serialize, so the
    // fallback is unreachable; degrade to an empty object (which the reader
    // decodes back to defaults) rather than a confusing `null`.
    let value =
        serde_json::to_value(&meta).unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    let mut pm = ProviderMetadata::default();
    pm.0.insert("openai".into(), value);
    pm
}

/// Read compaction state back out of a custom part's `provider_metadata.openai`
/// — the inverse of [`build_compaction_provider_metadata`]. Returns `None` when
/// the part carries no `openai` metadata or the shape doesn't decode.
pub fn read_compaction_provider_metadata(
    meta: &ProviderMetadata,
) -> Option<ResponsesCompactionProviderMetadata> {
    let openai = meta.0.get("openai")?;
    serde_json::from_value(openai.clone()).ok()
}

#[cfg(test)]
#[path = "provider_metadata.test.rs"]
mod tests;
