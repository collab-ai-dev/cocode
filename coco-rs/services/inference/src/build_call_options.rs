//! `build_call_options` — schema validation entry that constructs a fresh
//! `LanguageModelV4CallOptions` per turn.
//!
//! This is the **single ProviderOptions write site** in the entire
//! Coco codebase. Every other place reads `ModelInfo.extra_body`. The
//! function:
//!
//! 1. Wires typed sampling fields (`temperature`, `top_p`, `top_k`,
//!    `max_output_tokens`) — `None` means "let provider default",
//!    carried through to the wire body unchanged.
//! 2. Maps the reasoning channel — `ThinkingLevel.effort` flows
//!    through `call.reasoning`; budget / interleaved / summary go
//!    through `extra_body` via [`thinking_convert::to_extra_body`].
//!    `Some(level)` with `effort == None` disables thinking entirely
//!    rather than falling through to the model default.
//! 3. Shallow-merges `info.extra_body` ⊕ `per_call.extra_body` ⊕
//!    `thinking_extra` (per-call wins; thinking last) into a single
//!    flat `BTreeMap`.
//! 4. Wraps the merged map under
//!    `provider_options[<canonical_namespace>]`. The namespace key is
//!    derived from the [`ProviderApi`] for builtin SDKs (where
//!    `model.provider()` returns a hardcoded name) and from
//!    `ProviderConfig.name` for OpenAI-compat / Volcengine / Z.AI
//!    instances (where the SDK pass-through honors the configured
//!    `provider_id`).
//!
//! No `as u64` casts: `PositiveTokens` / `PositiveCount` provide
//! `From → u64` infallibly (see `coco_config::positive`).

use crate::cache_convert;
use crate::thinking_convert;
use coco_llm_types::LlmPrompt;
use coco_llm_types::ProviderOptions;
use coco_llm_types::ReasoningLevel;
use coco_types::Capability;
use coco_types::PromptCacheConfig;
use coco_types::ProviderApi;
use coco_types::ReasoningEffort;
use coco_types::ThinkingLevel;
use std::collections::BTreeMap;
use std::collections::HashMap;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider_utils::merge_json_value;

/// Per-call deltas applied on top of the resolved `ModelInfo`. Each
/// field overrides the corresponding model-level value when `Some`.
/// `extra_body` keys merge with model-level keys; per-call wins.
#[derive(Debug, Clone, Default)]
pub struct PerCallOverrides {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<coco_config::PositiveCount>,
    pub max_output_tokens: Option<coco_config::PositiveTokens>,
    /// Per-turn thinking override.
    /// - `None` — fall through to `info.default_thinking()`.
    /// - `Some(level)` with `effort == ReasoningEffort::Disable` —
    ///   explicitly disable thinking for this turn (must NOT silently
    ///   fall through to the model default).
    /// - `Some(level)` with `effort == ReasoningEffort::Auto` —
    ///   explicitly defer to the provider's server-side default.
    /// - `Some(level)` with `effort.is_explicit_level()` — use this
    ///   numeric level (Minimal..XHigh) on the typed reasoning lane.
    pub thinking_level: Option<ThinkingLevel>,
    pub extra_body: BTreeMap<String, serde_json::Value>,
    /// Anthropic `context_management` payload (camelCase wire shape).
    /// Encoded by `coco_compact::encode_anthropic_context_management`
    /// from the resolved `[ContextEditStrategy]` list. The Anthropic
    /// language model's `extract_anthropic_options` reads it; other
    /// providers ignore the namespace, so the field is a no-op there.
    pub context_management: Option<serde_json::Value>,
    /// Layer-A user intent for prompt caching. Translated to camelCase
    /// `provider_options` keys via [`cache_convert::to_extra_body`];
    /// non-Anthropic providers see no caching keys.
    pub cache_strategy: Option<PromptCacheConfig>,
    /// Per-call agentic-loop flag — gates the `claude-code-20250219`
    /// baseline beta in the Anthropic adapter. Helper calls (compaction,
    /// title generation) pass `false`; main agent loop passes `true`.
    pub agentic_query: bool,
    /// Per-call query source — used for 1h-TTL allowlist match.
    /// Forwarded to the adapter only when a non-disabled
    /// `cache_strategy` is also present (design §9.2 Finding 4).
    pub query_source: Option<String>,
    /// Generation stop sequences forwarded to the provider. Used by the
    /// auto-mode classifier (`</block>` after stage-1 verdict) and any
    /// helper call that wants the model to terminate early on a marker.
    /// Each provider crate maps to its native wire shape:
    ///   * Anthropic → `stop_sequences: [...]`
    ///   * OpenAI Chat / OpenAI-Compatible → `stop: [...]`
    ///   * Gemini → `stopSequences: [...]`
    pub stop_sequences: Option<Vec<String>>,
    /// Per-call fast-mode flag (config#247). When set and the provider is
    /// Anthropic, `build_call_options` writes `speed=fast` into
    /// `provider_options["anthropic"]`, which the adapter turns into the
    /// `fast-mode-2026-02-01` beta. Capability gating happens upstream (the
    /// engine only sets this when the resolved model declares
    /// `Capability::FastMode`); other providers ignore the flag.
    pub fast_mode: bool,
}

/// Build a fresh `LanguageModelV4CallOptions` for a turn, returning
/// the merged flat `extra_body` map alongside the call options.
///
/// The returned `BTreeMap` is the **post-merge, pre-namespace-wrap**
/// flat map that gets installed under
/// `provider_options[<canonical_namespace>]`. It is the canonical
/// input for `cache_detection::build_prompt_state_input` — the
/// detector hashes whatever the wire body actually carries, with no
/// per-feature plumbing required for new keys (cache_strategy,
/// requestedBetas, agenticQuery, querySource, …).
///
/// Callers that don't need the merged map (mock paths, simple
/// integration callsites) should use [`build_call_options`] instead.
pub fn build_call_options_with_extra(
    info: &coco_config::ModelInfo,
    api: ProviderApi,
    provider_name: &str,
    per_call: &PerCallOverrides,
    prompt: LlmPrompt,
    tools: Option<Vec<LanguageModelV4Tool>>,
) -> (
    LanguageModelV4CallOptions,
    BTreeMap<String, serde_json::Value>,
) {
    let mut call = LanguageModelV4CallOptions {
        prompt,
        ..Default::default()
    };
    call.tools = tools;
    // Tool-call argument parsing lives in the provider adapter now.
    // Each adapter calls `vercel_ai_provider_utils::parse_tool_arguments_or_empty`
    // (which wraps `llm_json::repair_json`) directly when constructing
    // `ToolCallPart`. The previous workspace-level `tool_input_parse_fn`
    // injection has been removed — there is no longer a per-call
    // pluggable parser hook.

    // Lane A: typed sampling. `None` semantically means "let provider
    // default" — every typed body builder writes the field only on
    // `Some`, so omitting it preserves provider-tuned defaults.
    call.temperature = per_call.temperature.or(info.temperature);
    call.top_p = per_call.top_p.or(info.top_p);
    call.top_k = per_call.top_k.or(info.top_k).map(u64::from);
    call.max_output_tokens = per_call
        .max_output_tokens
        .map(u64::from)
        .or_else(|| Some(u64::from(info.max_output_tokens)));
    if let Some(stops) = per_call.stop_sequences.as_ref()
        && !stops.is_empty()
    {
        call.stop_sequences = Some(stops.clone());
    }

    // Lane A2: typed reasoning channel.
    //
    // Resolution semantics:
    //   per_call.thinking_level == Some(t)                        → use t verbatim
    //   per_call.thinking_level == None                           → info.default_thinking()
    //
    // The typed `call.reasoning` slot is only set for explicit numeric
    // efforts (Minimal..XHigh). `Disable` and `Auto` leave it `None`
    // so the wire body omits any typed reasoning hint — `Disable` may
    // still emit an explicit-off toggle via `level.options`, and `Auto`
    // lets the server-side default apply.
    //
    // Critically, an explicit per-call `Disable` must NOT silently fall
    // through to the model default — that would let a turn disable
    // thinking only for it to come back via the model.
    // Clamp an explicit per-call numeric effort into the model's declared
    // `supported_thinking_levels` ladder before it reaches the wire.
    // `resolve_thinking_level`: exact match → nearest declared level →
    // verbatim passthrough when the model declares no ladder (trust the
    // caller). For an over-ceiling request (e.g. `--effort max` / XHigh on a
    // model whose top declared level is High) nearest-match lands on the top
    // declared level — a graceful degrade instead of emitting
    // `output_config.effort="max"`, which 400s on models that reject it.
    // This is the single wire choke point, so it covers every explicit
    // override source uniformly (CLI `--effort`, SDK `set_thinking_level`,
    // subagent frontmatter, the model picker) against the *resolved*
    // `ModelInfo` for this specific call. Only explicit levels are clamped:
    // `Off`/`Auto` are not numeric levels (they mean "off" / "defer to the
    // provider default") and must bypass the effort-distance math, which
    // would otherwise snap them to the nearest declared numeric level.
    let thinking: Option<ThinkingLevel> = match per_call.thinking_level.as_ref() {
        Some(t) if t.effort.is_explicit_level() => Some(info.resolve_thinking_level(t)),
        Some(t) => Some(t.clone()),
        None => info.default_thinking().cloned(),
    };
    if let Some(t) = thinking.as_ref()
        && t.effort.is_explicit_level()
    {
        call.reasoning = Some(reasoning_effort_to_level(t.effort));
    }

    // Lane B: deep-merge extra_body and wrap under the canonical
    // namespace. `BTreeMap` so key order in tests / snapshots is
    // deterministic.
    //
    // Per-call values layer onto model-level via [`merge_json_value`]:
    // nested objects merge recursively, arrays/primitives replace.
    // Reuses vercel-ai's helper so `__proto__` / `constructor` /
    // `prototype` pollution is filtered, and the merge semantics stay
    // identical to what `vercel_ai::generate_text` does for its
    // per-step `prepare_step` provider-option overrides.
    let mut extra: BTreeMap<String, serde_json::Value> = info.extra_body.clone();
    for (k, v) in &per_call.extra_body {
        merge_into_extra(&mut extra, k, v);
    }
    if let Some(t) = thinking.as_ref() {
        // Disabled levels (effort == None) still flow `level.options`
        // through — DeepSeek V4 emits `{"thinking":{"type":"disabled"}}`
        // when off. The Lane A2 typed-reasoning gate above (`call.reasoning`)
        // remains gated on effort != None.
        //
        // `info.capabilities` gates Anthropic adaptive thinking — `Auto`
        // emits `{type:adaptive}` only when `Capability::AdaptiveThinking`
        // is declared. Empty / None capabilities → safe degradation
        // (server default applies).
        let caps = info.capabilities.as_deref().unwrap_or(&[]);
        for (k, v) in thinking_convert::to_extra_body(t, api, caps) {
            merge_into_extra(&mut extra, &k, &v);
        }
    }

    // Lane C: Anthropic-only `context_management`. The provider's
    // `extract_anthropic_options` reads it from the
    // `provider_options["anthropic"]["contextManagement"]` slot; other
    // providers don't, so we skip the key entirely.
    //
    // Precedence: when the user has pre-stuffed `contextManagement`
    // into `extra_body` (wire parsing escape hatch), don't overwrite — log
    // and keep the user's value. This makes the manual override stable
    // even after we begin computing strategies upstream.
    if api == ProviderApi::Anthropic
        && let Some(ctx) = per_call.context_management.clone()
    {
        if extra.contains_key("contextManagement") {
            tracing::debug!(
                "build_call_options: user-supplied contextManagement in extra_body \
                 takes precedence over coco-computed strategy"
            );
        } else {
            extra.insert("contextManagement".to_string(), ctx);
        }
    }

    // Lane C2: Anthropic-only fast mode (config#247). The provider's
    // `extract_anthropic_options` reads `provider_options["anthropic"]["speed"]`
    // and emits the `fast-mode-2026-02-01` beta when it's `"fast"`. Capability
    // gating happens upstream (the engine sets `per_call.fast_mode` only when
    // the resolved model declares `Capability::FastMode`); here we just
    // translate the resolved flag into the Anthropic-specific wire option.
    // Other providers have no speed concept and ignore the flag. A
    // user-supplied `speed` in `extra_body` takes precedence (escape hatch).
    if api == ProviderApi::Anthropic && per_call.fast_mode && !extra.contains_key("speed") {
        extra.insert(
            "speed".to_string(),
            serde_json::Value::String("fast".to_string()),
        );
    }

    // Lane D: prompt-cache pass-through. Non-Anthropic / disabled →
    // no-op. Adapter (`vercel-ai-anthropic`) owns all policy
    // interpretation; this site only forwards typed user intent
    // (design §9.2 / §9.4).
    if let Some(ref cache_cfg) = per_call.cache_strategy {
        for (k, v) in cache_convert::to_extra_body(cache_cfg, api) {
            merge_into_extra(&mut extra, &k, &v);
        }
    }
    // Lane D2: per-call session context. Gated on a non-disabled
    // cache strategy — without the gate, `query_source` would re-hash
    // `extra_body_hash` for callers that never enabled caching
    // (design §9.2 / Finding 4).
    for (k, v) in cache_convert::session_context_to_extra_body(
        per_call.cache_strategy.as_ref(),
        per_call.agentic_query,
        per_call.query_source.as_deref(),
        api,
    ) {
        merge_into_extra(&mut extra, &k, &v);
    }

    // Lane E: parallel tool-call capability gate.
    //
    // Sets the **provider-agnostic** `LanguageModelV4CallOptions.parallel_tool_calls`
    // toggle. Each provider crate owns the wire translation:
    //   * `vercel-ai-openai` (Chat + Responses) → top-level
    //     `parallel_tool_calls: true` in the request body.
    //   * `vercel-ai-anthropic` → nested
    //     `tool_choice.disable_parallel_tool_use: false` (inverted
    //     polarity, applied by `prepare_anthropic_tools`).
    //   * `vercel-ai-openai-compatible` → top-level
    //     `parallel_tool_calls: true` (matches OpenAI wire shape).
    //   * `vercel-ai-google` → no-op (Gemini Function Calling is
    //     implicitly parallel).
    //
    // The inference layer therefore has no per-provider knowledge here.
    // Typed `provider_options` overrides emitted via Lane B (e.g.
    // user-set `provider_options.openai.parallelToolCalls`) still win
    // because each provider crate prefers its typed slot over this
    // generic flag.
    if info
        .capabilities
        .as_deref()
        .unwrap_or(&[])
        .contains(&Capability::ParallelToolCalls)
    {
        call.parallel_tool_calls = Some(true);
    }

    // Snapshot the merged flat map *before* namespace-wrapping so the
    // detector hash and the actual retry body cannot drift. The
    // returned map is the post-merge, pre-wrap canonical extra body.
    let merged_snapshot = extra.clone();

    if !extra.is_empty() {
        let mut po = ProviderOptions::default();
        let inner: HashMap<String, serde_json::Value> = extra.into_iter().collect();
        po.set(canonical_namespace_key(api, provider_name), inner);
        call.provider_options = Some(po);
    }

    (call, merged_snapshot)
}

/// Convenience wrapper around [`build_call_options_with_extra`] that
/// discards the merged extra map. Use when you don't need to feed
/// the detector hash (mock / test / simple integration callsites).
pub fn build_call_options(
    info: &coco_config::ModelInfo,
    api: ProviderApi,
    provider_name: &str,
    per_call: &PerCallOverrides,
    prompt: LlmPrompt,
    tools: Option<Vec<LanguageModelV4Tool>>,
) -> LanguageModelV4CallOptions {
    build_call_options_with_extra(info, api, provider_name, per_call, prompt, tools).0
}

/// Resolve the namespace key the language-model implementation will
/// read from `call.provider_options`.
///
/// - `Anthropic` / `Openai` / `Gemini` — SDK hardcodes the family
///   name (`"anthropic"`, `"openai"`, `"google"`) regardless of
///   `ProviderSettings.provider_id`. The wrap key MUST match.
/// - `OpenaiCompat` / `Volcengine` / `Zai` — SDK passes the
///   `provider_id` through; the runtime instance name (e.g.
///   `"azure-east"`, `"xai"`, `"volcengine"`) is what
///   `model.provider()` returns.
pub fn canonical_namespace_key(api: ProviderApi, provider_name: &str) -> &str {
    match api {
        ProviderApi::Anthropic => "anthropic",
        ProviderApi::Openai => "openai",
        ProviderApi::Gemini => "google",
        ProviderApi::OpenaiCompat | ProviderApi::Volcengine | ProviderApi::Zai => provider_name,
    }
}

/// Insert `value` at `key` in `extra`, deep-merging when both the
/// existing entry and `value` are JSON objects. Drops in delegation to
/// vercel-ai's [`merge_json_value`] so semantics (recursive object
/// merge, primitive/array replacement, `__proto__` filtering) match the
/// AI layer's per-step ProviderOptions merge byte for byte.
fn merge_into_extra(
    extra: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    value: &serde_json::Value,
) {
    match extra.get(key) {
        Some(existing) => {
            extra.insert(key.to_string(), merge_json_value(existing, value));
        }
        None => {
            extra.insert(key.to_string(), value.clone());
        }
    }
}

/// Map a coco-types `ReasoningEffort` to the vercel-ai
/// `ReasoningLevel` that flows through `LanguageModelV4CallOptions.reasoning`.
///
/// Precondition: `effort.is_explicit_level()` — the only call site
/// (Lane A2 in `build_call_options_with_extra`) gates on this. `Disable`
/// and `Auto` mean "don't emit a typed reasoning hint" and must be
/// handled by leaving `call.reasoning = None` at the call site, not by
/// translating into a vercel-ai level here.
fn reasoning_effort_to_level(effort: ReasoningEffort) -> ReasoningLevel {
    match effort {
        ReasoningEffort::Minimal => ReasoningLevel::Minimal,
        ReasoningEffort::Low => ReasoningLevel::Low,
        ReasoningEffort::Medium => ReasoningLevel::Medium,
        ReasoningEffort::High => ReasoningLevel::High,
        ReasoningEffort::XHigh => ReasoningLevel::Xhigh,
        ReasoningEffort::Off | ReasoningEffort::Auto => unreachable!(
            "reasoning_effort_to_level called with non-explicit effort {effort:?}; \
             Lane A2 must gate on `effort.is_explicit_level()` before invoking this"
        ),
    }
}

#[cfg(test)]
#[path = "build_call_options.test.rs"]
mod tests;
