//! `features` block — every feature gate, its stage, and its default.
//!
//! Key / stage / default come from the `FEATURES` registry via
//! `coco_types::all_features()`. `FeatureSpec` carries no description field, so
//! the blurb lives in the exhaustive match below — it also carries markdown
//! links (`[sandbox](sandbox.md)`) that have no business in a doc comment.
//! A new `Feature` variant stops compiling until someone describes it.

use anyhow::Result;
use coco_types::Feature;
use coco_types::all_features;
use coco_types::features::Stage;

pub fn render() -> Result<String> {
    let mut out = String::from(
        "| Key | Stage | Default | What it does |\n|-----|-------|---------|--------------|",
    );
    for spec in all_features() {
        let key = spec.key;
        let stage = stage_label(spec.stage);
        let default = if spec.default_enabled { "on" } else { "off" };
        let description = description(spec.id);
        out.push_str(&format!(
            "\n| `{key}` | {stage} | {default} | {description} |"
        ));
    }
    Ok(out)
}

/// Experimental is bolded: it is the one stage users can act on from the
/// `/experimental` menu.
fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Stable => "Stable",
        Stage::UnderDevelopment => "Under development",
        Stage::Experimental { .. } => "**Experimental**",
    }
}

fn description(feature: Feature) -> &'static str {
    match feature {
        Feature::WebSearch => "Exposes the `web_search` tool to the model.",
        Feature::WebFetch => "Exposes the `web_fetch` tool to the model.",
        Feature::Mcp => {
            "Exposes MCP management tools and dynamic MCP server tool wrappers to the model."
        }
        Feature::McpSkills => {
            "Discovers skills published by connected MCP servers and surfaces them as skills and \
             slash commands. Requires `mcp`."
        }
        Feature::NotebookEdit => "Exposes the `notebook_edit` tool to the model.",
        Feature::TaskV2 => {
            "V2 task tooling (`TaskCreate` / `TaskGet` / `TaskList` / `TaskUpdate`). When off, the \
             V1 `TodoWrite` tool is exposed instead."
        }
        Feature::ToolSearch => {
            "Lazy tool-schema loading via the `ToolSearch` tool. Deferrable tools are sent \
             name-only on the first turn and discovered on demand, saving a large share of the \
             tools-array token budget. When off, every enabled tool ships its full schema in \
             every request."
        }
        Feature::DynamicModelCard => {
            "Refreshes the model-card catalog from OpenRouter in a non-blocking startup task. The \
             bundled snapshot remains the fallback."
        }
        Feature::OutputRewrite => {
            "Compresses Bash dev-tool output (git, cargo, test runners, linters, docker) before it \
             reaches the model. Permission rules and sandbox decisions always evaluate the \
             original command. Silently no-ops with no backend available."
        }
        Feature::Sandbox => {
            "Runs shell commands inside a sandbox. Default off for risk-conservatism, not \
             immaturity. See [sandbox](sandbox.md)."
        }
        Feature::PlanMode => {
            "Plan-mode subsystem: the `EnterPlanMode` / `ExitPlanMode` tools and the plan-mode \
             context reminder. Turn off to reclaim the reminder tokens and tool schema."
        }
        Feature::Workflow => "Dynamic local workflow scripts.",
        Feature::AutoMemory => {
            "Auto-memory subsystem: extraction, team sync, and relevant-memory injection."
        }
        Feature::SkillLearning => {
            "Autonomous skill-learning loop that distills sessions into agent-owned skills, plus \
             the periodic curator. Off by default because it auto-writes executable artifacts."
        }
        Feature::Retrieval => "Retrieval subsystem: BM25, vector, AST, RepoMap, and reranker.",
        Feature::AgentTeams => {
            "Persistent agent teams and teammate orchestration: spawn addressable teammates and \
             coordinate via `SendMessage`."
        }
        Feature::Worktree => "Worktree tools (`EnterWorktree` / `ExitWorktree`).",
        Feature::Lsp => "LSP-backed code intelligence tool.",
        Feature::Voice => {
            "Voice input (speech-to-text dictation): microphone capture and STT, surfaced through \
             `/voice` and `/voice-config`. Off by default because microphone access and outbound \
             audio to a third party are privacy- and cost-sensitive."
        }
        Feature::Proactive => "Autonomous, tick-driven assistant loop helpers.",
        Feature::KairosBrief => "Brief user-message channel (`SendUserMessage`).",
        Feature::AgentTriggers => {
            "Local scheduling tools (`Cron*`, `ScheduleWakeup`, `Monitor`) and the `/loop` skill."
        }
        Feature::AgentTriggersRemote => "The `/schedule` skill for remote agent scheduling.",
        Feature::BuildingClaudeApps => "The `/claude-api` skill.",
        Feature::KairosDream => "The `/dream` skill for memory consolidation.",
        Feature::ReviewArtifact => "The `/hunter` bug-finding review skill.",
        Feature::RunSkillGenerator => "The `/run-skill-generator` skill.",
        Feature::ToolUseSummary => {
            "Short label emitted after each tool batch via an extra Fast-role call. Off by \
             default: it costs a call per tool-using turn and degrades to nothing on \
             reasoning-class Fast models."
        }
        Feature::ClaudeInChrome => "Auto-detects a Claude in Chrome installation.",
        Feature::NewInit => {
            "The newer multi-phase `/init` prompt instead of the single-prompt version."
        }
        Feature::ReactiveCompact => "Reactive compaction strategy instead of summarize-all.",
        Feature::PromptCacheBreakDetection => {
            "Prompt-cache break detection wiring during compaction."
        }
        Feature::Speculation => {
            "Pre-executes accepted prompt suggestions in an overlay sandbox and injects the result \
             instantly on accept."
        }
    }
}
