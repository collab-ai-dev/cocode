//! Runtime-backed context-window analysis for `/context` and SDK control.
//!
//! This module is the single rich analyzer. It builds the same prompt and
//! tool definitions as the next main-loop API call, then estimates the
//! breakdown locally without provider token-count calls.

use std::collections::HashMap;
use std::sync::Arc;

use coco_error::ErrorExt;
use coco_error::Location;
use coco_error::StatusCode;
use coco_error::stack_trace_debug;
use coco_inference::LanguageModelTool;
use coco_messages::LlmMessage;
use coco_messages::Message;
use coco_messages::MessageHistory;
use coco_types::AttachmentTypeBreakdown;
use coco_types::ContextCategoryKind;
use coco_types::GridCellKind;
use coco_types::MessageBreakdown;
use coco_types::ProviderModelSelection;
use coco_types::ToolAppState;
use coco_types::ToolTypeBreakdown;
use coco_types::fmt_token_compact;
use snafu::Snafu;

use crate::engine::QueryEngine;
use crate::engine_prompt::ModelToolSource;

#[stack_trace_debug]
#[derive(Snafu)]
#[snafu(visibility(pub), module)]
pub enum ContextAnalysisError {
    #[snafu(display("active main model has no resolved ModelInfo; context window is unavailable"))]
    MissingModelInfo {
        #[snafu(implicit)]
        location: Location,
    },
}

impl ErrorExt for ContextAnalysisError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::MissingModelInfo { .. } => StatusCode::InvalidArguments,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

pub type Result<T> = std::result::Result<T, ContextAnalysisError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextUsageCategory {
    pub kind: ContextCategoryKind,
    pub tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryFileEstimate {
    pub path: String,
    pub source: String,
    pub tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolEstimate {
    pub name: String,
    pub server_name: String,
    pub tokens: i64,
    pub deferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEstimate {
    pub agent_type: String,
    pub source: String,
    pub tokens: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillEstimate {
    pub name: String,
    pub source: String,
    pub tokens: i64,
}

#[derive(Debug, Clone)]
pub struct ContextUsageReport {
    pub total_tokens: i64,
    pub max_tokens: i64,
    pub raw_max_tokens: i64,
    pub percentage: f64,
    pub model: ProviderModelSelection,
    pub categories: Vec<ContextUsageCategory>,
    pub memory_files: Vec<MemoryFileEstimate>,
    pub mcp_tools: Vec<McpToolEstimate>,
    pub agents: Vec<AgentEstimate>,
    pub skills: Vec<SkillEstimate>,
    pub message_breakdown: MessageBreakdown,
    pub is_auto_compact_enabled: bool,
    pub auto_compact_threshold: Option<i64>,
}

impl ContextUsageReport {
    pub fn to_wire(&self) -> coco_types::ContextUsageResult {
        let mut result = coco_types::ContextUsageResult {
            total_tokens: self.total_tokens,
            max_tokens: self.max_tokens,
            raw_max_tokens: self.raw_max_tokens,
            percentage: self.percentage,
            model: format!("{}/{}", self.model.provider, self.model.model_id),
            categories: self
                .categories
                .iter()
                .filter(|c| c.tokens > 0 || c.kind == ContextCategoryKind::Free)
                .map(|c| coco_types::ContextUsageCategory {
                    kind: c.kind,
                    tokens: c.tokens,
                })
                .collect(),
            is_auto_compact_enabled: self.is_auto_compact_enabled,
            auto_compact_threshold: self.auto_compact_threshold,
            message_breakdown: Some(self.message_breakdown.clone()),
            memory_files: self
                .memory_files
                .iter()
                .map(|m| coco_types::ContextMemoryFile {
                    path: m.path.clone(),
                    source: m.source.clone(),
                    tokens: m.tokens,
                })
                .collect(),
            mcp_tools: self
                .mcp_tools
                .iter()
                .map(|m| coco_types::ContextMcpTool {
                    name: m.name.clone(),
                    server_name: m.server_name.clone(),
                    tokens: m.tokens,
                    is_loaded: !m.deferred,
                })
                .collect(),
            agents: self
                .agents
                .iter()
                .map(|a| coco_types::ContextAgent {
                    agent_type: a.agent_type.clone(),
                    source: a.source.clone(),
                    tokens: a.tokens,
                })
                .collect(),
            skills: self
                .skills
                .iter()
                .map(|s| coco_types::ContextSkill {
                    name: s.name.clone(),
                    source: s.source.clone(),
                    tokens: s.tokens,
                })
                .collect(),
            suggestions: Vec::new(),
        };
        result.suggestions = coco_types::build_suggestions(&result);
        result
    }
}

pub async fn analyze_engine_context(
    engine: &QueryEngine,
    history: &MessageHistory,
) -> Result<ContextUsageReport> {
    analyze_engine_context_with_sources(engine, history, None).await
}

pub async fn analyze_engine_context_with_sources(
    engine: &QueryEngine,
    history: &MessageHistory,
    skill_manager: Option<Arc<coco_skills::SkillManager>>,
) -> Result<ContextUsageReport> {
    let snapshot = engine
        .runtime_snapshot()
        .ok_or_else(|| context_analysis_error::MissingModelInfoSnafu.build())?;
    let model_info = snapshot
        .model_info
        .as_ref()
        .ok_or_else(|| context_analysis_error::MissingModelInfoSnafu.build())?;
    let raw_max_tokens = i64::from(model_info.context_window);
    let max_tokens = raw_max_tokens;
    let model = ProviderModelSelection {
        provider: snapshot.provider.clone(),
        model_id: snapshot.model_id.clone(),
    };

    let app_state = match &engine.app_state {
        Some(state) => state.read().await.clone(),
        None => ToolAppState::default(),
    };
    let built = engine.build_prompt(history).await;
    let tool_defs = engine.build_tool_definitions_detailed(&app_state).await;

    let system_text = first_system_text(&built.prompt);
    let system_total = coco_messages::estimate_text_tokens(system_text);
    // CLAUDE.md / AGENTS.md are embedded inline in the bootstrap-assembled
    // system prompt (`build_system_prompt` → "# Project Instructions"), so
    // discover them independently here and attribute their tokens to the
    // Memory files category instead of burying them in System prompt.
    let cwd = std::env::current_dir().unwrap_or_default();
    let memory_files: Vec<MemoryFileEstimate> = coco_context::discover_memory_files(&cwd)
        .into_iter()
        .map(|f| {
            let segment = format!("## {}\n{}\n\n", f.path.display(), f.content);
            MemoryFileEstimate {
                tokens: coco_messages::estimate_text_tokens(&segment),
                path: f.path.display().to_string(),
                source: memory_file_source_label(f.source).to_string(),
            }
        })
        .collect();
    let memory_tokens = memory_files.iter().map(|m| m.tokens).sum::<i64>();
    let system_tokens = system_total.saturating_sub(memory_tokens);

    let mut builtin_tool_tokens = 0;
    let mut mcp_tool_tokens = 0;
    let mut agent_tool_tokens = 0;
    let mut skill_tool_tokens = 0;
    let mut mcp_tools = Vec::new();

    for built_tool in &tool_defs {
        let estimate = estimate_tool_tokens(&built_tool.tool);
        let name = built_tool.tool.name().to_string();
        match &built_tool.source {
            ModelToolSource::Agent => {
                if !built_tool.deferred {
                    agent_tool_tokens += estimate;
                }
            }
            ModelToolSource::Skill => {
                if !built_tool.deferred {
                    skill_tool_tokens += estimate;
                }
            }
            ModelToolSource::Mcp { server_name } => {
                mcp_tools.push(McpToolEstimate {
                    name,
                    server_name: server_name.clone(),
                    tokens: estimate,
                    deferred: built_tool.deferred,
                });
                if !built_tool.deferred {
                    mcp_tool_tokens += estimate;
                }
            }
            ModelToolSource::BuiltIn if !built_tool.deferred => {
                builtin_tool_tokens += estimate;
            }
            ModelToolSource::BuiltIn => {}
        }
    }

    let agents = agent_estimates(engine).await;

    let skills = skill_estimates(engine, skill_manager.as_deref());

    // Category totals describe model-visible prompt/tool bytes. Detail
    // rows are explanatory source slices and may not sum to their
    // category: AgentTool already embeds agent listings in its prompt,
    // while skill rows come from the session catalog for parity with
    // SDK `/context`.
    let message_tokens =
        coco_messages::estimate_tokens_for_messages(built.messages_snapshot.as_ref().as_slice());
    let message_breakdown = message_breakdown(built.messages_snapshot.as_ref().as_slice());

    // Estimated model-visible content: every category except the reserved
    // buffer and free space (system + tools + mcp + agents + memory + skills
    // + messages).
    let actual_usage = system_tokens
        + builtin_tool_tokens
        + mcp_tool_tokens
        + agent_tool_tokens
        + memory_tokens
        + skill_tool_tokens
        + message_tokens;

    let is_auto_compact_enabled = engine.config.is_auto_compact_active();
    let auto_compact_threshold = is_auto_compact_enabled.then(|| {
        coco_compact::auto_compact_threshold(
            max_tokens,
            engine.config.max_output_tokens,
            &engine.config.compact.auto,
        )
    });
    let reserved = auto_compact_threshold
        .map(|t| (max_tokens - t).max(0))
        .unwrap_or(0);

    // Headline total prefers the real billed usage (folds the whole prompt —
    // system, tools, memory, messages) once an API call has committed a usage
    // marker; with no call yet it falls back to the estimated content sum so a
    // fresh session reflects its fixed overhead instead of 0.
    let total_tokens = if history.last_usage().is_some() {
        history.tokens_with_last_usage()
    } else {
        actual_usage
    };
    let percentage = if max_tokens > 0 {
        (total_tokens as f64 / max_tokens as f64) * 100.0
    } else {
        0.0
    };
    // Free space tiles the grid against the estimated content sum (not the
    // billed headline) so grid + legend stay internally consistent and sum to
    // the window.
    let free_tokens = (max_tokens - actual_usage - reserved).max(0);

    Ok(ContextUsageReport {
        total_tokens,
        max_tokens,
        raw_max_tokens,
        percentage,
        model,
        categories: vec![
            ContextUsageCategory {
                kind: ContextCategoryKind::SystemPrompt,
                tokens: system_tokens,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Tools,
                tokens: builtin_tool_tokens,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::McpTools,
                tokens: mcp_tool_tokens,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Agents,
                tokens: agent_tool_tokens,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::MemoryFiles,
                tokens: memory_tokens,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Skills,
                tokens: skill_tool_tokens,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Messages,
                tokens: message_tokens,
            },
            ContextUsageCategory {
                kind: ContextCategoryKind::Free,
                tokens: free_tokens,
            },
        ],
        memory_files,
        mcp_tools,
        agents,
        skills,
        message_breakdown,
        is_auto_compact_enabled,
        auto_compact_threshold,
    })
}

fn memory_file_source_label(source: coco_context::MemoryFileSource) -> &'static str {
    match source {
        coco_context::MemoryFileSource::Managed => "managed",
        coco_context::MemoryFileSource::UserGlobal => "user",
        coco_context::MemoryFileSource::ProjectConfig => "project_config",
        coco_context::MemoryFileSource::Project => "project",
        coco_context::MemoryFileSource::Local => "local",
    }
}

/// Glyph for a full content cell (fullness ≥ 0.7).
const GLYPH_USED: char = '\u{26C1}'; // ⛁
/// Glyph for a partially-filled category boundary cell (fullness < 0.7).
const GLYPH_PARTIAL: char = '\u{26C0}'; // ⛀
/// Glyph for the auto-compact reserved buffer.
const GLYPH_RESERVED: char = '\u{26DD}'; // ⛝
/// Glyph for a free cell.
const GLYPH_FREE: char = '\u{26F6}'; // ⛶
/// Grid width in cells.
const GRID_COLS: usize = 20;
/// Grid height in cells — 5 rows × 20 cols = 100, so one cell ≈ 1% of window.
const GRID_ROWS: usize = 5;

pub fn format_markdown(report: &ContextUsageReport) -> String {
    let mut out = String::new();
    out.push_str("## Context Usage\n\n");

    // Headline: the real, billed last-API usage. This is deliberately
    // independent of the estimated breakdown below — they are two different
    // number systems (actual usage vs. local char/4 estimates).
    out.push_str(&format!(
        "**{}/{}** · {}/{} tok ({:.0}%)\n",
        report.model.provider,
        report.model.model_id,
        fmt_token_compact(report.total_tokens),
        fmt_token_compact(report.max_tokens),
        report.percentage,
    ));
    match report.auto_compact_threshold {
        Some(threshold) => out.push_str(&format!(
            "Auto-compact at ~{} tok\n",
            fmt_token_compact(threshold)
        )),
        None => out.push_str("Auto-compact disabled\n"),
    }

    // Additive breakdown: content estimates + reserved buffer + free TILE the
    // window. `free` is derived from the category sum (not the actual-usage
    // headline), so the grid and legend are internally consistent and always
    // sum to the window size.
    let max = report.raw_max_tokens.max(1);
    let content: Vec<(ContextCategoryKind, i64)> = report
        .categories
        .iter()
        .filter(|c| c.kind != ContextCategoryKind::Free && c.tokens > 0)
        .map(|c| (c.kind, c.tokens))
        .collect();
    let category_sum: i64 = content.iter().map(|(_, tokens)| *tokens).sum();
    let reserved = report
        .auto_compact_threshold
        .map(|t| (max - t).max(0))
        .unwrap_or(0);
    let free = (max - category_sum - reserved).max(0);

    out.push_str("\n```\n");
    out.push_str(&render_usage_grid(&content, reserved, max));
    out.push_str("```\n\n");

    out.push_str(
        "Estimated usage by category (tiles the window; independent of the Used figure):\n\n",
    );
    for (kind, tokens) in &content {
        out.push_str(&format!(
            "- {GLYPH_USED} {}: {} tok ({:.1}%)\n",
            kind.label(),
            fmt_token_compact(*tokens),
            *tokens as f64 / max as f64 * 100.0,
        ));
    }
    if reserved > 0 {
        out.push_str(&format!(
            "- {GLYPH_RESERVED} Reserved (auto-compact): {} tok ({:.1}%)\n",
            fmt_token_compact(reserved),
            reserved as f64 / max as f64 * 100.0,
        ));
    }
    out.push_str(&format!(
        "- {GLYPH_FREE} Free space: {} ({:.1}%)\n",
        fmt_token_compact(free),
        free as f64 / max as f64 * 100.0,
    ));

    append_memory_section(&mut out, &report.memory_files);
    append_mcp_section(&mut out, &report.mcp_tools);
    append_agent_section(&mut out, &report.agents);
    append_skill_section(&mut out, &report.skills);

    out.push_str("\n### Messages\n\n");
    out.push_str("| Type | Tokens |\n");
    out.push_str("|---|---:|\n");
    out.push_str(&format!(
        "| User messages | {} |\n",
        format_tokens(report.message_breakdown.user_message_tokens)
    ));
    out.push_str(&format!(
        "| Assistant messages | {} |\n",
        format_tokens(report.message_breakdown.assistant_message_tokens)
    ));
    out.push_str(&format!(
        "| Tool calls | {} |\n",
        format_tokens(report.message_breakdown.tool_call_tokens)
    ));
    out.push_str(&format!(
        "| Tool results | {} |\n",
        format_tokens(report.message_breakdown.tool_result_tokens)
    ));
    out.push_str(&format!(
        "| Attachments | {} |\n",
        format_tokens(report.message_breakdown.attachment_tokens)
    ));

    let suggestions = coco_types::build_suggestions(&report.to_wire());
    if !suggestions.is_empty() {
        out.push_str("\n### Suggestions\n\n");
        for s in &suggestions {
            let icon = match s.severity {
                coco_types::SuggestionSeverity::Warning => '\u{26A0}', // ⚠
                coco_types::SuggestionSeverity::Info => '\u{2139}',    // ℹ
            };
            let savings = s
                .savings_tokens
                .map(|t| format!(" → save ~{}", fmt_token_compact(t)))
                .unwrap_or_default();
            out.push_str(&format!("- {icon} **{}**{savings}\n", s.title));
            out.push_str(&format!("  {}\n", s.detail));
        }
    }

    out
}

fn first_system_text(prompt: &[LlmMessage]) -> &str {
    match prompt.first() {
        Some(LlmMessage::System { content, .. }) => content
            .iter()
            .find_map(|part| match part {
                coco_messages::UserContent::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .unwrap_or(""),
        _ => "",
    }
}

fn estimate_tool_tokens(tool: &LanguageModelTool) -> i64 {
    match tool {
        LanguageModelTool::Function(t) => {
            let schema = serde_json::to_string(&t.input_schema).unwrap_or_default();
            let description = t.description.as_deref().unwrap_or_default();
            coco_messages::estimate_text_tokens(&format!("{}\n{}\n{}", t.name, description, schema))
        }
        LanguageModelTool::Provider(t) => {
            let serialized = serde_json::to_string(t).unwrap_or_default();
            coco_messages::estimate_text_tokens(&serialized)
        }
    }
}

fn message_breakdown(messages: &[std::sync::Arc<Message>]) -> MessageBreakdown {
    let mut tool_call_tokens = 0;
    let mut tool_result_tokens = 0;
    let mut attachment_tokens = 0;
    let mut assistant_message_tokens = 0;
    let mut user_message_tokens = 0;
    // tool name -> (call tokens, result tokens). Results attribute via the
    // tool-result message's own `tool_id`, so no id→name map is needed.
    let mut by_tool: HashMap<String, (i64, i64)> = HashMap::new();
    let mut by_attachment: HashMap<String, i64> = HashMap::new();

    for msg in messages {
        let tokens = coco_messages::estimate_message_tokens(msg.as_ref());
        match msg.as_ref() {
            Message::User(_) => user_message_tokens += tokens,
            Message::Assistant(a) => {
                assistant_message_tokens += tokens;
                if let LlmMessage::Assistant { content, .. } = &a.message {
                    for part in content {
                        if let coco_messages::AssistantContent::ToolCall(call) = part {
                            let est = coco_messages::estimate_text_tokens(
                                &serde_json::to_string(call).unwrap_or_default(),
                            );
                            tool_call_tokens += est;
                            by_tool.entry(call.tool_name.clone()).or_default().0 += est;
                        }
                    }
                }
            }
            Message::ToolResult(tr) => {
                tool_result_tokens += tokens;
                by_tool.entry(tr.tool_id.to_string()).or_default().1 += tokens;
            }
            Message::Attachment(att) => {
                attachment_tokens += tokens;
                *by_attachment
                    .entry(att.kind.as_str().to_string())
                    .or_default() += tokens;
            }
            Message::System(_) | Message::Progress(_) | Message::Tombstone(_) => {}
        }
    }

    let mut tool_calls_by_type: Vec<ToolTypeBreakdown> = by_tool
        .into_iter()
        .map(|(name, (call_tokens, result_tokens))| ToolTypeBreakdown {
            name,
            call_tokens,
            result_tokens,
        })
        .collect();
    tool_calls_by_type.sort_by_key(|t| std::cmp::Reverse(t.call_tokens + t.result_tokens));

    let mut attachments_by_type: Vec<AttachmentTypeBreakdown> = by_attachment
        .into_iter()
        .map(|(name, tokens)| AttachmentTypeBreakdown { name, tokens })
        .collect();
    attachments_by_type.sort_by_key(|a| std::cmp::Reverse(a.tokens));

    MessageBreakdown {
        tool_call_tokens,
        tool_result_tokens,
        attachment_tokens,
        assistant_message_tokens,
        user_message_tokens,
        tool_calls_by_type,
        attachments_by_type,
    }
}

async fn agent_estimates(engine: &QueryEngine) -> Vec<AgentEstimate> {
    let Some(catalog) = &engine.agent_catalog else {
        return Vec::new();
    };
    let ready_mcp_servers = engine.mcp_servers_ready_snapshot().await;
    let mut rows = Vec::new();
    match ready_mcp_servers {
        Some(servers) => {
            for def in catalog.active_with_mcp(&servers) {
                push_agent_estimate(&mut rows, def);
            }
        }
        None => {
            for def in catalog.active() {
                push_agent_estimate(&mut rows, def);
            }
        }
    }
    rows
}

fn push_agent_estimate(rows: &mut Vec<AgentEstimate>, def: &coco_types::AgentDefinition) {
    if def.source == coco_types::AgentSource::BuiltIn {
        return;
    }
    let when_to_use = def
        .when_to_use
        .as_deref()
        .or(def.description.as_deref())
        .unwrap_or_default();
    let agent_type = def.agent_type.to_string();
    rows.push(AgentEstimate {
        tokens: coco_messages::estimate_text_tokens(&format!("{agent_type}\n{when_to_use}")),
        agent_type,
        source: def.source.as_str().to_string(),
    });
}

fn skill_estimates(
    engine: &QueryEngine,
    skill_manager: Option<&coco_skills::SkillManager>,
) -> Vec<SkillEstimate> {
    let Some(manager) = skill_manager else {
        return Vec::new();
    };
    manager
        .visible(&engine.config.features)
        .into_iter()
        .map(|skill| SkillEstimate {
            name: skill
                .display_name
                .as_deref()
                .unwrap_or(&skill.name)
                .to_string(),
            source: skill_source_label(&skill.source),
            tokens: coco_skills::estimate_skill_tokens(&skill),
        })
        .collect()
}

fn skill_source_label(source: &coco_skills::SkillSource) -> String {
    match source {
        coco_skills::SkillSource::Bundled => "bundled".to_string(),
        coco_skills::SkillSource::User { path } => format!("user:{}", path.display()),
        coco_skills::SkillSource::Project { path } => format!("project:{}", path.display()),
        coco_skills::SkillSource::Plugin { plugin_name } => format!("plugin:{plugin_name}"),
        coco_skills::SkillSource::Managed { path } => format!("managed:{}", path.display()),
        coco_skills::SkillSource::Mcp { server_name } => format!("mcp:{server_name}"),
    }
}

fn append_memory_section(out: &mut String, rows: &[MemoryFileEstimate]) {
    if rows.is_empty() {
        return;
    }
    out.push_str("\n### Memory Files\n\n");
    out.push_str("| Path | Source | Tokens |\n");
    out.push_str("|---|---|---:|\n");
    for row in rows {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&row.path),
            markdown_cell(&row.source),
            format_tokens(row.tokens)
        ));
    }
}

fn append_mcp_section(out: &mut String, rows: &[McpToolEstimate]) {
    if rows.is_empty() {
        return;
    }
    out.push_str("\n### MCP Tools\n\n");
    out.push_str("| Tool | Server | Tokens | State |\n");
    out.push_str("|---|---|---:|---|\n");
    for row in rows {
        let state = if row.deferred { "deferred" } else { "loaded" };
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            markdown_cell(&row.name),
            markdown_cell(&row.server_name),
            format_tokens(row.tokens),
            state
        ));
    }
}

fn append_agent_section(out: &mut String, rows: &[AgentEstimate]) {
    if rows.is_empty() {
        return;
    }
    out.push_str("\n### Agents\n\n");
    out.push_str("| Agent Type | Source | Tokens |\n");
    out.push_str("|---|---|---:|\n");
    for row in rows {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&row.agent_type),
            markdown_cell(&row.source),
            format_tokens(row.tokens)
        ));
    }
}

fn append_skill_section(out: &mut String, rows: &[SkillEstimate]) {
    if rows.is_empty() {
        return;
    }
    out.push_str("\n### Skills\n\n");
    out.push_str("| Skill | Source | Tokens |\n");
    out.push_str("|---|---|---:|\n");
    for row in rows {
        out.push_str(&format!(
            "| {} | {} | {} |\n",
            markdown_cell(&row.name),
            markdown_cell(&row.source),
            format_tokens(row.tokens)
        ));
    }
}

fn markdown_cell(value: &str) -> String {
    value.replace('|', "\\|")
}

fn format_tokens(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Render the usage grid for the text form via the shared `build_grid`
/// allocator (category-first, reserved last, free fills the gap). Text has no
/// color, so glyphs alone distinguish full / partial / reserved / free cells.
fn render_usage_grid(categories: &[(ContextCategoryKind, i64)], reserved: i64, max: i64) -> String {
    let cells = coco_types::build_grid(categories, max, reserved, GRID_COLS, GRID_ROWS);
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let glyph = match cell.kind {
            GridCellKind::Category(_) if cell.fullness >= 0.7 => GLYPH_USED,
            GridCellKind::Category(_) => GLYPH_PARTIAL,
            GridCellKind::Reserved => GLYPH_RESERVED,
            GridCellKind::Free => GLYPH_FREE,
        };
        out.push(glyph);
        if (i + 1).is_multiple_of(GRID_COLS) {
            out.push('\n');
        } else {
            out.push(' ');
        }
    }
    out
}

#[cfg(test)]
#[path = "context_analysis.test.rs"]
mod tests;
