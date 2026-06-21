//! Semantic prompt envelope and provider layout payload types.

use std::collections::HashMap;

use coco_llm_types::LlmMessage;
use coco_llm_types::LlmPrompt;
use coco_llm_types::ProviderOptions;
use coco_llm_types::UserContentPart;
use coco_types::ProviderApi;
use serde::Deserialize;
use serde::Serialize;
use vercel_ai_provider::JSONValue;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::Warning;

use crate::cache_detection::canonical_extra_body_hash;
use crate::cache_detection::djb2_hash;

pub const PROMPT_LAYOUT_NAMESPACE: &str = "prompt_layout";

pub type PromptPart = UserContentPart;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptEnvelope {
    pub sections: Vec<PromptSection>,
    pub history: Vec<LlmMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptSection {
    pub kind: PromptSectionKind,
    pub content: Vec<PromptPart>,
    pub cache: CacheHint,
    pub source: PromptSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSectionKind {
    Identity,
    ModelBaseInstructions,
    DeveloperPolicy,
    ToolPolicy,
    ProjectInstructions,
    Environment,
    Memory,
    LoadedContext,
    SkillListing,
    McpInstructions,
    IdeContext,
    HookContext,
    ActiveTopic,
    UserContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSource {
    BuiltIn,
    Config,
    Project,
    Memory,
    Runtime,
    Tool,
    User,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheHint {
    #[default]
    Ephemeral,
    Stable,
    Breakpoint,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptLayoutOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_blocks: Option<Vec<AnthropicSystemBlock>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layout_warnings: Vec<Warning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash_inputs: Option<PromptHashInputs>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnthropicSystemBlock {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<AnthropicCacheControl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnthropicCacheControl {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptHashInputs {
    pub system_text_hash: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control_hash: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub developer_text_hash: Option<u64>,
    pub contextual_user_text_hash: u64,
    pub contextual_user_char_count: i64,
    pub tools_hash: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub per_tool_hashes: Vec<(String, u64)>,
}

pub fn put_layout_options(opts: &mut ProviderOptions, layout: &PromptLayoutOptions) {
    let value = serde_json::to_value(layout).unwrap_or(JSONValue::Null);
    let Some(object) = value.as_object() else {
        return;
    };

    let mut namespace = HashMap::new();
    for (key, value) in object {
        if !value.is_null() {
            namespace.insert(key.clone(), value.clone());
        }
    }
    opts.set(PROMPT_LAYOUT_NAMESPACE, namespace);
}

pub fn take_layout_options(opts: &ProviderOptions) -> Option<PromptLayoutOptions> {
    let namespace = opts.get(PROMPT_LAYOUT_NAMESPACE)?;
    let mut object = serde_json::Map::new();
    for (key, value) in namespace {
        object.insert(key.clone(), value.clone());
    }
    serde_json::from_value(JSONValue::Object(object)).ok()
}

/// Build a `PromptLayoutOptions` for a given provider family from the
/// normalized vercel-ai prompt that `build_call_options` is about to
/// send.
///
/// Routes the System / Developer text into the provider's native
/// top-level slot (OpenAI `instructions`, Anthropic `system[]`, Gemini
/// `systemInstruction`) and computes provider-agnostic
/// `prompt_hash_inputs` that the cache-break detector reads in lieu of
/// re-walking the prompt stream itself.
///
/// Today the engine still emits the entire prompt shell as a single
/// `LlmMessage::System`, so the adapter places that text
/// verbatim in the chosen slot. When `app/query` later constructs a
/// richer `PromptEnvelope` with semantic sections, the routing table in
/// `docs/coco-rs/provider-prompt-role-architecture.md` §4 picks the
/// destination per `(kind, family)`.
pub fn build_prompt_layout_from_prompt(
    prompt: &LlmPrompt,
    api: ProviderApi,
    tools: Option<&[LanguageModelV4Tool]>,
) -> PromptLayoutOptions {
    let (system_text, _) = collect_role_text(prompt, |m| matches!(m, LlmMessage::System { .. }));
    let (developer_text, _) =
        collect_role_text(prompt, |m| matches!(m, LlmMessage::Developer { .. }));
    let (contextual_user_text, contextual_user_chars) =
        collect_role_text(prompt, |m| matches!(m, LlmMessage::User { .. }));

    let mut layout = PromptLayoutOptions::default();

    if !system_text.is_empty() {
        match api {
            ProviderApi::Openai => {
                layout.instructions = Some(system_text.clone());
            }
            ProviderApi::Anthropic => {
                layout.system_blocks = Some(vec![AnthropicSystemBlock {
                    text: system_text.clone(),
                    cache_control: None,
                }]);
            }
            ProviderApi::Gemini => {
                layout.system_instruction = Some(system_text.clone());
            }
            // Compat-style providers receive identical chat-format
            // messages; no separate top-level slot is populated. The
            // provider crate ignores `prompt_layout` and falls back to
            // the System message already in the prompt stream.
            ProviderApi::OpenaiCompat | ProviderApi::Volcengine | ProviderApi::Zai => {}
        }
    }

    let (tool_names, per_tool_map, tools_hash) = hash_tools(tools);
    let per_tool_hashes: Vec<(String, u64)> = tool_names
        .into_iter()
        .map(|name| {
            let h = per_tool_map.get(&name).copied().unwrap_or(0);
            (name, h)
        })
        .collect();

    let system_text_hash = djb2_hash(system_text.as_bytes());
    let developer_text_hash = if developer_text.is_empty() {
        None
    } else {
        Some(djb2_hash(developer_text.as_bytes()))
    };
    let contextual_user_text_hash = djb2_hash(contextual_user_text.as_bytes());

    layout.prompt_hash_inputs = Some(PromptHashInputs {
        system_text_hash,
        cache_control_hash: None,
        developer_text_hash,
        contextual_user_text_hash,
        contextual_user_char_count: contextual_user_chars,
        tools_hash,
        per_tool_hashes,
    });

    layout
}

fn collect_role_text<F>(prompt: &LlmPrompt, predicate: F) -> (String, i64)
where
    F: Fn(&LlmMessage) -> bool,
{
    let mut text = String::new();
    for msg in prompt {
        if !predicate(msg) {
            continue;
        }
        let parts: &[UserContentPart] = match msg {
            LlmMessage::System { content, .. }
            | LlmMessage::Developer { content, .. }
            | LlmMessage::User { content, .. } => content,
            _ => continue,
        };
        if !text.is_empty() {
            text.push('\n');
        }
        for part in parts {
            if let UserContentPart::Text(t) = part {
                text.push_str(&t.text);
            }
        }
    }
    let chars = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
    (text, chars)
}

/// Canonical-hash the tool list. Returns `(tool_names_in_order,
/// per_tool_hashes, aggregate_hash)`.
///
/// Walks `names` in declaration order and folds each per-tool hash
/// through djb2 so the aggregate is deterministic regardless of HashMap
/// iteration order.
pub(crate) fn hash_tools(
    tools: Option<&[LanguageModelV4Tool]>,
) -> (Vec<String>, HashMap<String, u64>, u64) {
    let Some(tools) = tools else {
        return (Vec::new(), HashMap::new(), 0);
    };
    let mut names = Vec::with_capacity(tools.len());
    let mut per_tool = HashMap::with_capacity(tools.len());
    for (idx, tool) in tools.iter().enumerate() {
        let raw_name = match tool {
            LanguageModelV4Tool::Function(f) => f.name.clone(),
            LanguageModelV4Tool::Provider(p) => p.name.clone(),
        };
        let key = if raw_name.is_empty() {
            format!("__idx_{idx}")
        } else {
            raw_name
        };
        let value = serde_json::to_value(tool).unwrap_or(serde_json::Value::Null);
        per_tool.insert(key.clone(), canonical_extra_body_hash(&value));
        names.push(key);
    }
    let mut agg: u64 = 0;
    for name in &names {
        if let Some(h) = per_tool.get(name) {
            agg = agg.wrapping_mul(33).wrapping_add(*h);
        }
    }
    (names, per_tool, agg)
}

#[cfg(test)]
#[path = "prompt_layout.test.rs"]
mod tests;
