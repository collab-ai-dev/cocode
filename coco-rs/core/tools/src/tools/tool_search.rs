//! ToolSearchTool — keyword search and direct selection for deferred tools.
//!
//! ## Two query modes
//!
//! 1. **Direct selection** — `select:Tool1,Tool2,Tool3` (case-insensitive
//!    prefix). The model explicitly names the deferred tools it wants
//!    "unlocked". Comma-separated, whitespace-tolerant. Missing names
//!    are silently dropped; a name already present in the regular pool
//!    resolves harmlessly. Returns the resolved subset in `matches`.
//!
//! 2. **Keyword search** — any other query. Splits on whitespace; tokens
//!    starting with `+` are *required* (the candidate must match all
//!    `+terms`); the remaining tokens are *optional* (contribute to the
//!    score). Score formula:
//!
//!    | Match | Score |
//!    |---|---|
//!    | exact part hit (`parts.contains(term)`) | +12 MCP / +10 regular |
//!    | substring of a part (`part.contains(term)`) | +6 MCP / +5 regular |
//!    | full-name fallback (`full.contains(term) && score == 0`) | +3 |
//!    | `search_hint` word-boundary regex hit | +4 |
//!    | description word-boundary regex hit | +2 |
//!
//!    The candidate list is filtered to tools matching ALL required
//!    terms (when any are supplied) before scoring; ranked descending,
//!    capped at `max_results`.
//!
//! ## Promotion mechanism (multi-provider divergence)
//!
//! The Anthropic `tool_reference` content-block beta expands into
//! `<functions>...</functions>` markup inline on the next turn, but this
//! is provider-specific. For all other providers we instead emit an
//! `AppStatePatch` that inserts each matched name into
//! [`coco_types::ToolAppState::discovered_tool_names`].
//! On the next turn, `engine_prompt::build_tool_definitions` and the
//! `DeferredToolsDeltaGenerator` both observe the patch via
//! `ToolUseContext::discovered_tool_names`:
//!
//!   - **Definitions build** — `ToolRegistry::loaded_tools` upgrades
//!     discovered deferred tools into the loaded pool, so their full
//!     schema is sent in the next request (model can invoke them).
//!   - **Reminder** — `DeferredToolsDeltaGenerator` sees a non-empty
//!     `added` set in `compute_tools_delta` and emits a
//!     `<system-reminder>` announcing the new tools.
//!
//! Net effect: the model sees the same "tool became callable next turn"
//! signal it would on Anthropic, with no provider-specific dependency.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::SchemaContext;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolSpec;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use regex::Regex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

const DEFAULT_MAX_RESULTS: usize = 5;

/// MCP wire prefix used by [`parse_tool_name`] to detect MCP tools.
/// Centralized in [`coco_types::MCP_TOOL_PREFIX`]; duplicated here as
/// `&'static str` for `const`-context use.
const MCP_PREFIX: &str = "mcp__";

const PROMPT_HEAD: &str =
    "Fetches full schema definitions for deferred tools so they can be called.\n\n";

const PROMPT_TAIL: &str = " Until fetched, only the name is known — there is no parameter schema, so the tool cannot be invoked. This tool takes a query, matches it against the deferred tool list, and returns the matched tools' complete JSONSchema definitions inside a <functions> block. Once a tool's schema appears in that result, it is callable exactly like any tool defined at the top of the prompt.\n\nResult format: each matched tool appears as one <function>{\"description\":\"...\",\"name\":\"...\",\"parameters\":{...}}</function> line inside the <functions> block — the same encoding as the tool list at the top of this prompt.\n\nQuery forms:\n- \"select:Read,Edit,Grep\" — fetch these exact tools by name\n- \"notebook jupyter\" — keyword search, up to max_results best matches\n- \"+slack send\" — require \"slack\" in the name, rank by remaining terms";

/// Deferred tools appear by name in `<system-reminder>` messages.
const PROMPT_LOCATION_HINT: &str = "Deferred tools appear by name in <system-reminder> messages.";

/// Parse a `select:Tool1,Tool2,...` query into a list of tool names.
/// Returns `None` if the query isn't in select mode. Whitespace around
/// each name is trimmed; empty names are dropped.
///
/// **Prefix is case-insensitive** — `select:`, `Select:`, `SELECT:` all
/// trigger select mode (case-insensitive prefix check).
pub(super) fn parse_select_query(query: &str) -> Option<Vec<String>> {
    // Case-insensitive prefix match: if the first 7 chars (lowercased)
    // equal `"select:"`, strip them. Otherwise return None.
    if query.len() < 7 {
        return None;
    }
    let prefix = &query[..7];
    if !prefix.eq_ignore_ascii_case("select:") {
        return None;
    }
    let rest = &query[7..];
    Some(
        rest.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// Tool-name decomposition used for the keyword-scoring path.
///
///   - MCP wire-name `mcp__server__action_subaction` → `is_mcp = true`,
///     `parts = ["server", "action", "subaction"]`, `full = "server
///     action subaction"`. The `mcp__` prefix is stripped; remaining
///     `__` are treated as part separators, then each part is further
///     split on `_`.
///   - Regular name `CamelCaseTool` → `is_mcp = false`,
///     `parts = ["camel", "case", "tool"]`, `full = "camel case tool"`.
///     `[a-z][A-Z]` boundaries are split into separate parts; `_` is
///     also a separator.
#[derive(Debug, Clone)]
struct ParsedToolName {
    parts: Vec<String>,
    full: String,
    is_mcp: bool,
}

fn parse_tool_name(name: &str) -> ParsedToolName {
    if let Some(rest) = name.strip_prefix(MCP_PREFIX) {
        let lower = rest.to_lowercase();
        let parts: Vec<String> = lower
            .split("__")
            .flat_map(|p| p.split('_'))
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        let full = lower.replace("__", " ").replace('_', " ");
        return ParsedToolName {
            parts,
            full,
            is_mcp: true,
        };
    }

    // Insert a space between lower→upper transitions (CamelCase → spaced),
    // then replace `_` with space, lowercase, and split on whitespace.
    let mut spaced = String::with_capacity(name.len() * 2);
    let mut prev_is_lower = false;
    for ch in name.chars() {
        if prev_is_lower && ch.is_ascii_uppercase() {
            spaced.push(' ');
        }
        spaced.push(ch);
        prev_is_lower = ch.is_ascii_lowercase();
    }
    let spaced = spaced.replace('_', " ").to_lowercase();
    let parts: Vec<String> = spaced.split_whitespace().map(str::to_string).collect();
    let full = parts.join(" ");
    ParsedToolName {
        parts,
        full,
        is_mcp: false,
    }
}

/// Pre-compile word-boundary regexes for the search terms. Returns
/// `None` for any term that fails to compile (e.g. a term consisting
/// entirely of regex metacharacters — `escape` guarantees this won't
/// happen, but we still tolerate it).
fn compile_term_patterns(terms: &[String]) -> HashMap<String, Regex> {
    let mut patterns = HashMap::with_capacity(terms.len());
    for term in terms {
        if patterns.contains_key(term) {
            continue;
        }
        let pattern = format!(r"\b{}\b", regex::escape(term));
        if let Ok(re) = Regex::new(&pattern) {
            patterns.insert(term.clone(), re);
        }
    }
    patterns
}

/// One matched tool from the keyword path.
#[derive(Debug, Clone)]
struct ScoredTool {
    name: String,
    score: i32,
}

/// Score a deferred tool against pre-tokenized search terms. Returns
/// the raw score; the caller filters out `score <= 0` and sorts.
fn score_tool(
    tool: &dyn DynTool,
    parsed: &ParsedToolName,
    desc_lower: &str,
    hint_lower: &str,
    terms: &[String],
    patterns: &HashMap<String, Regex>,
) -> i32 {
    let _ = tool;
    let mut score: i32 = 0;
    for term in terms {
        // Exact part match — high weight (MCP servers / regular tool
        // name parts are the strongest signal).
        if parsed.parts.iter().any(|p| p == term) {
            score += if parsed.is_mcp { 12 } else { 10 };
        } else if parsed.parts.iter().any(|p| p.contains(term)) {
            // Substring of a part — model often types prefixes.
            score += if parsed.is_mcp { 6 } else { 5 };
        }

        // Full-name fallback — only if no part match landed. The check
        // runs per-term so the first hit captures the fallback bonus.
        if score == 0 && parsed.full.contains(term) {
            score += 3;
        }

        // search_hint word-boundary regex — curated capability
        // phrase, higher signal than description.
        if !hint_lower.is_empty()
            && let Some(re) = patterns.get(term)
            && re.is_match(hint_lower)
        {
            score += 4;
        }

        // Description word-boundary regex — avoid false positives
        // from short prefixes (e.g. "task" matching "tasking").
        if let Some(re) = patterns.get(term)
            && re.is_match(desc_lower)
        {
            score += 2;
        }
    }
    score
}

/// Run the keyword path over the deferred-tool list.
fn search_with_keywords(
    deferred: &[Arc<dyn DynTool>],
    all: &[Arc<dyn DynTool>],
    desc_opts: &DescriptionOptions,
    query: &str,
    max_results: usize,
) -> Vec<String> {
    let query_lower = query.to_lowercase();
    let query_trimmed = query_lower.trim();

    // Fast path 1: exact name match (deferred first, then full set).
    // Selecting an already-loaded tool is a harmless no-op that lets
    // the model proceed without retry churn.
    if let Some(t) = deferred
        .iter()
        .find(|t| t.name().eq_ignore_ascii_case(query_trimmed))
        .or_else(|| {
            all.iter()
                .find(|t| t.name().eq_ignore_ascii_case(query_trimmed))
        })
    {
        return vec![t.name().to_string()];
    }

    // Fast path 2: `mcp__<server>` prefix — returns up to `max_results`
    // MCP tools whose qualified name starts with the query. Length > 5
    // guards against the bare `mcp__` query.
    if query_trimmed.starts_with(MCP_PREFIX) && query_trimmed.len() > MCP_PREFIX.len() {
        let hits: Vec<String> = deferred
            .iter()
            .filter(|t| t.name().to_lowercase().starts_with(query_trimmed))
            .take(max_results)
            .map(|t| t.name().to_string())
            .collect();
        if !hits.is_empty() {
            return hits;
        }
    }

    // Tokenize: split on whitespace, partition into required (`+term`)
    // and optional. Empty `+` (length 1) is treated as a non-required
    // token to avoid creating an unmatchable empty required term.
    let tokens: Vec<&str> = query_trimmed
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    let mut required: Vec<String> = Vec::new();
    let mut optional: Vec<String> = Vec::new();
    for token in &tokens {
        if let Some(rest) = token.strip_prefix('+')
            && !rest.is_empty()
        {
            required.push(rest.to_string());
        } else {
            optional.push(token.to_string());
        }
    }

    // Scoring terms = required followed by optional when any required;
    // otherwise just all tokens.
    let scoring_terms: Vec<String> = if required.is_empty() {
        tokens.iter().map(|s| (*s).to_string()).collect()
    } else {
        let mut all_terms = required.clone();
        all_terms.extend(optional.iter().cloned());
        all_terms
    };
    if scoring_terms.is_empty() {
        return Vec::new();
    }
    let patterns = compile_term_patterns(&scoring_terms);

    // Precompute description + hint for each deferred tool so the
    // pre-filter and the scoring pass don't both call `description`.
    struct ToolWithText {
        tool: Arc<dyn DynTool>,
        parsed: ParsedToolName,
        desc_lower: String,
        hint_lower: String,
    }
    let prepared: Vec<ToolWithText> = deferred
        .iter()
        .map(|t| {
            let parsed = parse_tool_name(t.name());
            let desc_lower = t.description(&Value::Null, desc_opts).to_lowercase();
            let hint_lower = t.search_hint().map(str::to_lowercase).unwrap_or_default();
            ToolWithText {
                tool: t.clone(),
                parsed,
                desc_lower,
                hint_lower,
            }
        })
        .collect();

    // Pre-filter: require ALL `+term` matches on parts OR description
    // OR search_hint.
    let candidates: Vec<&ToolWithText> = if required.is_empty() {
        prepared.iter().collect()
    } else {
        prepared
            .iter()
            .filter(|tw| {
                required.iter().all(|term| {
                    if tw.parsed.parts.iter().any(|p| p == term) {
                        return true;
                    }
                    if tw.parsed.parts.iter().any(|p| p.contains(term)) {
                        return true;
                    }
                    if let Some(re) = patterns.get(term)
                        && re.is_match(&tw.desc_lower)
                    {
                        return true;
                    }
                    if !tw.hint_lower.is_empty()
                        && let Some(re) = patterns.get(term)
                        && re.is_match(&tw.hint_lower)
                    {
                        return true;
                    }
                    false
                })
            })
            .collect()
    };

    let mut scored: Vec<ScoredTool> = candidates
        .into_iter()
        .map(|tw| ScoredTool {
            name: tw.tool.name().to_string(),
            score: score_tool(
                tw.tool.as_ref(),
                &tw.parsed,
                &tw.desc_lower,
                &tw.hint_lower,
                &scoring_terms,
                &patterns,
            ),
        })
        .filter(|s| s.score > 0)
        .collect();
    scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    scored
        .into_iter()
        .take(max_results)
        .map(|s| s.name)
        .collect()
}

fn sort_tools_by_name(tools: &mut [Arc<dyn DynTool>]) {
    tools.sort_by(|a, b| a.name().cmp(b.name()));
}

fn canonical_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonical_json).collect()),
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = serde_json::Map::new();
            for (key, value) in entries {
                out.insert(key, canonical_json(value));
            }
            Value::Object(out)
        }
        other => other,
    }
}

fn stable_json_string(value: Value) -> String {
    serde_json::to_string(&canonical_json(value)).unwrap_or_default()
}

fn matched_tools_for_schema(
    matches: &[String],
    deferred: &[Arc<dyn DynTool>],
    enabled_tools: &[Arc<dyn DynTool>],
    all_tools: &[Arc<dyn DynTool>],
) -> Vec<Arc<dyn DynTool>> {
    let mut tools: Vec<Arc<dyn DynTool>> = matches
        .iter()
        .filter_map(|name| {
            deferred
                .iter()
                .chain(enabled_tools.iter())
                .chain(all_tools.iter())
                .find(|tool| tool.name() == name)
                .cloned()
        })
        .collect();
    sort_tools_by_name(&mut tools);
    tools.dedup_by(|a, b| a.name() == b.name());
    tools
}

async fn render_functions_for_client_side(
    matches: &[String],
    deferred: &[Arc<dyn DynTool>],
    enabled_tools: &[Arc<dyn DynTool>],
    all_tools: &[Arc<dyn DynTool>],
    ctx: &ToolUseContext,
) -> Option<String> {
    let tools = matched_tools_for_schema(matches, deferred, enabled_tools, all_tools);
    if tools.is_empty() {
        return None;
    }

    let mut tool_names: Vec<String> = all_tools
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    tool_names.sort();
    let prompt_options = PromptOptions {
        is_non_interactive: ctx.is_non_interactive,
        tool_names,
        permission_context: Some(ctx.permission_context.clone()),
        ..PromptOptions::default()
    };
    let schema_ctx = SchemaContext {
        features: Some(ctx.features.clone()),
        ..SchemaContext::default()
    };

    let mut lines = Vec::with_capacity(tools.len() + 2);
    lines.push("<functions>".to_string());
    for tool in tools {
        let ToolSpec::Function(spec) = tool.tool_spec(&schema_ctx, &prompt_options).await else {
            continue;
        };
        let schema = stable_json_string(serde_json::json!({
            "name": spec.name,
            "description": spec.description,
            "parameters": spec.parameters,
        }));
        lines.push(format!("<function>{schema}</function>"));
    }
    lines.push("</functions>".to_string());
    Some(lines.join("\n"))
}

/// Build the `AppStatePatch` that inserts the matched tool names into
/// [`coco_types::ToolAppState::discovered_tool_names`]. Returns `None`
/// when the match list is empty — no-op patches are wasteful and the
/// executor's compose-then-apply path is happier without them.
fn build_discovery_patch(matches: &[String]) -> Option<coco_types::AppStatePatch> {
    if matches.is_empty() {
        return None;
    }
    let names: Vec<String> = matches.to_vec();
    Some(Box::new(move |state: &mut coco_types::ToolAppState| {
        for name in names {
            state.discovered_tool_names.insert(name);
        }
    }))
}

/// Serde default for `max_results` — default is 5.
fn default_tool_search_max_results() -> Option<i64> {
    Some(DEFAULT_MAX_RESULTS as i64)
}

/// Typed input for [`ToolSearchTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ToolSearchInput {
    /// Query to find deferred tools. Use "select:<tool_name>" for
    /// direct selection, or keywords to search.
    pub query: String,
    /// Maximum number of results to return (default: 5)
    #[serde(default = "default_tool_search_max_results")]
    pub max_results: Option<i64>,
}

/// Typed output for [`ToolSearchTool`]. Same wire fields as the
/// pre-typed `build_envelope` produced.
///
/// All fields default so transcript replay / partial fixtures
/// round-trip via the `DynTool` blanket.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolSearchOutput {
    #[serde(default)]
    pub matches: Vec<String>,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub total_deferred_tools: i64,
    /// Set when the current model supports Anthropic's server-side
    /// `tool_reference` expansion — `render_for_model` then emits
    /// `tool_reference` content blocks instead of a text list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_as_tool_reference: Option<bool>,
    /// Empty-result retry hint — only set when no matches AND at
    /// least one MCP server is still mid-handshake.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_mcp_servers: Option<Vec<String>>,
    /// Client-side fallback schema block rendered for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered_functions: Option<String>,
}

pub struct ToolSearchTool;

#[async_trait::async_trait]
impl Tool for ToolSearchTool {
    type Input = ToolSearchInput;
    coco_tool_runtime::impl_runtime_schema!(ToolSearchInput);
    type Output = ToolSearchOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::ToolSearch)
    }
    fn name(&self) -> &str {
        ToolName::ToolSearch.as_str()
    }
    /// Hidden from the model when `ToolSearch` is inactive: the feature is off,
    /// the current model declared neither
    /// [`coco_types::Capability::ServerSideToolReference`] nor
    /// [`coco_types::Capability::ClientSideToolSearch`], or there are no
    /// deferred tools / pending MCP servers to search.
    ///
    /// Symmetric with [`coco_tool_runtime::ToolRegistry::loaded_tools`]
    /// which short-circuits the `should_defer()` filter on the same
    /// `ToolUseContext::tool_search_active()` predicate, so an
    /// inactive model surfaces every enabled tool's schema upfront
    /// and the `ToolSearch` round-trip never fires.
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.tool_search_active()
    }
    fn description(&self, _input: &ToolSearchInput, _options: &DescriptionOptions) -> String {
        format!("{PROMPT_HEAD}{PROMPT_LOCATION_HINT}{PROMPT_TAIL}")
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        format!("{PROMPT_HEAD}{PROMPT_LOCATION_HINT}{PROMPT_TAIL}")
    }
    fn is_read_only(&self, _input: &ToolSearchInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    fn is_concurrency_safe(&self, _input: &ToolSearchInput) -> bool {
        true
    }

    /// Render the search envelope into content parts the model sees.
    ///
    /// **Two emission shapes**, selected by the `render_as_tool_reference`
    /// flag the executor sets in `out`:
    ///
    /// 1. **`tool_reference` blocks** (Anthropic, capable models) —
    ///    one `Custom` part per match carrying
    ///    `{type:"tool-reference", toolName:X}` under
    ///    `provider_options.anthropic`. The Anthropic API server
    ///    expands the block into inline `<functions>` markup before
    ///    the prompt reaches the model. Client-side `tools` array is
    ///    NOT modified — cache prefix stays warm across discoveries.
    ///
    /// 2. **Text list** (every other provider + non-capable Anthropic
    ///    models) — single `Text` part rendering matched names and
    ///    explaining schemas arrive next turn. The executor pairs this
    ///    branch with an `AppStatePatch` that adds matches to
    ///    `discovered_tool_names`, so the next turn's `tools` array
    ///    surfaces the schemas client-side. One cache break per
    ///    discovery, unavoidable without server-side expansion.
    ///
    /// The empty-match branch is identical across paths (a model that
    /// matched zero tools has no schemas to surface either way), and
    /// Empty-match text: `No matching deferred tools found` +
    /// the pending-MCP-server suffix when servers are still
    /// mid-handshake.
    fn render_for_model(&self, out: &ToolSearchOutput) -> Vec<ToolResultContentPart> {
        let use_tool_reference = out.render_as_tool_reference.unwrap_or(false);

        if !out.matches.is_empty() && use_tool_reference {
            return out
                .matches
                .iter()
                .map(|m| coco_tool_runtime::tool_reference_content_part(m.as_str()))
                .collect();
        }

        let text = if out.matches.is_empty() {
            let mut text = "No matching deferred tools found".to_string();
            if let Some(pending) = out.pending_mcp_servers.as_ref() {
                let names: Vec<&str> = pending.iter().map(String::as_str).collect();
                if !names.is_empty() {
                    use std::fmt::Write;
                    let _ = write!(
                        text,
                        ". Some MCP servers are still connecting: {}. Their tools will become available shortly — try searching again.",
                        names.join(", ")
                    );
                }
            }
            text
        } else if let Some(rendered) = out.rendered_functions.as_ref() {
            rendered.clone()
        } else {
            format!(
                "Matched tools (schemas will be available next turn):\n{}",
                out.matches.join("\n")
            )
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: ToolSearchInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<ToolSearchOutput>, ToolError> {
        let raw_query = input.query.trim().to_string();

        if raw_query.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "query parameter is required".into(),
                error_code: None,
            });
        }

        let max_results = input
            .max_results
            .filter(|n| *n > 0)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS);

        // Snapshot the registry once so the searchable pools see a
        // consistent state. `ctx.tools.*` clone Arc handles — cheap.
        let mut all_tools = ctx.tools.all();
        sort_tools_by_name(&mut all_tools);
        // Pipeline-filtered candidate pools. ToolSearch must never match a
        // tool the registry would refuse to surface: a match that fails
        // `passes_filter_pipeline` is inert (it can't enter `loaded_tools`)
        // and would make the model re-search forever. `searchable_deferred`
        // is the deferred pool that passes the pipeline (discovered names
        // kept so re-select is an idempotent no-op); `enabled` is the
        // exact-name fallback corpus of pipeline-passing tools that aren't
        // deferred (already loaded → harmless no-op match).
        let mut deferred: Vec<Arc<dyn DynTool>> = ctx.tools.searchable_deferred(ctx);
        sort_tools_by_name(&mut deferred);
        let mut enabled_tools = ctx.tools.enabled(ctx);
        sort_tools_by_name(&mut enabled_tools);
        let total_deferred_tools = deferred.len() as i64;
        let deferred_tool_names: Vec<&str> = deferred.iter().map(|t| t.name()).collect();
        let enabled_tool_names: Vec<&str> = enabled_tools.iter().map(|t| t.name()).collect();
        tracing::debug!(
            query = %raw_query,
            max_results,
            total_deferred_tools,
            deferred_tools = ?deferred_tool_names,
            enabled_tools = ?enabled_tool_names,
            "ToolSearch candidate pools resolved"
        );

        // Build a DescriptionOptions for the description-aware path.
        // Includes the full tool-name list so tools whose description
        // varies by sibling tools (Agent / Skill) render their final
        // text rather than a placeholder.
        let mut tool_names: Vec<String> = all_tools.iter().map(|t| t.name().to_string()).collect();
        tool_names.sort();
        let desc_opts = DescriptionOptions {
            is_non_interactive: false,
            tool_names,
            permission_context: Some(ctx.permission_context.clone()),
        };

        // Whether the current model supports Anthropic's server-side
        // `tool_reference` expansion. When `true`, the envelope is
        // tagged so `render_for_model` emits `tool_reference` content
        // blocks (cache-friendly), and the `discovered_tool_names`
        // patch is skipped — the discovery state lives in message
        // history (the `tool_reference` blocks themselves).
        let use_tool_reference = ctx.model_supports_tool_reference;

        // Direct selection mode — `select:Tool1,Tool2,...`. Missing names
        // are silently dropped. Names that resolve in the full pool but not
        // the deferred set are returned anyway so the model proceeds without
        // retry churn.
        if let Some(names) = parse_select_query(&raw_query) {
            if names.is_empty() {
                return Err(ToolError::InvalidInput {
                    message: "select: query must name at least one tool (e.g. 'select:Read,Grep')"
                        .into(),
                    error_code: None,
                });
            }
            let mut matches: Vec<String> = Vec::new();
            let mut seen = HashSet::new();
            for name in &names {
                let lowered = name.to_lowercase();
                let hit = deferred
                    .iter()
                    .find(|t| {
                        t.name().eq_ignore_ascii_case(name)
                            || t.aliases().iter().any(|a| a.eq_ignore_ascii_case(name))
                    })
                    .or_else(|| {
                        enabled_tools.iter().find(|t| {
                            t.name().eq_ignore_ascii_case(name)
                                || t.aliases().iter().any(|a| a.eq_ignore_ascii_case(&lowered))
                        })
                    });
                if let Some(tool) = hit {
                    let canonical = tool.name().to_string();
                    if seen.insert(canonical.clone()) {
                        matches.push(canonical);
                    }
                }
            }
            tracing::debug!(
                query = %raw_query,
                mode = "select",
                matches = ?matches,
                "ToolSearch resolved matches"
            );
            let rendered_functions = if use_tool_reference {
                None
            } else {
                render_functions_for_client_side(
                    &matches,
                    &deferred,
                    &enabled_tools,
                    &all_tools,
                    ctx,
                )
                .await
            };
            let envelope = build_envelope(
                &matches,
                &raw_query,
                total_deferred_tools,
                use_tool_reference,
                rendered_functions,
                &ctx.mcp,
            )
            .await;
            return Ok(ToolResult {
                data: envelope,
                new_messages: vec![],
                app_state_patch: if use_tool_reference {
                    None
                } else {
                    build_discovery_patch(&matches)
                },
                permission_updates: Vec::new(),
                display_data: None,
            });
        }

        // Keyword path.
        let matches = search_with_keywords(
            &deferred,
            &enabled_tools,
            &desc_opts,
            &raw_query,
            max_results,
        );
        tracing::debug!(
            query = %raw_query,
            mode = "keyword",
            matches = ?matches,
            "ToolSearch resolved matches"
        );

        let rendered_functions = if use_tool_reference {
            None
        } else {
            render_functions_for_client_side(&matches, &deferred, &enabled_tools, &all_tools, ctx)
                .await
        };
        let envelope = build_envelope(
            &matches,
            &raw_query,
            total_deferred_tools,
            use_tool_reference,
            rendered_functions,
            &ctx.mcp,
        )
        .await;
        Ok(ToolResult {
            data: envelope,
            new_messages: vec![],
            app_state_patch: if use_tool_reference {
                None
            } else {
                build_discovery_patch(&matches)
            },
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Construct the structured envelope returned in `ToolResult.data`.
/// `render_for_model` reads:
/// - `matches: [String]` — names to surface (text list OR
///   `tool_reference` blocks, gated by `render_as_tool_reference`).
/// - `pending_mcp_servers: [String]` — non-empty only when the match
///   list is empty AND an MCP server is mid-handshake (retry hint).
/// - `render_as_tool_reference: bool` — set by the executor based on
///   the current model's `Capability::ServerSideToolReference`.
async fn build_envelope(
    matches: &[String],
    raw_query: &str,
    total_deferred_tools: i64,
    use_tool_reference: bool,
    rendered_functions: Option<String>,
    mcp: &coco_tool_runtime::McpHandleRef,
) -> ToolSearchOutput {
    // Empty-result retry hint: only attach when there's genuine MCP-
    // server churn so the model gets actionable info, not noise.
    let pending_mcp_servers = if matches.is_empty() {
        let pending = mcp.pending_server_names().await;
        if pending.is_empty() {
            None
        } else {
            Some(pending)
        }
    } else {
        None
    };

    ToolSearchOutput {
        matches: matches.to_vec(),
        query: raw_query.to_string(),
        total_deferred_tools,
        render_as_tool_reference: use_tool_reference.then_some(true),
        pending_mcp_servers,
        rendered_functions,
    }
}

#[cfg(test)]
#[path = "tool_search.test.rs"]
mod tests;
