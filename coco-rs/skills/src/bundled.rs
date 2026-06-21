//! Bundled skill definitions shipped with the binary.
//!
//! ## Inventory rules
//!
//! 1. **Inventory**: matches the upstream bundled skill set exactly. Skills that
//!    ship only as gated registrations (`feature(...)` calls) are gated here
//!    via `gated_by: Some(Feature::*)`. Skills that upstream ships as ant-only
//!    are general-purpose workflows and are registered **unconditionally** here
//!    (see root `CLAUDE.md` "Always-Enabled General-Purpose Commands").
//! 2. **No coco-only extras**: `/commit`, `review-pr`, and `pdf` are not
//!    bundled skills — `/commit` is a slash command, `review-pr` is covered by
//!    the review command, and PDF reading is handled by the Read tool. They were
//!    removed in Round 11.
//! 3. **Feature flags**: each gated skill maps to a `coco_types::Feature`
//!    variant — see `features.rs` and `parity-skills-commands-plugins.md §1.3`.
//! 4. **`disable_model_invocation` / `user_invocable`**: matched per-skill to
//!    upstream frontmatter.

use coco_types::Feature;
use coco_types::ToolName;
use std::collections::HashMap;

use crate::SkillContext;
use crate::SkillDefinition;
use crate::SkillSource;

mod batch;
mod claude_api;
mod claude_in_chrome;
mod debug;
mod dream;
mod hunter;
mod keybindings;
mod loop_skill;
mod lorem_ipsum;
mod remember;
mod run_skill_generator;
mod schedule;
mod simplify;
mod skillify;
mod stuck;
mod update_config;
mod verify;

/// Create a bundled skill with common defaults.
fn bundled(
    name: &str,
    description: &str,
    prompt: &str,
    allowed_tools: Vec<&str>,
) -> SkillDefinition {
    SkillDefinition {
        name: name.to_string(),
        display_name: None,
        description: description.to_string(),
        prompt: prompt.to_string(),
        source: SkillSource::Bundled,
        aliases: vec![],
        allowed_tools: Some(allowed_tools.into_iter().map(String::from).collect()),
        model: None,
        model_role: None,
        when_to_use: None,
        argument_names: vec![],
        paths: vec![],
        effort: None,
        context: SkillContext::Inline,
        agent: None,
        version: None,
        disabled: false,
        hooks: None,
        argument_hint: None,
        user_invocable: true,
        disable_model_invocation: false,
        shell: None,
        content_length: prompt.len() as i64,
        has_user_specified_description: true,
        progress_message: Some("running".to_string()),
        is_hidden: false,
        gated_by: None,
        files: HashMap::new(),
        skill_root: None,
    }
}

/// Get all bundled skill definitions.
///
/// **Selection logic** (coco-rs drops the ant gate):
/// - Always-on skills (update-config, keybindings-help, batch) plus the
///   formerly-ant general-purpose skills (verify, debug, skillify, remember,
///   simplify, stuck, lorem-ipsum) are returned unconditionally.
/// - 7 feature-gated skills (loop, schedule, claude-api, dream, hunter,
///   claude-in-chrome, run-skill-generator) carry `gated_by: Some(Feature::*)`
///   and are filtered by `SkillManager::visible(features)` — these remain
///   gated because they guard real capabilities, not a user-type convention.
pub fn get_bundled_skills() -> Vec<SkillDefinition> {
    let mut skills: Vec<SkillDefinition> = Vec::new();

    // ───────────────── unconditional ─────────────────

    // /update-config(unconditional)
    {
        let mut s = bundled(
            "update-config",
            "Configure settings via settings.json",
            update_config::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Edit.as_str(),
                ToolName::Write.as_str(),
                ToolName::Glob.as_str(),
            ],
        );
        s.when_to_use = Some("For hooks, permissions, env vars, or settings changes".to_string());
        skills.push(s);
    }

    // /keybindings-help(unconditional)
    // Uses `userInvocable: false` so only the model invokes.
    {
        let mut s = bundled(
            "keybindings-help",
            "Customize keyboard shortcuts and keybindings",
            keybindings::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Edit.as_str(),
                ToolName::Write.as_str(),
            ],
        );
        s.when_to_use = Some(
            "When the user wants to customize keyboard shortcuts, rebind keys, add chord bindings, or modify keybindings.json".to_string(),
        );
        s.user_invocable = false;
        s.is_hidden = true; // isHidden = !userInvocable
        skills.push(s);
    }

    // /batch(unconditional, disable_model_invocation)
    {
        let mut s = bundled(
            "batch",
            "Run a prompt or command on multiple files",
            &batch::prompt(),
            vec![
                ToolName::Bash.as_str(),
                ToolName::Read.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
                ToolName::Edit.as_str(),
                ToolName::Write.as_str(),
            ],
        );
        s.when_to_use =
            Some("When user wants sweeping mechanical changes across many files".to_string());
        s.disable_model_invocation = true;
        skills.push(s);
    }

    // ─── formerly ant-only — now unconditional (coco-rs drops USER_TYPE gate) ───

    {
        // /verify(general-purpose)
        skills.push(bundled(
            "verify",
            "Verify a code change does what it should by running the app",
            verify::PROMPT,
            vec![
                ToolName::Bash.as_str(),
                ToolName::Read.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
            ],
        ));

        // /debug(ant-only, disable_model_invocation)
        let mut debug_skill = bundled(
            "debug",
            "Debug your current Claude Code session by reading the session debug log. Includes all event logging",
            debug::PROMPT,
            vec![
                ToolName::Bash.as_str(),
                ToolName::Read.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
            ],
        );
        debug_skill.disable_model_invocation = true;
        skills.push(debug_skill);

        // /skillify(ant-only)
        let mut sk = bundled(
            "skillify",
            "Convert a workflow into a reusable skill file",
            &skillify::prompt(),
            vec![
                ToolName::Read.as_str(),
                ToolName::Write.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
            ],
        );
        sk.when_to_use = Some("When user wants to automate a repeatable workflow".to_string());
        skills.push(sk);

        // /remember — ant gate dropped, but the auto-memory capability gate is
        // kept (mirrors TS `isEnabled: isAutoMemoryEnabled()`): the skill audits
        // auto-memory, so it stays hidden until `Feature::AutoMemory` is enabled.
        // Registered here for catalog parity; `visible()` applies the gate.
        let mut rem = bundled(
            "remember",
            "Review auto-memory entries and propose promotions to CLAUDE.md, CLAUDE.local.md, or shared memory. Also detects outdated, conflicting, and duplicate entries across memory layers.",
            remember::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Write.as_str(),
                ToolName::Glob.as_str(),
            ],
        );
        rem.when_to_use = Some(
            "Use when the user wants to review, organize, or promote their auto-memory entries. Also useful for cleaning up outdated or conflicting entries across CLAUDE.md, CLAUDE.local.md, and auto-memory.".to_string(),
        );
        rem.gated_by = Some(Feature::AutoMemory);
        skills.push(rem);

        // /simplify(general-purpose) — upstream simplify.ts has no ant gate
        skills.push(bundled(
            "simplify",
            "Review changed code for reuse, quality, and efficiency, then fix any issues found",
            &simplify::prompt(),
            vec![
                ToolName::Bash.as_str(),
                ToolName::Read.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
                ToolName::Edit.as_str(),
                ToolName::Write.as_str(),
            ],
        ));

        // /stuck(ant-only)
        skills.push(bundled(
            "stuck",
            "Help when stuck in loops or debugging dead ends",
            stuck::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
                ToolName::Bash.as_str(),
            ],
        ));

        // /lorem-ipsum(ant-only)
        let mut li = bundled(
            "lorem-ipsum",
            "Generate filler text for long context testing. Specify token count as argument (e.g., /lorem-ipsum 50000). Outputs approximately the requested number of tokens.",
            lorem_ipsum::PROMPT,
            vec![],
        );
        li.argument_hint = Some("[token_count]".to_string());
        skills.push(li);
    }

    // ───────────────── feature-gated ─────────────────

    // /loop(gated AGENT_TRIGGERS)
    {
        let mut s = bundled(
            "loop",
            "Run a prompt or slash command on a recurring interval",
            &loop_skill::prompt(),
            vec![
                ToolName::Bash.as_str(),
                ToolName::Read.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
            ],
        );
        s.gated_by = Some(Feature::AgentTriggers);
        skills.push(s);
    }

    // /schedule(gated AGENT_TRIGGERS_REMOTE)
    {
        let mut s = bundled(
            "schedule",
            "Create, update, list, or run scheduled remote agents (triggers) that execute on a cron schedule",
            &schedule::prompt(),
            vec![
                ToolName::RemoteTrigger.as_str(),
                ToolName::AskUserQuestion.as_str(),
            ],
        );
        s.when_to_use = Some(
            "When user wants to schedule recurring remote agents or manage scheduled triggers"
                .to_string(),
        );
        s.gated_by = Some(Feature::AgentTriggersRemote);
        skills.push(s);
    }

    // /claude-api(gated BUILDING_CLAUDE_APPS)
    {
        let mut s = bundled(
            "claude-api",
            "Build apps with the Claude API or Anthropic SDK",
            claude_api::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Grep.as_str(),
                ToolName::Glob.as_str(),
                ToolName::WebFetch.as_str(),
            ],
        );
        s.when_to_use = Some(
            "When code imports anthropic SDK or user asks about Claude API, Anthropic SDKs, or Agent SDK".to_string(),
        );
        s.gated_by = Some(Feature::BuildingClaudeApps);
        skills.push(s);
    }

    // /dream(gated KAIROS|KAIROS_DREAM)
    {
        let mut s = bundled(
            "dream",
            "Run KAIROS auto-dream memory consolidation: review and consolidate session memory",
            dream::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Edit.as_str(),
                ToolName::Write.as_str(),
                ToolName::Grep.as_str(),
                ToolName::Glob.as_str(),
            ],
        );
        s.gated_by = Some(Feature::KairosDream);
        skills.push(s);
    }

    // /hunter(gated REVIEW_ARTIFACT)
    {
        let mut s = bundled(
            "hunter",
            "Deep bug-finding review: scour code for bugs, security issues, and edge cases",
            hunter::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Bash.as_str(),
                ToolName::Grep.as_str(),
                ToolName::Glob.as_str(),
            ],
        );
        s.gated_by = Some(Feature::ReviewArtifact);
        skills.push(s);
    }

    // /claude-in-chrome
    // (gated by `shouldAutoEnableClaudeInChrome()` runtime check)
    {
        let mut s = bundled(
            "claude-in-chrome",
            "Automates your Chrome browser to interact with web pages — clicking elements, filling forms, capturing screenshots, reading console logs, and navigating sites. Opens pages in new tabs within your existing Chrome session. Requires site-level permissions before executing (configured in the extension).",
            claude_in_chrome::PROMPT,
            // Browser MCP tools are dynamic; allowed_tools entries must be `mcp__claude-in-chrome__*`
            // populated at startup by the Chrome MCP integration. Empty here is a placeholder.
            vec![],
        );
        s.when_to_use = Some(
            "When the user wants to interact with web pages, automate browser tasks, capture screenshots, read console logs, or perform any browser-based actions. Always invoke BEFORE attempting to use any mcp__claude-in-chrome__* tools.".to_string(),
        );
        s.gated_by = Some(Feature::ClaudeInChrome);
        skills.push(s);
    }

    // /run-skill-generator(gated RUN_SKILL_GENERATOR)
    {
        let mut s = bundled(
            "run-skill-generator",
            "Create or refine a SKILL.md file for a custom workflow",
            run_skill_generator::PROMPT,
            vec![
                ToolName::Read.as_str(),
                ToolName::Write.as_str(),
                ToolName::Edit.as_str(),
                ToolName::Glob.as_str(),
                ToolName::Grep.as_str(),
                ToolName::AskUserQuestion.as_str(),
            ],
        );
        s.gated_by = Some(Feature::RunSkillGenerator);
        skills.push(s);
    }

    skills
}

/// Register all bundled skills into a SkillManager. Feature-gated skills are
/// still filtered later at the [`crate::SkillManager::visible`] layer.
pub fn register_bundled(manager: &crate::SkillManager) {
    for skill in get_bundled_skills() {
        manager.register(skill);
    }
}

#[cfg(test)]
#[path = "bundled.test.rs"]
mod tests;
