//! Google Generative AI language model implementation.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use regex::Regex;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use serde_json::json;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FilePart;
use vercel_ai_provider::FinishReason;
use vercel_ai_provider::LanguageModelV4;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4GenerateResult;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4StreamResult;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::LanguageModelV4ToolResult;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::ReasoningFilePart;
use vercel_ai_provider::ReasoningLevel;
use vercel_ai_provider::ResponseFormat;
use vercel_ai_provider::ToolResultContent;
use vercel_ai_provider::ToolResultPart;
use vercel_ai_provider::Warning;
use vercel_ai_provider::content::SourcePart;
use vercel_ai_provider::language_model::LanguageModelV4Request;
use vercel_ai_provider::language_model::LanguageModelV4Response;
use vercel_ai_provider::language_model::v4::stream::File as StreamFile;
use vercel_ai_provider::language_model::v4::stream::ReasoningFile as StreamReasoningFile;
use vercel_ai_provider::response_metadata::ResponseMetadata;

use vercel_ai_provider_utils::JsonResponseHandler;
use vercel_ai_provider_utils::combine_headers;
use vercel_ai_provider_utils::extract_namespaced;
use vercel_ai_provider_utils::is_custom_reasoning;
use vercel_ai_provider_utils::map_reasoning_to_provider_budget;
use vercel_ai_provider_utils::map_reasoning_to_provider_effort;
use vercel_ai_provider_utils::post_json_to_api_with_client_tapped;
use vercel_ai_provider_utils::post_stream_to_api_with_client_tapped;
use vercel_ai_provider_utils::without_trailing_slash;

use crate::convert_google_generative_ai_usage::GoogleUsageMetadata;
use crate::convert_google_generative_ai_usage::convert_usage;
use crate::convert_to_google_generative_ai_messages::ConvertOptions;
use crate::convert_to_google_generative_ai_messages::convert_to_google_generative_ai_messages_with_warnings;
use crate::get_model_path::get_model_path;
use crate::google_error::GoogleFailedResponseHandler;
use crate::google_generative_ai_options::GoogleLanguageModelOptions;
use crate::google_prepare_tools::prepare_tools;

/// Type alias for supported URL pattern function.
type SupportedUrlsFn = Arc<dyn Fn() -> HashMap<String, Vec<Regex>> + Send + Sync>;

/// Configuration for the Google Generative AI language model.
pub struct GoogleGenerativeAILanguageModelConfig {
    /// Provider identifier string.
    pub provider: String,
    /// Base URL for the API.
    pub base_url: String,
    /// Function to generate request headers.
    pub headers: Arc<dyn Fn() -> HashMap<String, String> + Send + Sync>,
    /// Function to generate unique IDs.
    pub generate_id: Arc<dyn Fn() -> String + Send + Sync>,
    /// Supported URL patterns for file references.
    pub supported_urls: Option<SupportedUrlsFn>,
    /// Optional HTTP client for connection pooling.
    pub client: Option<Arc<reqwest::Client>>,
}

/// Google Generative AI language model.
pub struct GoogleGenerativeAILanguageModel {
    model_id: String,
    config: GoogleGenerativeAILanguageModelConfig,
}

impl GoogleGenerativeAILanguageModel {
    /// Create a new Google Generative AI language model.
    pub fn new(model_id: impl Into<String>, config: GoogleGenerativeAILanguageModelConfig) -> Self {
        Self {
            model_id: model_id.into(),
            config,
        }
    }

    /// Determine the provider options namespace key.
    fn provider_options_name(&self) -> String {
        if self.config.provider.contains("vertex") {
            "vertex".to_string()
        } else {
            "google".to_string()
        }
    }

    /// Parse provider options into typed + extras.
    ///
    /// - `typed` — parsed `GoogleLanguageModelOptions`, used for
    ///   `generationConfig` / `safetySettings` / `thinkingConfig` /
    ///   `cachedContent` typed body writes and `responseSchema`
    ///   side-effects.
    /// - `raw` — extras captured by `#[serde(flatten)]`. The language
    ///   model **deep-merges** these into the wire body after typed
    ///   writes (see "extras override typed writes" doctrine in
    ///   `services/inference/CLAUDE.md`).
    ///
    /// Namespace policy: `"google"` is canonical; `provider_options_name`
    /// is the custom override (e.g. `"vertex"`). When both are present
    /// they are **deep-merged** with custom winning on per-key overlap —
    /// previously a fallback-not-found path silently dropped one side.
    fn parse_provider_options(
        &self,
        options: &LanguageModelV4CallOptions,
        provider_options_name: &str,
    ) -> (
        GoogleLanguageModelOptions,
        std::collections::BTreeMap<String, Value>,
    ) {
        extract_namespaced(
            options.provider_options.as_ref(),
            "google",
            provider_options_name,
        )
    }

    /// Build the request arguments for the Google API.
    /// Returns (body, headers, warnings, provider_options_name).
    ///
    /// `pub` so sibling transports that speak the same Gemini wire (e.g.
    /// `vercel-ai-google-codeassist`, which wraps this body in a Code Assist
    /// envelope and posts it to `cloudcode-pa`) can reuse the exact
    /// `generateContent` body builder rather than duplicating it.
    #[allow(clippy::type_complexity)]
    pub fn get_args(
        &self,
        options: &LanguageModelV4CallOptions,
    ) -> Result<(Value, HashMap<String, String>, Vec<Warning>, String), AISdkError> {
        let provider_options_name = self.provider_options_name();
        let (provider_opts, raw_provider_options) =
            self.parse_provider_options(options, &provider_options_name);
        let mut warnings: Vec<Warning> = Vec::new();

        // Check if model supports system instructions (Gemma models don't)
        let model_lower = self.model_id.to_lowercase();
        let is_gemma = model_lower.starts_with("gemma-");

        let convert_opts = ConvertOptions {
            supports_system_instruction: !is_gemma,
            provider_options_name: provider_options_name.clone(),
            supports_function_response_parts: is_gemini_3_model(&self.model_id),
        };

        let (prompt, prompt_warnings) =
            convert_to_google_generative_ai_messages_with_warnings(&options.prompt, &convert_opts)
                .map_err(AISdkError::new)?;
        warnings.extend(prompt_warnings);

        // Prepare tools
        let prepared_tools = prepare_tools(&options.tools, &options.tool_choice, &self.model_id);
        warnings.extend(prepared_tools.tool_warnings);

        // Vertex RAG store warning for non-vertex providers
        if let Some(ref tools) = options.tools {
            let has_rag = tools.iter().any(|t| {
                matches!(t, vercel_ai_provider::LanguageModelV4Tool::Provider(p)
                    if p.id == crate::tool::vertex_rag_store::VERTEX_RAG_STORE_TOOL_ID)
            });
            if has_rag && !self.config.provider.contains("vertex") {
                warnings.push(Warning::unsupported_with_details(
                    "provider-defined tool",
                    "google.vertex_rag_store requires a Vertex AI provider (google.vertex.*)",
                ));
            }
        }

        // Build generation config
        let mut generation_config = json!({});

        if let Some(max_tokens) = options.max_output_tokens {
            generation_config["maxOutputTokens"] = json!(max_tokens);
        }
        if let Some(temp) = options.temperature {
            generation_config["temperature"] = json!(temp);
        }
        if let Some(top_p) = options.top_p {
            generation_config["topP"] = json!(top_p);
        }
        if let Some(top_k) = options.top_k {
            generation_config["topK"] = json!(top_k);
        }
        if let Some(ref stop) = options.stop_sequences {
            generation_config["stopSequences"] = json!(stop);
        }
        if let Some(freq) = options.frequency_penalty {
            generation_config["frequencyPenalty"] = json!(freq);
        }
        if let Some(pres) = options.presence_penalty {
            generation_config["presencePenalty"] = json!(pres);
        }
        if let Some(seed) = options.seed {
            generation_config["seed"] = json!(seed);
        }

        // Response format
        if let Some(ref format) = options.response_format {
            match format {
                ResponseFormat::Json {
                    schema,
                    name: _,
                    description: _,
                } => {
                    generation_config["responseMimeType"] = json!("application/json");
                    // Only send responseSchema if structured_outputs is not explicitly false
                    if provider_opts.structured_outputs != Some(false)
                        && let Some(schema_val) = schema
                    {
                        let schema_json = serde_json::to_value(schema_val)
                            .unwrap_or(Value::Object(Default::default()));
                        if let Some(openapi) =
                            crate::convert_json_schema_to_openapi_schema::convert_json_schema_to_openapi_schema(&schema_json)
                        {
                            generation_config["responseSchema"] = openapi;
                        }
                    }
                }
                ResponseFormat::Text => {
                    // Do not set responseMimeType for text format;
                    // the API defaults to text output when no MIME type is specified.
                }
            }
        }

        // Provider-specific options
        if let Some(ref modalities) = provider_opts.response_modalities {
            generation_config["responseModalities"] =
                serde_json::to_value(modalities).unwrap_or(Value::Null);
        }
        if let Some(ref media_res) = provider_opts.media_resolution {
            generation_config["mediaResolution"] =
                serde_json::to_value(media_res).unwrap_or(Value::Null);
        }
        if let Some(audio_ts) = provider_opts.audio_timestamp {
            generation_config["audioTimestamp"] = json!(audio_ts);
        }
        if let Some(ref image_config) = provider_opts.image_config
            && let Ok(val) = serde_json::to_value(image_config)
        {
            generation_config["imageConfig"] = val;
        }

        // Build request body
        let mut body = json!({
            "generationConfig": generation_config,
            "contents": prompt.contents,
        });

        // Layout slot wins over the converter-derived `systemInstruction`:
        // when `provider_options["prompt_layout"].system_instruction` is
        // set, write the layout text verbatim and emit a warning when both
        // sources are present.
        let layout = parse_prompt_layout_namespace(&options.provider_options);
        let layout_system_instruction = layout.as_ref().and_then(|l| l.system_instruction.clone());
        match (layout_system_instruction, prompt.system_instruction) {
            (Some(layout_si), Some(_converter_si)) => {
                warnings.push(Warning::other(
                    "Google `systemInstruction` set both via prompt_layout and converter; \
                     layout wins",
                ));
                body["systemInstruction"] = json!({ "parts": [{ "text": layout_si }] });
            }
            (Some(layout_si), None) => {
                body["systemInstruction"] = json!({ "parts": [{ "text": layout_si }] });
            }
            (None, Some(si)) => {
                body["systemInstruction"] = serde_json::to_value(si).unwrap_or(Value::Null);
            }
            (None, None) => {}
        }

        // Tools
        let mut tools_array: Vec<Value> = Vec::new();
        if let Some(func_decls) = prepared_tools.function_declarations {
            tools_array.push(json!({ "functionDeclarations": func_decls }));
        }
        for entry in prepared_tools.tool_entries {
            tools_array.push(entry);
        }
        if !tools_array.is_empty() {
            body["tools"] = json!(tools_array);
        }

        // Merge tool_config with retrieval config
        let mut tool_config_val = prepared_tools.tool_config.unwrap_or(json!({}));
        if let Some(ref retrieval_config) = provider_opts.retrieval_config
            && let Ok(val) = serde_json::to_value(retrieval_config)
        {
            tool_config_val["retrievalConfig"] = val;
        }
        if tool_config_val != json!({}) {
            body["toolConfig"] = tool_config_val;
        }

        // Safety settings
        if let Some(ref safety) = provider_opts.safety_settings
            && let Ok(val) = serde_json::to_value(safety)
        {
            body["safetySettings"] = val;
        }

        // Thinking config: resolve from top-level reasoning, then merge provider option on top
        let resolved_thinking =
            resolve_thinking_config(options.reasoning, &self.model_id, &mut warnings);
        let thinking_config = match (&provider_opts.thinking_config, &resolved_thinking) {
            (Some(provider_tc), Some(resolved_tc)) => {
                // Provider option takes precedence: merge resolved as base, overlay provider
                let mut base = serde_json::to_value(resolved_tc).unwrap_or(json!({}));
                let overlay = serde_json::to_value(provider_tc).unwrap_or(json!({}));
                if let (Value::Object(b), Value::Object(o)) = (&mut base, &overlay) {
                    for (k, v) in o {
                        b.insert(k.clone(), v.clone());
                    }
                }
                Some(base)
            }
            (Some(provider_tc), None) => serde_json::to_value(provider_tc).ok(),
            (None, Some(resolved_tc)) => serde_json::to_value(resolved_tc).ok(),
            (None, None) => None,
        };
        if let Some(val) = thinking_config {
            body["generationConfig"]["thinkingConfig"] = val;
        }

        // Cached content
        if let Some(ref cached) = provider_opts.cached_content {
            body["cachedContent"] = json!(cached);
        }

        // Labels
        if let Some(ref labels) = provider_opts.labels {
            body["labels"] = serde_json::to_value(labels).unwrap_or(Value::Null);
        }

        // Deep-merge extra_body onto the wire body. Producers
        // (`coco_inference::thinking_convert`, user-supplied
        // `provider_options["google"]` extras) own the wire-correct
        // nesting — e.g. `{"generationConfig": {"thinkingConfig":
        // {..}}}` lands at `body.generationConfig.thinkingConfig`
        // without clobbering sibling typed writes like
        // `body.generationConfig.maxOutputTokens`. Shallow overwrite
        // would clobber the whole `generationConfig` subtree on any
        // overlap. Deep merge gives extras true escape-hatch override
        // semantics at every nesting depth.
        if !raw_provider_options.is_empty() {
            let overlay = Value::Object(raw_provider_options.into_iter().collect());
            body = vercel_ai_provider_utils::merge_json_value(&body, &overlay);
        }

        // Build headers
        let headers = combine_headers(vec![Some((self.config.headers)()), options.headers.clone()]);

        Ok((body, headers, warnings, provider_options_name))
    }

    /// Map a parsed `generateContent` response into a generate result (content
    /// parts, sources, finish reason, usage, provider metadata).
    ///
    /// `pub` so sibling transports (`vercel-ai-google-codeassist`) that unwrap
    /// their own response envelope can reuse the exact same mapping rather than
    /// duplicating it. `request_body` is echoed onto `result.request`.
    pub fn map_response(
        &self,
        response: &GoogleGenerateContentResponse,
        request_body: Value,
        warnings: Vec<Warning>,
    ) -> LanguageModelV4GenerateResult {
        let provider_options_name = self.provider_options_name();
        let id_gen = &*self.config.generate_id;
        let candidate = response.candidates.first();

        // Extract content parts with thoughtSignature support
        let content = if let Some(candidate) = candidate {
            if let Some(ref content) = candidate.content {
                convert_response_parts_with_metadata(&content.parts, id_gen, &provider_options_name)
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        // Extract sources from grounding metadata
        let mut all_content = content;
        if let Some(candidate) = candidate {
            let sources = extract_sources(
                &candidate.grounding_metadata,
                &candidate.url_context_metadata,
                id_gen,
            );
            for source in sources {
                all_content.push(AssistantContentPart::Source(source));
            }
        }

        // Check for tool calls in content (exclude provider-executed)
        let has_tool_calls = all_content.iter().any(|p| {
            matches!(p, AssistantContentPart::ToolCall(tc) if tc.provider_executed != Some(true))
        });

        let finish_reason = crate::map_google_generative_ai_finish_reason::map_finish_reason(
            candidate.and_then(|c| c.finish_reason.as_deref()),
            has_tool_calls,
        );

        let usage = convert_usage(response.usage_metadata.as_ref());

        // Build provider metadata under namespace key — always include all 6 fields
        let mut namespace_meta = HashMap::new();
        namespace_meta.insert(
            "promptFeedback".to_string(),
            response.prompt_feedback.clone().unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "groundingMetadata".to_string(),
            candidate
                .and_then(|c| c.grounding_metadata.as_ref())
                .and_then(|gm| serde_json::to_value(gm).ok())
                .unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "urlContextMetadata".to_string(),
            candidate
                .and_then(|c| c.url_context_metadata.as_ref())
                .and_then(|ucm| serde_json::to_value(ucm).ok())
                .unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "safetyRatings".to_string(),
            candidate
                .and_then(|c| c.safety_ratings.clone())
                .unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "usageMetadata".to_string(),
            response
                .usage_metadata
                .as_ref()
                .and_then(|um| serde_json::to_value(um).ok())
                .unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "finishMessage".to_string(),
            candidate
                .and_then(|c| c.finish_message.as_ref())
                .map(|fm| Value::String(fm.clone()))
                .unwrap_or(Value::Null),
        );

        let provider_metadata = {
            let mut outer = HashMap::new();
            outer.insert(
                provider_options_name,
                serde_json::to_value(&namespace_meta).unwrap_or(Value::Null),
            );
            Some(ProviderMetadata::from_map(outer))
        };

        let mut result = LanguageModelV4GenerateResult::new(all_content, usage, finish_reason);
        result.warnings = warnings;
        result.request = Some(LanguageModelV4Request::new().with_body(request_body));
        if let Some(model_version) = response.model_version.clone() {
            result.response = Some(LanguageModelV4Response::new().with_model_id(model_version));
        }
        result.provider_metadata = provider_metadata;
        result
    }
}

/// Google API response for generateContent.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleGenerateContentResponse {
    #[serde(default)]
    pub candidates: Vec<GoogleCandidate>,
    #[serde(default)]
    pub usage_metadata: Option<GoogleUsageMetadata>,
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub prompt_feedback: Option<Value>,
}

/// A candidate response from Google.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleCandidate {
    #[serde(default)]
    pub content: Option<GoogleCandidateContent>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub finish_message: Option<String>,
    #[serde(default)]
    pub safety_ratings: Option<Value>,
    #[serde(default)]
    pub grounding_metadata: Option<GroundingMetadata>,
    #[serde(default)]
    pub url_context_metadata: Option<UrlContextMetadata>,
}

/// Content in a candidate response.
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleCandidateContent {
    #[serde(default)]
    pub parts: Vec<GoogleResponsePart>,
}

/// A part in a Google response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleResponsePart {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub thought: Option<bool>,
    #[serde(default)]
    pub thought_signature: Option<String>,
    #[serde(default)]
    pub function_call: Option<GoogleFunctionCall>,
    #[serde(default)]
    pub inline_data: Option<GoogleInlineData>,
    #[serde(default)]
    pub executable_code: Option<GoogleExecutableCode>,
    #[serde(default)]
    pub code_execution_result: Option<GoogleCodeExecutionResult>,
}

/// A function call in a Google response.
#[derive(Debug, Clone, Deserialize)]
pub struct GoogleFunctionCall {
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

/// Inline data in a Google response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleInlineData {
    pub mime_type: String,
    pub data: String,
}

/// Executable code in a Google response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleExecutableCode {
    #[serde(default)]
    pub language: Option<String>,
    pub code: String,
}

/// Code execution result in a Google response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleCodeExecutionResult {
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
}

/// Grounding metadata from Google Search.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingMetadata {
    #[serde(default)]
    pub grounding_chunks: Option<Vec<GroundingChunk>>,
    #[serde(default)]
    pub web_search_queries: Option<Vec<String>>,
    #[serde(default)]
    pub image_search_queries: Option<Value>,
    #[serde(default)]
    pub retrieval_queries: Option<Value>,
    #[serde(default)]
    pub search_entry_point: Option<Value>,
    #[serde(default)]
    pub grounding_supports: Option<Value>,
    #[serde(default)]
    pub retrieval_metadata: Option<Value>,
}

/// A grounding chunk (source).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingChunk {
    #[serde(default)]
    pub web: Option<GroundingWeb>,
    #[serde(default)]
    pub image: Option<GroundingImage>,
    #[serde(default, rename = "retrievedContext")]
    pub retrieved_context: Option<GroundingRetrievedContext>,
    #[serde(default)]
    pub maps: Option<GroundingMaps>,
}

/// Web grounding info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingWeb {
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

/// Image grounding info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingImage {
    #[serde(default)]
    pub source_uri: Option<String>,
    #[serde(default)]
    pub image_uri: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
}

/// Retrieved context grounding info.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroundingRetrievedContext {
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub file_search_store: Option<String>,
}

/// Maps grounding info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundingMaps {
    #[serde(default)]
    pub uri: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default, rename = "placeId")]
    pub place_id: Option<String>,
}

/// URL context metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlContextMetadata {
    #[serde(default)]
    pub url_metadata: Option<Vec<UrlMetadataEntry>>,
}

/// A URL metadata entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UrlMetadataEntry {
    #[serde(default)]
    pub retrieved_url: Option<String>,
    #[serde(default)]
    pub url_retrieval_status: Option<String>,
}

/// Extract sources from grounding and URL context metadata.
fn detect_media_type(uri: &str) -> &'static str {
    if uri.ends_with(".pdf") {
        "application/pdf"
    } else if uri.ends_with(".txt") {
        "text/plain"
    } else if uri.ends_with(".docx") {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    } else if uri.ends_with(".doc") {
        "application/msword"
    } else if uri.ends_with(".md") || uri.ends_with(".markdown") {
        "text/markdown"
    } else {
        "application/octet-stream"
    }
}

fn extract_sources(
    grounding: &Option<GroundingMetadata>,
    url_context: &Option<UrlContextMetadata>,
    id_gen: &dyn Fn() -> String,
) -> Vec<SourcePart> {
    let mut seen_urls = std::collections::HashSet::new();
    extract_sources_with_dedup(grounding, url_context, id_gen, &mut seen_urls)
}

fn extract_sources_with_dedup(
    grounding: &Option<GroundingMetadata>,
    url_context: &Option<UrlContextMetadata>,
    id_gen: &dyn Fn() -> String,
    seen_urls: &mut std::collections::HashSet<String>,
) -> Vec<SourcePart> {
    let mut sources = Vec::new();

    // Extract from grounding metadata
    if let Some(gm) = grounding
        && let Some(ref chunks) = gm.grounding_chunks
    {
        for chunk in chunks {
            // Web chunks
            if let Some(ref web) = chunk.web
                && let Some(ref uri) = web.uri
                && seen_urls.insert(uri.clone())
            {
                let mut source = SourcePart::url(id_gen(), uri);
                if let Some(ref title) = web.title {
                    source.title = Some(title.clone());
                }
                sources.push(source);
            }

            // Image chunks
            if let Some(ref img) = chunk.image
                && let Some(ref source_uri) = img.source_uri
                && seen_urls.insert(source_uri.clone())
            {
                let mut source = SourcePart::url(id_gen(), source_uri);
                if let Some(ref title) = img.title {
                    source.title = Some(title.clone());
                }
                sources.push(source);
            }

            // Retrieved context chunks
            if let Some(ref ctx) = chunk.retrieved_context {
                if let Some(ref uri) = ctx.uri {
                    if uri.starts_with("http://") || uri.starts_with("https://") {
                        // HTTP URL -> URL source
                        if seen_urls.insert(uri.clone()) {
                            let mut source = SourcePart::url(id_gen(), uri);
                            if let Some(ref title) = ctx.title {
                                source.title = Some(title.clone());
                            }
                            sources.push(source);
                        }
                    } else {
                        // File URI (gs://, etc.) -> Document source with media type detection
                        let media_type = detect_media_type(uri);
                        let title = ctx.title.as_deref().unwrap_or("Unknown Document");
                        let filename = uri.rsplit('/').next().map(str::to_string);
                        let mut source = SourcePart::document(id_gen(), title, media_type);
                        source.filename = filename;
                        source.url = Some(uri.clone());
                        sources.push(source);
                    }
                } else if let Some(ref fss) = ctx.file_search_store {
                    // No URI but file_search_store
                    let title = ctx.title.as_deref().unwrap_or("Unknown Document");
                    let filename = fss.rsplit('/').next().map(str::to_string);
                    let mut source =
                        SourcePart::document(id_gen(), title, "application/octet-stream");
                    source.filename = filename;
                    sources.push(source);
                }
            }

            // Maps chunks
            if let Some(ref maps) = chunk.maps
                && let Some(ref uri) = maps.uri
                && seen_urls.insert(uri.clone())
            {
                let mut source = SourcePart::url(id_gen(), uri);
                if let Some(ref title) = maps.title {
                    source.title = Some(title.clone());
                }
                sources.push(source);
            }
        }
    }

    // Extract from URL context metadata.
    // NOTE: TS does NOT extract Source parts from urlContextMetadata — this is a Rust enhancement.
    if let Some(ucm) = url_context
        && let Some(ref entries) = ucm.url_metadata
    {
        for entry in entries {
            if let Some(ref url) = entry.retrieved_url
                && seen_urls.insert(url.clone())
            {
                sources.push(SourcePart::url(id_gen(), url));
            }
        }
    }

    sources
}

/// Build thoughtSignature provider metadata for a part.
fn thought_signature_metadata(
    part: &GoogleResponsePart,
    provider_options_name: &str,
) -> Option<ProviderMetadata> {
    let sig = part.thought_signature.as_ref()?;
    let mut meta = HashMap::new();
    meta.insert(
        provider_options_name.to_string(),
        json!({ "thoughtSignature": sig }),
    );
    Some(ProviderMetadata::from_map(meta))
}

/// Convert Google response parts to assistant content parts with thoughtSignature metadata.
fn convert_response_parts_with_metadata(
    parts: &[GoogleResponsePart],
    id_gen: &dyn Fn() -> String,
    provider_options_name: &str,
) -> Vec<AssistantContentPart> {
    let mut result: Vec<AssistantContentPart> = Vec::new();
    let mut last_code_execution_tool_call_id: Option<String> = None;

    for part in parts {
        let ts_meta = thought_signature_metadata(part, provider_options_name);

        // Handle executable code (before text)
        if let Some(ref exec_code) = part.executable_code {
            let call_id = id_gen();
            last_code_execution_tool_call_id = Some(call_id.clone());
            let mut tc = vercel_ai_provider::ToolCallPart::new(
                &call_id,
                "code_execution",
                json!({
                    "language": exec_code.language,
                    "code": exec_code.code,
                }),
            );
            tc.provider_executed = Some(true);
            result.push(AssistantContentPart::ToolCall(tc));
        }

        // Handle code execution result
        if let Some(ref exec_result) = part.code_execution_result {
            let call_id = last_code_execution_tool_call_id
                .take()
                .unwrap_or_else(id_gen);
            result.push(AssistantContentPart::ToolResult(ToolResultPart::new(
                call_id,
                "code_execution",
                ToolResultContent::json(json!({
                    "outcome": exec_result.outcome,
                    "output": exec_result.output,
                })),
            )));
        }

        // Handle text parts
        if let Some(ref text) = part.text {
            // Empty text with thoughtSignature: apply to last content
            if text.is_empty() && ts_meta.is_some() {
                if let Some(last) = result.last_mut() {
                    match last {
                        AssistantContentPart::Text(tp) => {
                            tp.provider_metadata = ts_meta.clone();
                        }
                        AssistantContentPart::Reasoning(rp) => {
                            rp.provider_metadata = ts_meta.clone();
                        }
                        _ => {}
                    }
                }
            } else if part.thought == Some(true) {
                result.push(AssistantContentPart::Reasoning(
                    vercel_ai_provider::content::ReasoningPart {
                        text: text.clone(),
                        provider_metadata: ts_meta.clone(),
                    },
                ));
            } else {
                result.push(AssistantContentPart::Text(
                    vercel_ai_provider::content::TextPart {
                        text: text.clone(),
                        provider_metadata: ts_meta.clone(),
                    },
                ));
            }
        }

        // Handle function calls
        if let Some(ref fc) = part.function_call {
            let mut tc = vercel_ai_provider::ToolCallPart::new(id_gen(), &fc.name, fc.args.clone());
            tc.provider_metadata = ts_meta.clone();
            result.push(AssistantContentPart::ToolCall(tc));
        }

        // Handle inline data
        if let Some(ref inline) = part.inline_data {
            if part.thought == Some(true) {
                let mut rfp = ReasoningFilePart::from_base64(&inline.data, &inline.mime_type);
                rfp.provider_metadata = ts_meta.clone();
                result.push(AssistantContentPart::ReasoningFile(rfp));
            } else {
                let mut fp = FilePart::image_base64(&inline.data, &inline.mime_type);
                fp.provider_metadata = ts_meta.clone();
                result.push(AssistantContentPart::File(fp));
            }
        }
    }

    result
}

/// Convert Google response parts to assistant content parts (simple version, used in tests).
#[cfg(test)]
fn convert_response_parts(
    parts: &[GoogleResponsePart],
    id_gen: &dyn Fn() -> String,
) -> Vec<AssistantContentPart> {
    convert_response_parts_with_metadata(parts, id_gen, "google")
}

#[async_trait]
impl LanguageModelV4 for GoogleGenerativeAILanguageModel {
    fn provider(&self) -> &str {
        &self.config.provider
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        if let Some(ref f) = self.config.supported_urls {
            f()
        } else {
            HashMap::new()
        }
    }

    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError> {
        let (body, headers, warnings, _provider_options_name) = self.get_args(options)?;

        let model_path = get_model_path(&self.model_id);
        let url = format!(
            "{}/{}:generateContent",
            without_trailing_slash(&self.config.base_url),
            model_path
        );

        let response: GoogleGenerateContentResponse = post_json_to_api_with_client_tapped(
            &url,
            Some(headers),
            &body,
            JsonResponseHandler::new(),
            GoogleFailedResponseHandler,
            abort_signal.clone(),
            self.config.client.clone(),
            options.wire_tap.clone(),
        )
        .await?;

        Ok(self.map_response(&response, body, warnings))
    }

    async fn do_stream(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError> {
        let (body, headers, warnings, provider_options_name) = self.get_args(options)?;
        let include_raw = options.include_raw_chunks.unwrap_or(false);

        let model_path = get_model_path(&self.model_id);
        let url = format!(
            "{}/{}:streamGenerateContent?alt=sse",
            without_trailing_slash(&self.config.base_url),
            model_path
        );

        let byte_stream = post_stream_to_api_with_client_tapped(
            &url,
            Some(headers),
            &body,
            abort_signal.clone(),
            self.config.client.clone(),
            options.wire_tap.clone(),
        )
        .await?;

        let id_gen = self.config.generate_id.clone();

        let stream = create_google_stream(
            byte_stream,
            id_gen,
            include_raw,
            warnings,
            provider_options_name,
            ChunkEnvelope::Direct,
        );

        let mut result = LanguageModelV4StreamResult::new(stream);
        result.request = Some(LanguageModelV4Request::new().with_body(body));

        Ok(result)
    }
}

struct StreamState {
    byte_stream: vercel_ai_provider_utils::ByteStream,
    buffer: String,
    id_gen: Arc<dyn Fn() -> String + Send + Sync>,
    include_raw: bool,
    text_id: Option<String>,
    reasoning_id: Option<String>,
    tool_call_ids: HashMap<String, String>,
    seen_source_urls: std::collections::HashSet<String>,
    pending_parts: Vec<LanguageModelV4StreamPart>,
    done: bool,
    started: bool,
    warnings: Vec<Warning>,
    provider_options_name: String,
    envelope: ChunkEnvelope,
    last_code_execution_tool_call_id: Option<String>,
    last_grounding_metadata: Option<GroundingMetadata>,
    last_url_context_metadata: Option<UrlContextMetadata>,
}

/// How each SSE `data:` chunk is framed before it deserializes into a
/// `GoogleGenerateContentResponse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkEnvelope {
    /// The chunk is a bare `GoogleGenerateContentResponse` (the public
    /// `generativelanguage` streaming API).
    Direct,
    /// The chunk is wrapped as `{"response": <GoogleGenerateContentResponse>}`
    /// (the Code Assist `cloudcode-pa` streaming API). The `response` field is
    /// unwrapped before mapping.
    CodeAssistWrapped,
}

/// Create a stream of LanguageModelV4StreamPart from a byte stream.
///
/// `pub` so `vercel-ai-google-codeassist` can reuse the full SSE framing +
/// part-mapping state machine, supplying its own byte stream (posted to
/// `cloudcode-pa` with a Bearer header) and `ChunkEnvelope::CodeAssistWrapped`.
pub fn create_google_stream(
    byte_stream: vercel_ai_provider_utils::ByteStream,
    id_gen: Arc<dyn Fn() -> String + Send + Sync>,
    include_raw: bool,
    warnings: Vec<Warning>,
    provider_options_name: String,
    envelope: ChunkEnvelope,
) -> Pin<Box<dyn Stream<Item = Result<LanguageModelV4StreamPart, AISdkError>> + Send>> {
    use std::collections::HashSet;

    let stream = futures::stream::unfold(
        StreamState {
            byte_stream,
            buffer: String::new(),
            id_gen,
            include_raw,
            text_id: None,
            reasoning_id: None,
            tool_call_ids: HashMap::new(),
            seen_source_urls: HashSet::new(),
            pending_parts: Vec::new(),
            done: false,
            started: false,
            warnings,
            provider_options_name,
            envelope,
            last_code_execution_tool_call_id: None,
            last_grounding_metadata: None,
            last_url_context_metadata: None,
        },
        |mut state| async move {
            // Return pending parts first
            if let Some(part) = state.pending_parts.pop() {
                return Some((Ok(part), state));
            }

            if state.done {
                return None;
            }

            // Emit stream start
            if !state.started {
                state.started = true;
                let w = std::mem::take(&mut state.warnings);
                let part = LanguageModelV4StreamPart::StreamStart { warnings: w };
                return Some((Ok(part), state));
            }

            use futures::TryStreamExt;
            loop {
                match state.byte_stream.try_next().await {
                    Ok(Some(bytes)) => {
                        let chunk = String::from_utf8_lossy(&bytes);
                        state.buffer.push_str(&chunk);

                        // Process SSE lines
                        while let Some(pos) = state.buffer.find('\n') {
                            let line = state.buffer[..pos].trim().to_string();
                            state.buffer = state.buffer[pos + 1..].to_string();

                            if line.is_empty() || line == "data: [DONE]" {
                                continue;
                            }

                            if let Some(data) = line.strip_prefix("data: ") {
                                // Code Assist wraps each chunk as `{"response": {...}}`;
                                // unwrap to the inner response before mapping.
                                let parsed = match state.envelope {
                                    ChunkEnvelope::Direct => {
                                        serde_json::from_str::<GoogleGenerateContentResponse>(data)
                                    }
                                    ChunkEnvelope::CodeAssistWrapped => {
                                        serde_json::from_str::<Value>(data).and_then(|v| {
                                            let inner =
                                                v.get("response").cloned().unwrap_or(Value::Null);
                                            serde_json::from_value::<GoogleGenerateContentResponse>(
                                                inner,
                                            )
                                        })
                                    }
                                };
                                match parsed {
                                    Ok(response) => {
                                        let mut stream_ctx = StreamProcessingContext {
                                            id_gen: &state.id_gen,
                                            text_id: &mut state.text_id,
                                            reasoning_id: &mut state.reasoning_id,
                                            tool_call_ids: &mut state.tool_call_ids,
                                            seen_source_urls: &mut state.seen_source_urls,
                                            include_raw: state.include_raw,
                                            provider_options_name: &state.provider_options_name,
                                            last_code_execution_tool_call_id: &mut state
                                                .last_code_execution_tool_call_id,
                                            last_grounding_metadata: &mut state
                                                .last_grounding_metadata,
                                            last_url_context_metadata: &mut state
                                                .last_url_context_metadata,
                                        };
                                        let parts =
                                            process_stream_chunk(&response, &mut stream_ctx, data);
                                        if !parts.is_empty() {
                                            let mut parts = parts;
                                            let first = parts.remove(0);
                                            parts.reverse();
                                            state.pending_parts = parts;
                                            return Some((Ok(first), state));
                                        }
                                    }
                                    Err(e) => {
                                        let part = LanguageModelV4StreamPart::Error {
                                            error: vercel_ai_provider::StreamError::new(format!(
                                                "Failed to parse chunk: {e}"
                                            )),
                                        };
                                        return Some((Ok(part), state));
                                    }
                                }
                            }
                        }
                    }
                    Ok(None) => {
                        // Stream ended - close any open text/reasoning blocks
                        let mut final_parts = Vec::new();
                        if let Some(ref tid) = state.text_id {
                            final_parts.push(LanguageModelV4StreamPart::TextEnd {
                                id: tid.clone(),
                                provider_metadata: None,
                            });
                        }
                        if let Some(ref rid) = state.reasoning_id {
                            final_parts.push(LanguageModelV4StreamPart::ReasoningEnd {
                                id: rid.clone(),
                                provider_metadata: None,
                            });
                        }

                        state.done = true;

                        if let Some(part) = final_parts.pop() {
                            final_parts.reverse();
                            state.pending_parts = final_parts;
                            return Some((Ok(part), state));
                        }

                        return None;
                    }
                    Err(e) => {
                        state.done = true;
                        return Some((Err(AISdkError::new(format!("Stream error: {e}"))), state));
                    }
                }
            }
        },
    );

    Box::pin(stream)
}

/// Mutable state passed to `process_stream_chunk` on every SSE event.
struct StreamProcessingContext<'a> {
    id_gen: &'a Arc<dyn Fn() -> String + Send + Sync>,
    text_id: &'a mut Option<String>,
    reasoning_id: &'a mut Option<String>,
    tool_call_ids: &'a mut HashMap<String, String>,
    seen_source_urls: &'a mut std::collections::HashSet<String>,
    include_raw: bool,
    provider_options_name: &'a str,
    last_code_execution_tool_call_id: &'a mut Option<String>,
    last_grounding_metadata: &'a mut Option<GroundingMetadata>,
    last_url_context_metadata: &'a mut Option<UrlContextMetadata>,
}

/// Process a single SSE chunk and return stream parts.
fn process_stream_chunk(
    response: &GoogleGenerateContentResponse,
    ctx: &mut StreamProcessingContext<'_>,
    raw_data: &str,
) -> Vec<LanguageModelV4StreamPart> {
    let mut parts = Vec::new();

    // Emit raw chunk if requested
    if ctx.include_raw
        && let Ok(raw_val) = serde_json::from_str::<Value>(raw_data)
    {
        parts.push(LanguageModelV4StreamPart::Raw { raw_value: raw_val });
    }

    let candidate = match response.candidates.first() {
        Some(c) => c,
        None => {
            // Emit usage if present even without candidate
            if let Some(ref usage_meta) = response.usage_metadata {
                let usage = convert_usage(Some(usage_meta));
                parts.push(LanguageModelV4StreamPart::Finish {
                    usage,
                    finish_reason: FinishReason::other(),
                    provider_metadata: None,
                });
            }
            return parts;
        }
    };

    // Track grounding/url metadata for this chunk
    if let Some(ref gm) = candidate.grounding_metadata {
        *ctx.last_grounding_metadata = Some(gm.clone());
    }
    if let Some(ref ucm) = candidate.url_context_metadata {
        *ctx.last_url_context_metadata = Some(ucm.clone());
    }

    if let Some(ref content) = candidate.content {
        for part in &content.parts {
            let ts_meta = thought_signature_metadata(part, ctx.provider_options_name);

            // Handle executable code (before text)
            if let Some(ref exec_code) = part.executable_code {
                let call_id = (ctx.id_gen)();
                *ctx.last_code_execution_tool_call_id = Some(call_id.clone());
                let args = json!({
                    "language": exec_code.language,
                    "code": exec_code.code,
                });
                let args_str = serde_json::to_string(&args).unwrap_or_default();

                parts.push(LanguageModelV4StreamPart::ToolInputStart {
                    id: call_id.clone(),
                    tool_name: "code_execution".to_string(),
                    provider_executed: Some(true),
                    dynamic: None,
                    title: None,
                    provider_metadata: None,
                });
                parts.push(LanguageModelV4StreamPart::ToolInputDelta {
                    id: call_id.clone(),
                    delta: args_str.clone(),
                    provider_metadata: None,
                });
                parts.push(LanguageModelV4StreamPart::ToolInputEnd {
                    id: call_id.clone(),
                    provider_metadata: None,
                });
                parts.push(LanguageModelV4StreamPart::ToolCall(
                    LanguageModelV4ToolCall::new(&call_id, "code_execution", args_str)
                        .with_provider_executed(true),
                ));
            }

            // Handle code execution result
            if let Some(ref exec_result) = part.code_execution_result {
                let call_id = ctx
                    .last_code_execution_tool_call_id
                    .take()
                    .unwrap_or_else(|| (ctx.id_gen)());
                parts.push(LanguageModelV4StreamPart::ToolResult(
                    LanguageModelV4ToolResult::new(
                        &call_id,
                        "code_execution",
                        json!({
                            "outcome": exec_result.outcome,
                            "output": exec_result.output,
                        }),
                    ),
                ));
            }

            // Handle text parts
            if let Some(ref text) = part.text {
                if text.is_empty() {
                    // Empty text with thoughtSignature: emit empty delta to pass metadata
                    if ts_meta.is_some()
                        && let Some(ref tid) = *ctx.text_id
                    {
                        parts.push(LanguageModelV4StreamPart::TextDelta {
                            id: tid.clone(),
                            delta: String::new(),
                            provider_metadata: ts_meta.clone(),
                        });
                    }
                } else if part.thought == Some(true) {
                    // Close any active text block before starting reasoning
                    if let Some(tid) = ctx.text_id.take() {
                        parts.push(LanguageModelV4StreamPart::TextEnd {
                            id: tid,
                            provider_metadata: None,
                        });
                    }
                    // Reasoning text
                    if ctx.reasoning_id.is_none() {
                        let id = (ctx.id_gen)();
                        *ctx.reasoning_id = Some(id.clone());
                        parts.push(LanguageModelV4StreamPart::ReasoningStart {
                            id,
                            provider_metadata: ts_meta.clone(),
                        });
                    }
                    parts.push(LanguageModelV4StreamPart::ReasoningDelta {
                        id: ctx.reasoning_id.clone().unwrap_or_default(),
                        delta: text.clone(),
                        provider_metadata: ts_meta.clone(),
                    });
                } else {
                    // Close reasoning block if transitioning
                    if let Some(rid) = ctx.reasoning_id.take() {
                        parts.push(LanguageModelV4StreamPart::ReasoningEnd {
                            id: rid,
                            provider_metadata: None,
                        });
                    }

                    // Regular text
                    if ctx.text_id.is_none() {
                        let id = (ctx.id_gen)();
                        *ctx.text_id = Some(id.clone());
                        parts.push(LanguageModelV4StreamPart::TextStart {
                            id,
                            provider_metadata: ts_meta.clone(),
                        });
                    }
                    parts.push(LanguageModelV4StreamPart::TextDelta {
                        id: ctx.text_id.clone().unwrap_or_default(),
                        delta: text.clone(),
                        provider_metadata: ts_meta.clone(),
                    });
                }
            }

            // Handle function calls
            if let Some(ref fc) = part.function_call {
                // Close text block if open
                if let Some(tid) = ctx.text_id.take() {
                    parts.push(LanguageModelV4StreamPart::TextEnd {
                        id: tid,
                        provider_metadata: None,
                    });
                }

                let call_id = (ctx.id_gen)();
                let args_str = serde_json::to_string(&fc.args).unwrap_or_default();

                parts.push(LanguageModelV4StreamPart::ToolInputStart {
                    id: call_id.clone(),
                    tool_name: fc.name.clone(),
                    provider_executed: None,
                    dynamic: None,
                    title: None,
                    provider_metadata: ts_meta.clone(),
                });
                parts.push(LanguageModelV4StreamPart::ToolInputDelta {
                    id: call_id.clone(),
                    delta: args_str.clone(),
                    provider_metadata: ts_meta.clone(),
                });
                parts.push(LanguageModelV4StreamPart::ToolInputEnd {
                    id: call_id.clone(),
                    provider_metadata: ts_meta.clone(),
                });
                let mut tc = LanguageModelV4ToolCall::new(&call_id, &fc.name, args_str);
                if let Some(ref meta) = ts_meta {
                    tc.provider_metadata = Some(meta.clone());
                }
                parts.push(LanguageModelV4StreamPart::ToolCall(tc));

                ctx.tool_call_ids.insert(fc.name.clone(), call_id);
            }

            // Handle inline data (images)
            if let Some(ref inline) = part.inline_data {
                // Close active text block
                if let Some(tid) = ctx.text_id.take() {
                    parts.push(LanguageModelV4StreamPart::TextEnd {
                        id: tid,
                        provider_metadata: None,
                    });
                }
                // Close active reasoning block
                if let Some(rid) = ctx.reasoning_id.take() {
                    parts.push(LanguageModelV4StreamPart::ReasoningEnd {
                        id: rid,
                        provider_metadata: None,
                    });
                }
                if part.thought == Some(true) {
                    parts.push(LanguageModelV4StreamPart::ReasoningFile(
                        StreamReasoningFile {
                            data: inline.data.clone(),
                            media_type: inline.mime_type.clone(),
                            provider_metadata: ts_meta.clone(),
                        },
                    ));
                } else {
                    parts.push(LanguageModelV4StreamPart::File(StreamFile {
                        data: inline.data.clone(),
                        media_type: inline.mime_type.clone(),
                        provider_metadata: ts_meta.clone(),
                    }));
                }
            }
        }
    }

    // Extract sources using shared logic, with streaming dedup via seen_source_urls
    let sources = extract_sources_with_dedup(
        &candidate.grounding_metadata,
        &candidate.url_context_metadata,
        ctx.id_gen.as_ref(),
        ctx.seen_source_urls,
    );
    for source in sources {
        parts.push(LanguageModelV4StreamPart::Source(source));
    }

    // Handle finish
    if let Some(ref finish_str) = candidate.finish_reason {
        // Exclude provider-executed tool calls from has_tool_calls check
        let has_tool_calls = !ctx.tool_call_ids.is_empty();
        let finish_reason = crate::map_google_generative_ai_finish_reason::map_finish_reason(
            Some(finish_str),
            has_tool_calls,
        );

        let usage = convert_usage(response.usage_metadata.as_ref());

        // Build provider metadata under namespace key — always include all 6 fields
        let mut namespace_meta = HashMap::new();
        namespace_meta.insert(
            "promptFeedback".to_string(),
            response.prompt_feedback.clone().unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "groundingMetadata".to_string(),
            ctx.last_grounding_metadata
                .as_ref()
                .and_then(|gm| serde_json::to_value(gm).ok())
                .unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "urlContextMetadata".to_string(),
            ctx.last_url_context_metadata
                .as_ref()
                .and_then(|ucm| serde_json::to_value(ucm).ok())
                .unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "safetyRatings".to_string(),
            candidate.safety_ratings.clone().unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "usageMetadata".to_string(),
            response
                .usage_metadata
                .as_ref()
                .and_then(|um| serde_json::to_value(um).ok())
                .unwrap_or(Value::Null),
        );
        namespace_meta.insert(
            "finishMessage".to_string(),
            candidate
                .finish_message
                .as_ref()
                .map(|fm| Value::String(fm.clone()))
                .unwrap_or(Value::Null),
        );

        let provider_metadata = {
            let mut outer = HashMap::new();
            outer.insert(
                ctx.provider_options_name.to_string(),
                serde_json::to_value(&namespace_meta).unwrap_or(Value::Null),
            );
            Some(ProviderMetadata::from_map(outer))
        };

        // Close open blocks before finish
        if let Some(tid) = ctx.text_id.take() {
            parts.push(LanguageModelV4StreamPart::TextEnd {
                id: tid,
                provider_metadata: None,
            });
        }
        if let Some(rid) = ctx.reasoning_id.take() {
            parts.push(LanguageModelV4StreamPart::ReasoningEnd {
                id: rid,
                provider_metadata: None,
            });
        }

        // Emit model version as response metadata
        if let Some(ref model_version) = response.model_version {
            parts.push(LanguageModelV4StreamPart::ResponseMetadata(
                ResponseMetadata::new().with_model(model_version),
            ));
        }

        parts.push(LanguageModelV4StreamPart::Finish {
            usage,
            finish_reason,
            provider_metadata,
        });
    }

    parts
}

/// Whether the model ID corresponds to a Gemini 3 family model.
fn is_gemini_3_model(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    // Match "gemini-3" followed by a separator or end of string
    lower.starts_with("gemini-3.") || lower.starts_with("gemini-3-") || lower == "gemini-3"
}

/// Max output tokens constant for Gemini 2.5 budget calculation.
fn max_output_tokens_for_gemini_25() -> i64 {
    65536
}

/// Max thinking tokens for a Gemini 2.5 model.
fn max_thinking_tokens_for_gemini_25(model_id: &str) -> i64 {
    let lower = model_id.to_lowercase();
    if lower.contains("2.5-pro") || lower.contains("gemini-3-pro-image") {
        32768
    } else {
        24576
    }
}

/// Resolve top-level reasoning to a Google thinking config.
///
/// For Gemini 3 models (excluding gemini-3-pro-image): maps to `thinkingLevel`.
/// For Gemini 2.5 models: maps to `thinkingBudget`.
fn resolve_thinking_config(
    reasoning: Option<ReasoningLevel>,
    model_id: &str,
    warnings: &mut Vec<Warning>,
) -> Option<crate::google_generative_ai_options::ThinkingConfig> {
    if !is_custom_reasoning(reasoning) {
        return None;
    }
    let level = reasoning?;

    if is_gemini_3_model(model_id) && !model_id.contains("gemini-3-pro-image") {
        return resolve_gemini_3_thinking_config(level, warnings);
    }

    resolve_gemini_25_thinking_config(level, model_id, warnings)
}

fn resolve_gemini_3_thinking_config(
    level: ReasoningLevel,
    warnings: &mut Vec<Warning>,
) -> Option<crate::google_generative_ai_options::ThinkingConfig> {
    use crate::google_generative_ai_options::ThinkingConfig;
    use crate::google_generative_ai_options::ThinkingLevel as GoogleThinkingLevel;

    if level == ReasoningLevel::Off {
        // Cannot fully disable thinking on Gemini 3; use minimal.
        return Some(ThinkingConfig {
            thinking_budget: None,
            include_thoughts: None,
            thinking_level: Some(GoogleThinkingLevel::Minimal),
        });
    }

    let effort_map = HashMap::from([
        (ReasoningLevel::Minimal, "minimal"),
        (ReasoningLevel::Low, "low"),
        (ReasoningLevel::Medium, "medium"),
        (ReasoningLevel::High, "high"),
        (ReasoningLevel::Xhigh, "high"),
    ]);

    let thinking_level_str = map_reasoning_to_provider_effort(level, &effort_map, warnings)?;
    let thinking_level = match thinking_level_str.as_str() {
        "minimal" => GoogleThinkingLevel::Minimal,
        "low" => GoogleThinkingLevel::Low,
        "medium" => GoogleThinkingLevel::Medium,
        "high" => GoogleThinkingLevel::High,
        _ => return None,
    };

    Some(ThinkingConfig {
        thinking_budget: None,
        include_thoughts: None,
        thinking_level: Some(thinking_level),
    })
}

fn resolve_gemini_25_thinking_config(
    level: ReasoningLevel,
    model_id: &str,
    warnings: &mut Vec<Warning>,
) -> Option<crate::google_generative_ai_options::ThinkingConfig> {
    use crate::google_generative_ai_options::ThinkingConfig;

    if level == ReasoningLevel::Off {
        return Some(ThinkingConfig {
            thinking_budget: Some(0),
            include_thoughts: None,
            thinking_level: None,
        });
    }

    let budget = map_reasoning_to_provider_budget(
        level,
        max_output_tokens_for_gemini_25(),
        max_thinking_tokens_for_gemini_25(model_id),
        Some(0),
        None,
        warnings,
    )?;

    Some(ThinkingConfig {
        thinking_budget: Some(budget as u64),
        include_thoughts: None,
        thinking_level: None,
    })
}

/// Local mirror of `coco_inference::PromptLayoutOptions` (Google slot
/// only). Provider crates do NOT depend on `coco-inference`; the wire
/// shape under `provider_options["prompt_layout"]` is the cross-layer
/// contract, and this struct deserializes it locally.
#[derive(serde::Deserialize, Default)]
struct PromptLayoutWire {
    #[serde(default)]
    system_instruction: Option<String>,
}

fn parse_prompt_layout_namespace(
    provider_options: &Option<vercel_ai_provider::ProviderOptions>,
) -> Option<PromptLayoutWire> {
    let opts = provider_options.as_ref()?;
    let inner = opts.get("prompt_layout")?;
    let mut object = serde_json::Map::new();
    for (key, value) in inner {
        object.insert(key.clone(), value.clone());
    }
    serde_json::from_value(Value::Object(object)).ok()
}

#[cfg(test)]
#[path = "google_generative_ai_language_model.test.rs"]
mod tests;
