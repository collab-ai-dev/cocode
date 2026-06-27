//! Message pruning utilities.
//!
//! This module provides functions for pruning messages to manage
//! context window limits.

use vercel_ai_provider::LanguageModelV4Message;

/// How to handle reasoning content pruning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReasoningPruneMode {
    /// Keep all reasoning content.
    #[default]
    None,
    /// Remove all reasoning content.
    All,
    /// Remove reasoning content except in the last message.
    BeforeLastMessage,
}

/// How to handle tool calls/results pruning.
#[derive(Debug, Clone, Default)]
pub enum ToolCallsPruneMode {
    /// Keep all tool calls.
    #[default]
    None,
    /// Remove all tool calls.
    All,
    /// Remove tool calls except in the last N messages.
    BeforeLastMessage(usize),
    /// Custom pruning with specific tools.
    Custom {
        /// How to prune.
        mode: ToolCallsPruneModeInner,
        /// Which tools to prune (if specified).
        tools: Option<Vec<String>>,
    },
}

/// Inner mode for tool calls pruning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolCallsPruneModeInner {
    /// Keep all.
    #[default]
    All,
    /// Remove all.
    None,
    /// Remove before last message.
    BeforeLastMessage,
}

/// Options for pruning messages.
#[derive(Debug, Clone, Default)]
pub struct PruneMessagesOptions {
    /// How to prune reasoning content.
    pub reasoning: ReasoningPruneMode,
    /// How to prune tool calls.
    pub tool_calls: ToolCallsPruneMode,
    /// Whether to remove empty messages after pruning.
    pub remove_empty: bool,
}

/// Prune messages according to the specified options.
pub fn prune_messages(
    messages: Vec<LanguageModelV4Message>,
    options: &PruneMessagesOptions,
) -> Vec<LanguageModelV4Message> {
    let mut result = messages;

    // Prune reasoning content
    result = prune_reasoning(result, options.reasoning);

    // Prune tool calls
    result = prune_tool_calls(result, &options.tool_calls);

    // Remove empty messages if requested
    if options.remove_empty {
        result.retain(|msg| !is_empty_message(msg));
    }

    result
}

fn prune_reasoning(
    messages: Vec<LanguageModelV4Message>,
    mode: ReasoningPruneMode,
) -> Vec<LanguageModelV4Message> {
    if mode == ReasoningPruneMode::None {
        return messages;
    }

    let len = messages.len();
    messages
        .into_iter()
        .enumerate()
        .map(|(idx, msg)| {
            if let LanguageModelV4Message::Assistant {
                content,
                provider_options,
            } = &msg
            {
                // Check if this message should be pruned
                let should_prune = match mode {
                    ReasoningPruneMode::All => true,
                    ReasoningPruneMode::BeforeLastMessage => idx < len - 1,
                    ReasoningPruneMode::None => false,
                };

                if should_prune {
                    // Filter out reasoning parts
                    let filtered_content: Vec<_> = content
                        .iter()
                        .filter(|part| {
                            !matches!(part, vercel_ai_provider::AssistantContentPart::Reasoning(_))
                        })
                        .cloned()
                        .collect();

                    return LanguageModelV4Message::Assistant {
                        content: filtered_content,
                        provider_options: provider_options.clone(),
                    };
                }
            }
            msg
        })
        .collect()
}

fn prune_tool_calls(
    messages: Vec<LanguageModelV4Message>,
    mode: &ToolCallsPruneMode,
) -> Vec<LanguageModelV4Message> {
    match mode {
        ToolCallsPruneMode::None => messages,
        ToolCallsPruneMode::All => {
            // Remove all tool calls and tool messages
            messages
                .into_iter()
                .filter(|msg| !matches!(msg, LanguageModelV4Message::Tool { .. }))
                .map(|msg| {
                    if let LanguageModelV4Message::Assistant {
                        content,
                        provider_options,
                    } = &msg
                    {
                        let filtered_content: Vec<_> = content
                            .iter()
                            .filter(|part| {
                                !matches!(
                                    part,
                                    vercel_ai_provider::AssistantContentPart::ToolCall(_)
                                        | vercel_ai_provider::AssistantContentPart::ToolResult(_)
                                )
                            })
                            .cloned()
                            .collect();
                        LanguageModelV4Message::Assistant {
                            content: filtered_content,
                            provider_options: provider_options.clone(),
                        }
                    } else {
                        msg
                    }
                })
                .collect()
        }
        ToolCallsPruneMode::BeforeLastMessage(n) => {
            // Keep tool calls in the last N messages
            let len = messages.len();
            let keep_from = len.saturating_sub(*n);

            messages
                .into_iter()
                .enumerate()
                .map(|(idx, msg)| {
                    if idx >= keep_from {
                        return msg;
                    }

                    // Remove tool calls from earlier messages
                    if let LanguageModelV4Message::Assistant {
                        content,
                        provider_options,
                    } = &msg
                    {
                        let filtered_content: Vec<_> = content
                            .iter()
                            .filter(|part| {
                                !matches!(
                                    part,
                                    vercel_ai_provider::AssistantContentPart::ToolCall(_)
                                        | vercel_ai_provider::AssistantContentPart::ToolResult(_)
                                )
                            })
                            .cloned()
                            .collect();
                        return LanguageModelV4Message::Assistant {
                            content: filtered_content,
                            provider_options: provider_options.clone(),
                        };
                    }

                    // Remove tool messages from earlier messages
                    if matches!(msg, LanguageModelV4Message::Tool { .. }) {
                        return LanguageModelV4Message::Tool {
                            content: vec![],
                            provider_options: None,
                        }; // Will be filtered if remove_empty
                    }

                    msg
                })
                .collect()
        }
        ToolCallsPruneMode::Custom { mode, tools } => {
            prune_tool_calls_custom(messages, *mode, tools.as_deref())
        }
    }
}

/// Check whether a tool name should be pruned given an optional filter list.
///
/// If `tool_filter` is `None`, all tools are pruned. If `Some`, only tools
/// whose name appears in the list are pruned.
fn should_prune_tool(tool_name: &str, tool_filter: Option<&[String]>) -> bool {
    match tool_filter {
        None => true,
        Some(names) => names.iter().any(|n| n == tool_name),
    }
}

/// Filter assistant content parts, removing tool calls/results that match
/// the tool filter.
fn filter_assistant_content(
    content: &[vercel_ai_provider::AssistantContentPart],
    tool_filter: Option<&[String]>,
) -> Vec<vercel_ai_provider::AssistantContentPart> {
    content
        .iter()
        .filter(|part| match part {
            vercel_ai_provider::AssistantContentPart::ToolCall(tc) => {
                !should_prune_tool(&tc.tool_name, tool_filter)
            }
            vercel_ai_provider::AssistantContentPart::ToolResult(tr) => {
                !should_prune_tool(&tr.tool_name, tool_filter)
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// Filter tool content parts, removing tool results that match the tool filter.
fn filter_tool_content(
    content: &[vercel_ai_provider::ToolContentPart],
    tool_filter: Option<&[String]>,
) -> Vec<vercel_ai_provider::ToolContentPart> {
    content
        .iter()
        .filter(|part| match part {
            vercel_ai_provider::ToolContentPart::ToolResult(tr) => {
                !should_prune_tool(&tr.tool_name, tool_filter)
            }
            _ => true,
        })
        .cloned()
        .collect()
}

fn prune_tool_calls_custom(
    messages: Vec<LanguageModelV4Message>,
    mode: ToolCallsPruneModeInner,
    tool_filter: Option<&[String]>,
) -> Vec<LanguageModelV4Message> {
    match mode {
        ToolCallsPruneModeInner::None => {
            // Keep all tool calls -- return messages unchanged.
            messages
        }
        ToolCallsPruneModeInner::All => {
            // Remove matching tool calls/results from every message.
            messages
                .into_iter()
                .map(|msg| match msg {
                    LanguageModelV4Message::Assistant {
                        ref content,
                        ref provider_options,
                    } => LanguageModelV4Message::Assistant {
                        content: filter_assistant_content(content, tool_filter),
                        provider_options: provider_options.clone(),
                    },
                    LanguageModelV4Message::Tool {
                        ref content,
                        ref provider_options,
                    } => LanguageModelV4Message::Tool {
                        content: filter_tool_content(content, tool_filter),
                        provider_options: provider_options.clone(),
                    },
                    other => other,
                })
                .collect()
        }
        ToolCallsPruneModeInner::BeforeLastMessage => {
            // Only prune messages before the last one.
            let len = messages.len();
            messages
                .into_iter()
                .enumerate()
                .map(|(idx, msg)| {
                    // Keep the last message untouched.
                    if idx >= len - 1 {
                        return msg;
                    }

                    match msg {
                        LanguageModelV4Message::Assistant {
                            ref content,
                            ref provider_options,
                        } => LanguageModelV4Message::Assistant {
                            content: filter_assistant_content(content, tool_filter),
                            provider_options: provider_options.clone(),
                        },
                        LanguageModelV4Message::Tool {
                            ref content,
                            ref provider_options,
                        } => LanguageModelV4Message::Tool {
                            content: filter_tool_content(content, tool_filter),
                            provider_options: provider_options.clone(),
                        },
                        other => other,
                    }
                })
                .collect()
        }
    }
}

fn is_empty_message(msg: &LanguageModelV4Message) -> bool {
    match msg {
        LanguageModelV4Message::System { content, .. } => content.is_empty(),
        LanguageModelV4Message::Developer { content, .. } => content.is_empty(),
        LanguageModelV4Message::User { content, .. } => content.is_empty(),
        LanguageModelV4Message::Assistant { content, .. } => content.is_empty(),
        LanguageModelV4Message::Tool { content, .. } => content.is_empty(),
    }
}

#[cfg(test)]
#[path = "prune_messages.test.rs"]
mod tests;
