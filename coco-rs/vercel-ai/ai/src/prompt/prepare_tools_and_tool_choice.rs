//! Prepare tools and tool choice for language model calls.
//!
//! This module provides utilities for preparing tool definitions and
//! tool choice settings for language model requests.

use std::sync::Arc;

use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::LanguageModelV4ToolChoice;
use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

use crate::types::ToolRegistry;

/// Prepare tools and tool choice for a call.
///
/// This function adds tool definitions and tool choice to call options.
///
/// # Arguments
///
/// * `call_options` - The base call options to modify.
/// * `tools` - Optional tool registry to get tool definitions from.
/// * `tool_choice` - Optional tool choice setting.
///
/// # Returns
///
/// Modified call options with tools configured.
pub fn prepare_tools_and_tool_choice(
    mut call_options: LanguageModelV4CallOptions,
    tools: Option<&Arc<ToolRegistry>>,
    tool_choice: Option<&LanguageModelV4ToolChoice>,
) -> LanguageModelV4CallOptions {
    // Add tool definitions
    if let Some(registry) = tools {
        let definitions: Vec<LanguageModelV4Tool> = registry
            .definitions()
            .into_iter()
            .map(|d| LanguageModelV4Tool::function(d.clone()))
            .collect();
        if !definitions.is_empty() {
            call_options.tools = Some(definitions);
        }
    }

    // Add tool choice
    if let Some(choice) = tool_choice {
        call_options.tool_choice = Some(choice.clone());
    }

    call_options
}

/// Prepare tool definitions from a tool registry.
///
/// # Arguments
///
/// * `tools` - The tool registry to extract definitions from.
///
/// # Returns
///
/// A vector of tool definitions, or an empty vector if no tools.
pub fn prepare_tool_definitions(tools: &ToolRegistry) -> Vec<LanguageModelV4FunctionTool> {
    tools.definitions().into_iter().cloned().collect()
}

/// Determine the effective tool choice.
///
/// This function determines the tool choice based on available tools
/// and user preference.
///
/// # Arguments
///
/// * `tools` - Optional tool registry.
/// * `tool_choice` - User-specified tool choice.
/// * `auto_tool_choice` - Whether to automatically select tools.
///
/// # Returns
///
/// The effective tool choice to use.
pub fn determine_tool_choice(
    tools: Option<&Arc<ToolRegistry>>,
    tool_choice: Option<&LanguageModelV4ToolChoice>,
    auto_tool_choice: bool,
) -> Option<LanguageModelV4ToolChoice> {
    // User-specified choice takes precedence
    if let Some(choice) = tool_choice {
        return Some(choice.clone());
    }

    // Auto tool choice if tools are available
    if auto_tool_choice
        && let Some(registry) = tools
        && !registry.definitions().is_empty()
    {
        return Some(LanguageModelV4ToolChoice::auto());
    }

    None
}

/// Check if tool calling is required.
///
/// # Arguments
///
/// * `tool_choice` - The tool choice setting.
///
/// # Returns
///
/// `true` if tool calling is required, `false` otherwise.
pub fn is_tool_call_required(tool_choice: Option<&LanguageModelV4ToolChoice>) -> bool {
    matches!(
        tool_choice,
        Some(LanguageModelV4ToolChoice::Required) | Some(LanguageModelV4ToolChoice::Tool { .. })
    )
}

/// Check if tool calling is disabled.
///
/// # Arguments
///
/// * `tools` - Whether tools are available.
/// * `tool_choice` - The tool choice setting.
///
/// # Returns
///
/// `true` if tool calling is disabled, `false` otherwise.
pub fn is_tool_call_disabled(
    has_tools: bool,
    tool_choice: Option<&LanguageModelV4ToolChoice>,
) -> bool {
    !has_tools || matches!(tool_choice, Some(LanguageModelV4ToolChoice::None))
}

#[cfg(test)]
#[path = "prepare_tools_and_tool_choice.test.rs"]
mod tests;
