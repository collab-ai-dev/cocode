use std::collections::HashSet;

use serde_json::Value;
use serde_json::json;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::Warning;

use crate::cache_control::CacheContext;
use crate::cache_control::CacheControlValidator;

/// Result of preparing tools for the Anthropic Messages API.
pub struct PreparedAnthropicTools {
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
    pub warnings: Vec<Warning>,
    pub betas: HashSet<String>,
}

/// Convert SDK tools + tool_choice to Anthropic Messages API format.
///
/// `supports_strict_tools` controls whether the `strict` field is included on
/// tool definitions and triggers the `structured-outputs` beta header.
///
/// `context_management_eligible` mirrors the predicate in
/// `beta_resolver::should_emit_context_management` — passed in (rather
/// than recomputed) so the memory-tool entry below cannot disagree
/// with the central decision (R3-F2). When false, the memory tool is
/// dropped from the wire payload.
pub fn prepare_anthropic_tools(
    tools: &Option<Vec<LanguageModelV4Tool>>,
    tool_choice: &Option<LanguageModelV4ToolChoice>,
    disable_parallel_tool_use: Option<bool>,
    supports_strict_tools: bool,
    context_management_eligible: bool,
    mut cache_validator: Option<&mut CacheControlValidator>,
) -> PreparedAnthropicTools {
    let mut warnings = Vec::new();
    let mut betas = HashSet::new();

    // Empty tools → no tools
    let tools_ref = tools.as_ref().filter(|ts| !ts.is_empty());

    let Some(tools_ref) = tools_ref else {
        return PreparedAnthropicTools {
            tools: None,
            tool_choice: None,
            warnings,
            betas,
        };
    };

    let mut anthropic_tools = Vec::new();

    for tool in tools_ref {
        match tool {
            LanguageModelV4Tool::Function(ft) => {
                let mut tool_def = json!({
                    "name": ft.name,
                    "description": ft.description,
                    "input_schema": ft.input_schema,
                });

                // Strict mode (only when strict tools are supported)
                if supports_strict_tools {
                    if let Some(strict) = ft.strict {
                        tool_def["strict"] = Value::Bool(strict);
                    }
                    betas.insert("structured-outputs-2025-11-13".into());
                }

                // Input examples
                if let Some(ref examples) = ft.input_examples {
                    let example_inputs: Vec<Value> = examples
                        .iter()
                        .map(|e| {
                            let map: serde_json::Map<String, Value> = e
                                .input
                                .iter()
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect();
                            Value::Object(map)
                        })
                        .collect();
                    tool_def["input_examples"] = Value::Array(example_inputs);
                    betas.insert("advanced-tool-use-2025-11-20".into());
                }

                // Strict mode warning when not supported
                if !supports_strict_tools && ft.strict.is_some() {
                    warnings.push(Warning::Unsupported {
                            feature: "strict".into(),
                            details: Some(format!(
                                "Tool '{}' has strict: {:?}, but strict mode is not supported by this provider. The strict property will be ignored.",
                                ft.name, ft.strict,
                            )),
                        });
                }

                // Provider-specific options (eagerInputStreaming, deferLoading, allowedCallers)
                if let Some(ref po) = ft.provider_options
                    && let Some(anthropic_opts) = po.0.get("anthropic")
                {
                    if anthropic_opts
                        .get("eagerInputStreaming")
                        .and_then(Value::as_bool)
                        == Some(true)
                    {
                        tool_def["eager_input_streaming"] = json!(true);
                    }
                    if anthropic_opts.get("deferLoading").and_then(Value::as_bool) == Some(true) {
                        tool_def["defer_loading"] = json!(true);
                    }
                    if let Some(callers) = anthropic_opts.get("allowedCallers") {
                        tool_def["allowed_callers"] = callers.clone();
                        betas.insert("advanced-tool-use-2025-11-20".into());
                    }
                }

                // Cache control on function tools
                if let Some(ref mut validator) = cache_validator
                    && let Some(cc) = validator.get_cache_control_from_options(
                        &ft.provider_options,
                        CacheContext {
                            type_name: "function tool",
                            can_cache: true,
                        },
                    )
                {
                    tool_def["cache_control"] = cc;
                }

                // Built-in/MCP boundary breakpoint. The engine marks the last
                // built-in tool with `cacheBoundary: true` (it owns the
                // `is_mcp` partition); the provider owns the cache POLICY, so
                // the marker's TTL is resolved here to match the last-user
                // auto-marker and is gated on caching being active. Skipped
                // when an explicit `cacheControl` already set this tool's
                // breakpoint, so the slot is never double-counted.
                if tool_def.get("cache_control").is_none()
                    && let Some(ref mut validator) = cache_validator
                    && let Some(cc) = validator.tool_boundary_cache_control(&ft.provider_options)
                {
                    tool_def["cache_control"] = cc;
                }

                anthropic_tools.push(tool_def);
            }
            LanguageModelV4Tool::Provider(pt) => {
                match pt.id.as_str() {
                    // Code execution tools
                    "anthropic.code_execution_20250522" => {
                        betas.insert("code-execution-2025-05-22".into());
                        anthropic_tools.push(json!({
                            "type": "code_execution_20250522",
                            "name": "code_execution",
                        }));
                    }
                    "anthropic.code_execution_20250825" => {
                        betas.insert("code-execution-2025-08-25".into());
                        anthropic_tools.push(json!({
                            "type": "code_execution_20250825",
                            "name": "code_execution",
                        }));
                    }
                    "anthropic.code_execution_20260120" => {
                        anthropic_tools.push(json!({
                            "type": "code_execution_20260120",
                            "name": "code_execution",
                        }));
                    }

                    // Computer use tools
                    "anthropic.computer_20241022" => {
                        betas.insert("computer-use-2024-10-22".into());
                        anthropic_tools.push(json!({
                            "name": "computer",
                            "type": "computer_20241022",
                            "display_width_px": pt.args.get("displayWidthPx"),
                            "display_height_px": pt.args.get("displayHeightPx"),
                            "display_number": pt.args.get("displayNumber"),
                        }));
                    }
                    "anthropic.computer_20250124" => {
                        betas.insert("computer-use-2025-01-24".into());
                        anthropic_tools.push(json!({
                            "name": "computer",
                            "type": "computer_20250124",
                            "display_width_px": pt.args.get("displayWidthPx"),
                            "display_height_px": pt.args.get("displayHeightPx"),
                            "display_number": pt.args.get("displayNumber"),
                        }));
                    }
                    "anthropic.computer_20251124" => {
                        betas.insert("computer-use-2025-11-24".into());
                        anthropic_tools.push(json!({
                            "name": "computer",
                            "type": "computer_20251124",
                            "display_width_px": pt.args.get("displayWidthPx"),
                            "display_height_px": pt.args.get("displayHeightPx"),
                            "display_number": pt.args.get("displayNumber"),
                            "enable_zoom": pt.args.get("enableZoom"),
                        }));
                    }

                    // Text editor tools
                    "anthropic.text_editor_20241022" => {
                        betas.insert("computer-use-2024-10-22".into());
                        anthropic_tools.push(json!({
                            "name": "str_replace_editor",
                            "type": "text_editor_20241022",
                        }));
                    }
                    "anthropic.text_editor_20250124" => {
                        betas.insert("computer-use-2025-01-24".into());
                        anthropic_tools.push(json!({
                            "name": "str_replace_editor",
                            "type": "text_editor_20250124",
                        }));
                    }
                    "anthropic.text_editor_20250429" => {
                        betas.insert("computer-use-2025-01-24".into());
                        anthropic_tools.push(json!({
                            "name": "str_replace_based_edit_tool",
                            "type": "text_editor_20250429",
                        }));
                    }
                    "anthropic.text_editor_20250728" => {
                        let mut tool_val = json!({
                            "name": "str_replace_based_edit_tool",
                            "type": "text_editor_20250728",
                        });
                        if let Some(max_chars) = pt.args.get("maxCharacters") {
                            tool_val["max_characters"] = max_chars.clone();
                        }
                        anthropic_tools.push(tool_val);
                    }

                    // Bash tools
                    "anthropic.bash_20241022" => {
                        betas.insert("computer-use-2024-10-22".into());
                        anthropic_tools.push(json!({
                            "name": "bash",
                            "type": "bash_20241022",
                        }));
                    }
                    "anthropic.bash_20250124" => {
                        betas.insert("computer-use-2025-01-24".into());
                        anthropic_tools.push(json!({
                            "name": "bash",
                            "type": "bash_20250124",
                        }));
                    }

                    // Memory tool. Gated on the SAME predicate as
                    // `body["context_management"]` and the
                    // `context-management-2025-06-27` beta — when the
                    // model lacks the capability OR the endpoint is
                    // not first-party OR experimental betas are
                    // disabled, the memory tool is dropped from the
                    // wire payload (would be rejected server-side
                    // without the gate beta). Single source of truth
                    // — Finding R3-F2.
                    "anthropic.memory_20250818" => {
                        if context_management_eligible {
                            betas.insert("context-management-2025-06-27".into());
                            anthropic_tools.push(json!({
                                "name": "memory",
                                "type": "memory_20250818",
                            }));
                        } else {
                            warnings.push(Warning::Other {
                                message: "memory tool requires context-management beta; \
                                          gated by capability + topology + experimental flag — \
                                          tool dropped from request"
                                    .into(),
                            });
                        }
                    }

                    // Web search tools
                    "anthropic.web_search_20250305" => {
                        let mut tool_val = json!({
                            "type": "web_search_20250305",
                            "name": "web_search",
                        });
                        if let Some(v) = pt.args.get("maxUses") {
                            tool_val["max_uses"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("allowedDomains") {
                            tool_val["allowed_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("blockedDomains") {
                            tool_val["blocked_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("userLocation") {
                            tool_val["user_location"] = v.clone();
                        }
                        anthropic_tools.push(tool_val);
                    }
                    "anthropic.web_search_20260209" => {
                        betas.insert("code-execution-web-tools-2026-02-09".into());
                        let mut tool_val = json!({
                            "type": "web_search_20260209",
                            "name": "web_search",
                        });
                        if let Some(v) = pt.args.get("maxUses") {
                            tool_val["max_uses"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("allowedDomains") {
                            tool_val["allowed_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("blockedDomains") {
                            tool_val["blocked_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("userLocation") {
                            tool_val["user_location"] = v.clone();
                        }
                        anthropic_tools.push(tool_val);
                    }

                    // Web fetch tools
                    "anthropic.web_fetch_20250910" => {
                        betas.insert("web-fetch-2025-09-10".into());
                        let mut tool_val = json!({
                            "type": "web_fetch_20250910",
                            "name": "web_fetch",
                        });
                        if let Some(v) = pt.args.get("maxUses") {
                            tool_val["max_uses"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("allowedDomains") {
                            tool_val["allowed_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("blockedDomains") {
                            tool_val["blocked_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("citations") {
                            tool_val["citations"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("maxContentTokens") {
                            tool_val["max_content_tokens"] = v.clone();
                        }
                        anthropic_tools.push(tool_val);
                    }
                    "anthropic.web_fetch_20260209" => {
                        betas.insert("code-execution-web-tools-2026-02-09".into());
                        let mut tool_val = json!({
                            "type": "web_fetch_20260209",
                            "name": "web_fetch",
                        });
                        if let Some(v) = pt.args.get("maxUses") {
                            tool_val["max_uses"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("allowedDomains") {
                            tool_val["allowed_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("blockedDomains") {
                            tool_val["blocked_domains"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("citations") {
                            tool_val["citations"] = v.clone();
                        }
                        if let Some(v) = pt.args.get("maxContentTokens") {
                            tool_val["max_content_tokens"] = v.clone();
                        }
                        anthropic_tools.push(tool_val);
                    }

                    // Tool search tools
                    "anthropic.tool_search_regex_20251119" => {
                        anthropic_tools.push(json!({
                            "type": "tool_search_tool_regex_20251119",
                            "name": "tool_search_tool_regex",
                        }));
                    }
                    "anthropic.tool_search_bm25_20251119" => {
                        anthropic_tools.push(json!({
                            "type": "tool_search_tool_bm25_20251119",
                            "name": "tool_search_tool_bm25",
                        }));
                    }

                    _ => {
                        warnings.push(Warning::Unsupported {
                            feature: format!("provider-defined tool {}", pt.id),
                            details: None,
                        });
                    }
                }
            }
        }
    }

    if anthropic_tools.is_empty() {
        return PreparedAnthropicTools {
            tools: None,
            tool_choice: None,
            warnings,
            betas,
        };
    }

    // Map tool choice
    let disable = disable_parallel_tool_use.unwrap_or(false);

    let mapped_tool_choice = match tool_choice {
        None => {
            if disable {
                Some(json!({"type": "auto", "disable_parallel_tool_use": true}))
            } else {
                None
            }
        }
        Some(LanguageModelV4ToolChoice::Auto) => {
            let mut tc = json!({"type": "auto"});
            if disable {
                tc["disable_parallel_tool_use"] = Value::Bool(true);
            }
            Some(tc)
        }
        Some(LanguageModelV4ToolChoice::Required) => {
            let mut tc = json!({"type": "any"});
            if disable {
                tc["disable_parallel_tool_use"] = Value::Bool(true);
            }
            Some(tc)
        }
        Some(LanguageModelV4ToolChoice::None) => {
            // Anthropic does not support 'none' tool choice — remove tools
            return PreparedAnthropicTools {
                tools: None,
                tool_choice: None,
                warnings,
                betas,
            };
        }
        Some(LanguageModelV4ToolChoice::Tool { tool_name }) => {
            let mut tc = json!({"type": "tool", "name": tool_name});
            if disable {
                tc["disable_parallel_tool_use"] = Value::Bool(true);
            }
            Some(tc)
        }
    };

    PreparedAnthropicTools {
        tools: Some(anthropic_tools),
        tool_choice: mapped_tool_choice,
        warnings,
        betas,
    }
}

#[cfg(test)]
#[path = "prepare_tools.test.rs"]
mod tests;
