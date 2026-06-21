//! Render the dynamic AgentTool prompt string.
//!
//! The template is byte-for-byte faithful modulo three deliberate
//! substitutions:
//!
//! - tool names (`Agent`, `SendMessage`, `Read`, `Glob`) come from the
//!   typed [`coco_types::ToolName`] enum so any rename in the runtime
//!   propagates here automatically;
//! - the `isolation: "remote"` bullet is gated by
//!   [`PromptOptions::ant_build`] — coco-rs is a 3p build by default,
//!   so the bullet is suppressed unless callers opt-in;
//! - the `getFeatureValue_CACHED_MAY_BE_STALE('tengu_agent_list_attach', …)`
//!   gate is replaced by an explicit [`PromptOptions::list_via_attachment`]
//!   bool. The CLI bootstrap reads `COCO_AGENT_LIST_IN_MESSAGES` and
//!   threads the result here.
//!
//! Two contracts you should keep stable:
//!
//! 1. **Agent line format** (`prompt.ts:43-45`):
//!    `- {agentType}: {whenToUse} (Tools: {toolsDescription})`
//! 2. **Tools description branches** (`prompt.ts:15-37`):
//!    - allow + deny: comma-separated allow-list minus deny-list (or `"None"`)
//!    - allow only: comma-separated allow-list
//!    - deny only: `"All tools except {deny-list}"`
//!    - neither: `"All tools"`

use coco_types::{AgentDefinition, ToolName};

use crate::snapshot::AgentCatalogSnapshot;

/// Inputs that shape the AgentTool prompt for a given turn.
///
/// All fields default to "off" so existing callers keep their previous
/// shape without opting into newer sections (fork, embedded tools,
/// teammate variants, attachment listing, ant-only `remote` isolation).
#[derive(Debug, Default, Clone)]
pub struct PromptOptions {
    /// Restrict the listed agents to this set (e.g. user's
    /// `Agent(type1,type2)` permission rule). `None` = no restriction.
    pub allowed_agent_types: Option<Vec<String>>,
    /// Agent types denied by an `Agent(<type>)` permission deny rule. These
    /// are dropped from the listing so the model never sees an agent that
    /// `AgentTool::execute` would reject with a permission error. Mirrors TS
    /// `filterDeniedAgents`. Empty = no deny filtering.
    pub denied_agent_types: Vec<String>,
    /// Pre-filter MCP-required agents whose servers are not yet ready.
    /// `None` = no MCP filtering.
    pub ready_mcp_servers: Option<Vec<String>>,
    /// When true, render slim coordinator prompt (no usage notes).
    pub coordinator_mode: bool,
    /// When true, render fork guidance section + fork-aware examples
    /// and swap the closing sentence on the shared header.
    pub fork_enabled: bool,
    /// When true, the host build embeds search tools (`bfs`/`ugrep`)
    /// inside the Bash tool, so the "When NOT to use" section points
    /// at `find` / `grep` via Bash instead of dedicated `Glob` / `Grep`
    /// tools.
    pub has_embedded_search_tools: bool,
    /// True when the parent session is itself an in-process teammate.
    /// Drops the run_in_background / name / team_name / mode bullet
    /// because in-process teammates only support synchronous spawn.
    pub is_in_process_teammate: bool,
    /// True when the parent session is a (non in-process) teammate.
    /// Drops the name / team_name / mode bullet because teammates
    /// cannot spawn other teammates. Ignored when
    /// [`is_in_process_teammate`] is also true.
    ///
    /// [`is_in_process_teammate`]: Self::is_in_process_teammate
    pub is_teammate: bool,
    /// Inject the agent list into a `<system-reminder>` attachment
    /// instead of inline in the tool description.
    pub list_via_attachment: bool,
    /// Pro subscriptions skip the inline "Launch multiple agents
    /// concurrently" bullet because the same guidance is shown by the
    /// agent_listing_delta attachment for them.
    pub is_pro_subscription: bool,
    /// True when the host disabled background tasks via
    /// `COCO_BACKGROUND_TASKS_DISABLE`. Suppresses the
    /// `run_in_background` paragraphs.
    pub background_tasks_disabled: bool,
    /// Internal-build flag enabling the `isolation: "remote"` bullet.
    /// coco-rs ships only the `worktree` runtime, so this stays off
    /// in 3p builds.
    pub ant_build: bool,
    /// Model-aware file-write tool for the usage examples, resolved by the
    /// caller from the model's available tools (`Write` for Claude,
    /// `apply_patch` for gpt-5). `None` → fall back to `Write`. Keeps the
    /// example from naming a tool the model lacks. See
    /// [`coco_types::ToolName::write_tool_for`].
    pub file_write_tool: Option<ToolName>,
}

pub struct AgentToolPromptRenderer<'a> {
    snapshot: &'a AgentCatalogSnapshot,
}

impl<'a> AgentToolPromptRenderer<'a> {
    pub fn new(snapshot: &'a AgentCatalogSnapshot) -> Self {
        Self { snapshot }
    }

    /// Format the agent listing block in source-load order (built-in →
    /// plugin → user → project → flag → managed) so the
    /// model-visible block and its prompt-cache key are stable.
    pub fn agent_list(&self, opts: &PromptOptions) -> String {
        self.snapshot
            .active_in_load_order()
            .filter(|def| visible_to_prompt(def, opts))
            .map(format_agent_line)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render the full AgentTool description that goes into the tool
    /// schema. Coordinator mode keeps it slim; non-coordinator mode
    /// includes when-not-to-use, full usage notes, optional fork
    /// section, "Writing the prompt", and examples.
    pub fn full_prompt(&self, opts: &PromptOptions) -> String {
        let agent = ToolName::Agent.as_str();

        let agent_list_section = if opts.list_via_attachment {
            "Available agent types are listed in <system-reminder> messages in the conversation."
                .to_owned()
        } else if self.snapshot.active_count() == 0 {
            // Keep the literal header sentence so downstream
            // prompt-cache keys remain stable; the listing collapses
            // to an empty line.
            "Available agent types and the tools they have access to:".to_owned()
        } else {
            let listing = self.agent_list(opts);
            format!("Available agent types and the tools they have access to:\n{listing}")
        };

        let closing_sentence = if opts.fork_enabled {
            format!(
                "When using the {agent} tool, specify a subagent_type to use a specialized \
                 agent, or omit it to fork yourself \u{2014} a fork inherits your full \
                 conversation context.",
            )
        } else {
            format!(
                "When using the {agent} tool, specify a subagent_type parameter to select \
                 which agent type to use. If omitted, the general-purpose agent is used.",
            )
        };

        let shared = format!(
            "Launch a new agent to handle complex, multi-step tasks autonomously.\n\
             \n\
             The {agent} tool launches specialized agents (subprocesses) that autonomously \
             handle complex tasks. Each agent type has specific capabilities and tools \
             available to it.\n\
             \n\
             {agent_list_section}\n\
             \n\
             {closing_sentence}",
        );

        if opts.coordinator_mode {
            // The coordinator system prompt already covers usage /
            // examples / when-not-to-use.
            return shared;
        }

        let mut out = String::with_capacity(shared.len() + 4096);
        out.push_str(&shared);

        let when_not_to_use = when_not_to_use_section(opts);
        if !when_not_to_use.is_empty() {
            out.push('\n');
            out.push_str(&when_not_to_use);
        }

        out.push_str("\n\n");
        out.push_str(&usage_notes_section(opts));

        if opts.fork_enabled {
            out.push_str(&when_to_fork_section());
        }
        out.push_str(&writing_the_prompt_section(opts));

        out.push_str("\n\n");
        out.push_str(&if opts.fork_enabled {
            fork_examples()
        } else {
            current_examples(opts)
        });

        out
    }
}

fn visible_to_prompt(def: &AgentDefinition, opts: &PromptOptions) -> bool {
    if let Some(allowed) = opts.allowed_agent_types.as_ref()
        && !allowed.iter().any(|t| t == &def.name)
    {
        return false;
    }
    // Mirror TS `filterDeniedAgents`: drop agents an `Agent(<type>)` deny rule
    // forbids, so the model never sees an agent `AgentTool::execute` rejects.
    if opts.denied_agent_types.iter().any(|t| t == &def.name) {
        return false;
    }
    if !def.required_mcp_servers.is_empty() {
        let Some(ready) = opts.ready_mcp_servers.as_ref() else {
            return false;
        };
        // Case-insensitive substring match.
        let all_ready = def.required_mcp_servers.iter().all(|required| {
            let needle = required.to_ascii_lowercase();
            ready
                .iter()
                .any(|name| name.to_ascii_lowercase().contains(&needle))
        });
        if !all_ready {
            return false;
        }
    }
    true
}

fn format_agent_line(def: &AgentDefinition) -> String {
    let when = def
        .when_to_use
        .as_deref()
        .or(def.description.as_deref())
        .unwrap_or("");
    format!(
        "- {name}: {when} (Tools: {tools})",
        name = def.name,
        tools = format_tools_description(&def.allowed_tools, &def.disallowed_tools),
    )
}

/// Compute the tools description string.
/// `Wildcard` allowed-list means "All tools"
/// (or "All tools except …" when deny-list is non-empty).
pub fn format_tools_description(
    allowed: &coco_types::ToolAllowList,
    disallowed: &[String],
) -> String {
    let deny_empty = disallowed.is_empty();
    match allowed {
        coco_types::ToolAllowList::Explicit(list) if !list.is_empty() => {
            if !deny_empty {
                let effective: Vec<&String> =
                    list.iter().filter(|t| !disallowed.contains(t)).collect();
                if effective.is_empty() {
                    return "None".to_owned();
                }
                return effective
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
            }
            list.join(", ")
        }
        coco_types::ToolAllowList::Explicit(_) | coco_types::ToolAllowList::Wildcard => {
            if !deny_empty {
                format!("All tools except {}", disallowed.join(", "))
            } else {
                "All tools".to_owned()
            }
        }
    }
}

/// "When NOT to use" guidance. Empty string when fork is enabled.
fn when_not_to_use_section(opts: &PromptOptions) -> String {
    if opts.fork_enabled {
        return String::new();
    }
    let agent = ToolName::Agent.as_str();
    let file_read = ToolName::Read.as_str();
    let glob = ToolName::Glob.as_str();
    // Embedded builds point at `find`/`grep` via Bash; non-embedded
    // uses the Glob tool for both file lookup and content search
    // (intentional — Glob is both find-the-file and first-pass file
    // enumeration).
    let (file_search_hint, content_search_hint) = if opts.has_embedded_search_tools {
        ("`find` via the Bash tool", "`grep` via the Bash tool")
    } else {
        let glob = format!("the {glob} tool");
        // Both halves point at Glob; using a single owned string keeps
        // allocations minimal but the duplicated value is fine for
        // one prompt render.
        return format!(
            "\nWhen NOT to use the {agent} tool:\n\
             - If you want to read a specific file path, use the {file_read} tool or {glob} \
             instead of the {agent} tool, to find the match more quickly\n\
             - If you are searching for a specific class definition like \"class Foo\", use \
             {glob} instead, to find the match more quickly\n\
             - If you are searching for code within a specific file or set of 2-3 files, use \
             the {file_read} tool instead of the {agent} tool, to find the match more quickly\n\
             - Other tasks that are not related to the agent descriptions above\n",
        );
    };

    format!(
        "\nWhen NOT to use the {agent} tool:\n\
         - If you want to read a specific file path, use the {file_read} tool or {file_search_hint} \
         instead of the {agent} tool, to find the match more quickly\n\
         - If you are searching for a specific class definition like \"class Foo\", use \
         {content_search_hint} instead, to find the match more quickly\n\
         - If you are searching for code within a specific file or set of 2-3 files, use \
         the {file_read} tool instead of the {agent} tool, to find the match more quickly\n\
         - Other tasks that are not related to the agent descriptions above\n",
    )
}

/// Full usage-notes block.
fn usage_notes_section(opts: &PromptOptions) -> String {
    let agent = ToolName::Agent.as_str();
    let send_message = ToolName::SendMessage.as_str();

    let mut s = String::new();
    s.push_str("Usage notes:\n");
    // First bullet always present.
    s.push_str(
        "- Always include a short description (3-5 words) summarizing what the agent will do",
    );
    // Concurrency hint — only when the agent list is inline AND the
    // user is non-pro.
    if !opts.list_via_attachment && !opts.is_pro_subscription {
        s.push_str(
            "\n- Launch multiple agents concurrently whenever possible, to maximize \
             performance; to do that, use a single message with multiple tool uses",
        );
    }
    s.push('\n');

    s.push_str(
        "- When the agent is done, it will return a single message back to you. The result \
         returned by the agent is not visible to the user. To show the user the result, you \
         should send a text message back to the user with a concise summary of the result.",
    );

    // run_in_background paragraphs — gates on
    // !background_tasks_disabled && !is_in_process_teammate && !fork_enabled.
    if !opts.background_tasks_disabled && !opts.is_in_process_teammate && !opts.fork_enabled {
        s.push_str(
            "\n- You can optionally run agents in the background using the run_in_background \
             parameter. When an agent runs in the background, you will be automatically \
             notified when it completes \u{2014} do NOT sleep, poll, or proactively check on its \
             progress. Continue with other work or respond to the user instead.\n\
             - **Foreground vs background**: Use foreground (default) when you need the agent's \
             results before you can proceed \u{2014} e.g., research agents whose findings inform \
             your next steps. Use background when you have genuinely independent work to do in \
             parallel.",
        );
    }
    s.push('\n');

    // SendMessage continuation.
    let fresh_agent_phrase = if opts.fork_enabled {
        " Each fresh Agent invocation with a subagent_type starts without context \u{2014} \
         provide a complete task description."
    } else {
        " Each Agent invocation starts fresh \u{2014} provide a complete task description."
    };
    s.push_str(&format!(
        "- To continue a previously spawned agent, use {send_message} with the agent's ID or \
         name as the `to` field. The agent resumes with its full context preserved.{fresh_agent_phrase}\n",
    ));

    // Trust + research-vs-write.
    s.push_str("- The agent's outputs should generally be trusted\n");
    let user_intent_clause = if opts.fork_enabled {
        ""
    } else {
        ", since it is not aware of the user's intent"
    };
    s.push_str(&format!(
        "- Clearly tell the agent whether you expect it to write code or just to do research \
         (search, file reads, web fetches, etc.){user_intent_clause}\n",
    ));

    // Proactive + parallel hint.
    s.push_str(
        "- If the agent description mentions that it should be used proactively, then you \
         should try your best to use it without the user having to ask for it first. Use your \
         judgement.\n",
    );
    s.push_str(&format!(
        "- If the user specifies that they want you to run agents \"in parallel\", you MUST \
         send a single message with multiple {agent} tool use content blocks. For example, if \
         you need to launch both a build-validator agent and a test-runner agent in parallel, \
         send a single message with both tool calls.\n",
    ));

    // Worktree isolation.
    s.push_str(
        "- You can optionally set `isolation: \"worktree\"` to run the agent in a temporary \
         git worktree, giving it an isolated copy of the repository. The worktree is \
         automatically cleaned up if the agent makes no changes; if changes are made, the \
         worktree path and branch are returned in the result.",
    );

    // Ant-only remote isolation.
    if opts.ant_build {
        s.push_str(
            "\n- You can set `isolation: \"remote\"` to run the agent in a remote CCR \
             environment. This is always a background task; you'll be notified when it \
             completes. Use for long-running tasks that need a fresh sandbox.",
        );
    }

    // Teammate / in-process teammate notices.
    if opts.is_in_process_teammate {
        s.push_str(
            "\n- The run_in_background, name, team_name, and mode parameters are not available \
             in this context. Only synchronous subagents are supported.",
        );
    } else if opts.is_teammate {
        s.push_str(
            "\n- The name, team_name, and mode parameters are not available in this context \
             \u{2014} teammates cannot spawn other teammates. Omit them to spawn a subagent.",
        );
    }

    s
}

/// "When to fork" block. Returned with the leading `\n\n` already
/// included so the caller can concatenate without a follow-up
/// blank-line check.
fn when_to_fork_section() -> String {
    "\n\n## When to fork\n\
     \n\
     Fork yourself (omit `subagent_type`) when the intermediate tool output isn't worth keeping \
     in your context. The criterion is qualitative \u{2014} \"will I need this output again\" \
     \u{2014} not task size.\n\
     - **Research**: fork open-ended questions. If research can be broken into independent \
     questions, launch parallel forks in one message. A fork beats a fresh subagent for this \
     \u{2014} it inherits context and shares your cache.\n\
     - **Implementation**: prefer to fork implementation work that requires more than a couple \
     of edits. Do research before jumping to implementation.\n\
     \n\
     Forks are cheap because they share your prompt cache. Don't set `model` on a fork \
     \u{2014} a different model can't reuse the parent's cache. Pass a short `name` (one or \
     two words, lowercase) so the user can see the fork in the teams panel and steer it \
     mid-run.\n\
     \n\
     **Don't peek.** The tool result includes an `output_file` path \u{2014} do not Read or \
     tail it unless the user explicitly asks for a progress check. You get a completion \
     notification; trust it. Reading the transcript mid-flight pulls the fork's tool noise \
     into your context, which defeats the point of forking.\n\
     \n\
     **Don't race.** After launching, you know nothing about what the fork found. Never \
     fabricate or predict fork results in any format \u{2014} not as prose, summary, or \
     structured output. The notification arrives as a user-role message in a later turn; it \
     is never something you write yourself. If the user asks a follow-up before the \
     notification lands, tell them the fork is still running \u{2014} give status, not a \
     guess.\n\
     \n\
     **Writing a fork prompt.** Since the fork inherits your context, the prompt is a \
     *directive* \u{2014} what to do, not what the situation is. Be specific about scope: \
     what's in, what's out, what another agent is handling. Don't re-explain background.\n"
        .to_owned()
}

/// "Writing the prompt" section. Wraps the fresh-agent prefix branch
/// on `fork_enabled`. Includes leading `\n\n`.
fn writing_the_prompt_section(opts: &PromptOptions) -> String {
    let fresh_prefix = if opts.fork_enabled {
        "When spawning a fresh agent (with a `subagent_type`), it starts with zero context. "
    } else {
        ""
    };
    let terse_prefix = if opts.fork_enabled {
        "For fresh agents, terse"
    } else {
        "Terse"
    };
    format!(
        "\n\n## Writing the prompt\n\
         \n\
         {fresh_prefix}Brief the agent like a smart colleague who just walked into the room \
         \u{2014} it hasn't seen this conversation, doesn't know what you've tried, doesn't \
         understand why this task matters.\n\
         - Explain what you're trying to accomplish and why.\n\
         - Describe what you've already learned or ruled out.\n\
         - Give enough context about the surrounding problem that the agent can make judgment \
         calls rather than just following a narrow instruction.\n\
         - If you need a short response, say so (\"report in under 200 words\").\n\
         - Lookups: hand over the exact command. Investigations: hand over the question \
         \u{2014} prescribed steps become dead weight when the premise is wrong.\n\
         \n\
         {terse_prefix} command-style prompts produce shallow, generic work.\n\
         \n\
         **Never delegate understanding.** Don't write \"based on your findings, fix the bug\" \
         or \"based on the research, implement it.\" Those phrases push synthesis onto the \
         agent instead of doing it yourself. Write prompts that prove you understood: include \
         file paths, line numbers, what specifically to change.\n",
    )
}

/// Examples block when fork is enabled.
fn fork_examples() -> String {
    let agent = ToolName::Agent.as_str();
    format!(
        "Example usage:\n\
         \n\
         <example>\n\
         user: \"What's left on this branch before we can ship?\"\n\
         assistant: <thinking>Forking this \u{2014} it's a survey question. I want the punch \
         list, not the git output in my context.</thinking>\n\
         {agent}({{\n\
         \u{20}\u{20}name: \"ship-audit\",\n\
         \u{20}\u{20}description: \"Branch ship-readiness audit\",\n\
         \u{20}\u{20}prompt: \"Audit what's left before this branch can ship. Check: \
         uncommitted changes, commits ahead of main, whether tests exist, whether the \
         GrowthBook gate is wired up, whether CI-relevant files changed. Report a punch list \
         \u{2014} done vs. missing. Under 200 words.\"\n\
         }})\n\
         assistant: Ship-readiness audit running.\n\
         <commentary>\n\
         Turn ends here. The coordinator knows nothing about the findings yet. What follows \
         is a SEPARATE turn \u{2014} the notification arrives from outside, as a user-role \
         message. It is not something the coordinator writes.\n\
         </commentary>\n\
         [later turn \u{2014} notification arrives as user message]\n\
         assistant: Audit's back. Three blockers: no tests for the new prompt path, GrowthBook \
         gate wired but not in build_flags.yaml, and one uncommitted file.\n\
         </example>\n\
         \n\
         <example>\n\
         user: \"so is the gate wired up or not\"\n\
         <commentary>\n\
         User asks mid-wait. The audit fork was launched to answer exactly this, and it \
         hasn't returned. The coordinator does not have this answer. Give status, not a \
         fabricated result.\n\
         </commentary>\n\
         assistant: Still waiting on the audit \u{2014} that's one of the things it's \
         checking. Should land shortly.\n\
         </example>\n\
         \n\
         <example>\n\
         user: \"Can you get a second opinion on whether this migration is safe?\"\n\
         assistant: <thinking>I'll ask the code-reviewer agent \u{2014} it won't see my \
         analysis, so it can give an independent read.</thinking>\n\
         <commentary>\n\
         A subagent_type is specified, so the agent starts fresh. It needs full context in \
         the prompt. The briefing explains what to assess and why.\n\
         </commentary>\n\
         {agent}({{\n\
         \u{20}\u{20}name: \"migration-review\",\n\
         \u{20}\u{20}description: \"Independent migration review\",\n\
         \u{20}\u{20}subagent_type: \"code-reviewer\",\n\
         \u{20}\u{20}prompt: \"Review migration 0042_user_schema.sql for safety. Context: \
         we're adding a NOT NULL column to a 50M-row table. Existing rows get a backfill \
         default. I want a second opinion on whether the backfill approach is safe under \
         concurrent writes \u{2014} I've checked locking behavior but want independent \
         verification. Report: is this safe, and if not, what specifically breaks?\"\n\
         }})\n\
         </example>\n",
    )
}

/// Examples block when fork is disabled.
fn current_examples(opts: &PromptOptions) -> String {
    let agent = ToolName::Agent.as_str();
    // Model-aware: name the file-write tool the model actually has
    // (Claude → Write, gpt-5 → apply_patch), not a hardcoded `Write`.
    let file_write = opts.file_write_tool.unwrap_or(ToolName::Write);
    let file_write = file_write.as_str();
    format!(
        "Example usage:\n\
         \n\
         <example_agent_descriptions>\n\
         \"test-runner\": use this agent after you are done writing code to run tests\n\
         \"greeting-responder\": use this agent to respond to user greetings with a friendly \
         joke\n\
         </example_agent_descriptions>\n\
         \n\
         <example>\n\
         user: \"Please write a function that checks if a number is prime\"\n\
         assistant: I'm going to use the {file_write} tool to write the following code:\n\
         <code>\n\
         function isPrime(n) {{\n\
         \u{20}\u{20}if (n <= 1) return false\n\
         \u{20}\u{20}for (let i = 2; i * i <= n; i++) {{\n\
         \u{20}\u{20}\u{20}\u{20}if (n % i === 0) return false\n\
         \u{20}\u{20}}}\n\
         \u{20}\u{20}return true\n\
         }}\n\
         </code>\n\
         <commentary>\n\
         Since a significant piece of code was written and the task was completed, now use \
         the test-runner agent to run the tests\n\
         </commentary>\n\
         assistant: Uses the {agent} tool to launch the test-runner agent\n\
         </example>\n\
         \n\
         <example>\n\
         user: \"Hello\"\n\
         <commentary>\n\
         Since the user is greeting, use the greeting-responder agent to respond with a \
         friendly joke\n\
         </commentary>\n\
         assistant: \"I'm going to use the {agent} tool to launch the greeting-responder \
         agent\"\n\
         </example>\n",
    )
}
