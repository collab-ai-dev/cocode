//! Prompt builders — concatenate verbatim text blocks (`include_str!`)
//! with run-time inputs.

use std::path::Path;

const TYPES_INDIVIDUAL: &str = include_str!("text/types_individual.md");
const TYPES_COMBINED: &str = include_str!("text/types_combined.md");
const WHAT_NOT_TO_SAVE: &str = include_str!("text/what_not_to_save.md");
const HOW_TO_SAVE_TEMPLATE: &str = include_str!("text/how_to_save.md");
const HOW_TO_SAVE_SKIP_INDEX: &str = include_str!("text/how_to_save_skipindex.md");

/// Build the personal-only "How to save memories" block with the
/// `{MAX_ENTRYPOINT_LINES}` placeholder substituted. One truth-of-record
/// for the line cap.
fn how_to_save() -> String {
    HOW_TO_SAVE_TEMPLATE.replace("{MAX_ENTRYPOINT_LINES}", &MAX_ENTRYPOINT_LINES.to_string())
}
const WHEN_TO_ACCESS: &str = include_str!("text/when_to_access.md");
const EXTRACT_GUIDANCE: &str = include_str!("text/extract.md");
const DREAM_GUIDANCE: &str = include_str!("text/dream.md");
const SESSION_TEMPLATE: &str = include_str!("text/session_template.md");
const SEARCHING_PAST_CONTEXT: &str = include_str!("text/searching_past_context.md");

/// `MAX_ENTRYPOINT_LINES` — surfaced into prompt copy via the
/// `{MAX_ENTRYPOINT_LINES}` placeholder.
const MAX_ENTRYPOINT_LINES: i32 = 200;

/// Combined-mode "must avoid sensitive data in team" addendum to the
/// shared `WHAT_NOT_TO_SAVE` block. Appended only when team memory is
/// on.
const COMBINED_TEAM_SECRET_ADDENDUM: &str = "- You MUST avoid saving sensitive data within shared team memories. For example, never save API keys or user credentials.";

/// Memory-and-other-forms-of-persistence block. Used by every
/// system-prompt variant (auto / combined / kairos) so `Plan` / `Tasks`
/// distinctions stay calibrated.
const PERSISTENCE_GUIDANCE: &str = "## Memory and other forms of persistence\n\
Memory is one of several persistence mechanisms available to you as you assist the user in a given conversation. The distinction is often that memory can be recalled in future conversations and should not be used for persisting information that is only useful within the scope of the current conversation.\n\
- When to use or update a plan instead of memory: If you are about to start a non-trivial implementation task and would like to reach alignment with the user on your approach you should use a Plan rather than saving this information to memory. Similarly, if you already have a plan within the conversation and you have changed your approach persist that change by updating the plan rather than saving a memory.\n\
- When to use or update tasks instead of memory: When you need to break your work in current conversation into discrete steps or keep track of your progress use tasks instead of saving to memory. Tasks are great for persisting information about the work that needs to be done in the current conversation, but memory should be reserved for information that will be useful in future conversations.";

/// Shared opener for all variants — "build up this memory system"
/// lines.
const BEHAVIOR_GUIDANCE: &str = "You should build up this memory system over time so that future conversations can have a complete picture of who the user is, how they'd like to collaborate with you, what behaviors to avoid or repeat, and the context behind the work the user gives you.\n\nIf the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry.";

/// Which system-prompt variant to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemPromptVariant {
    /// Personal-only memory (no team).
    Auto,
    /// Personal + team memory directories.
    Combined,
    /// KAIROS daily-log mode (assistant-mode append-only).
    Kairos,
}

/// Build the `# auto memory` system-prompt block.
///
/// `index_content` is the truncated `MEMORY.md` body (or `None` when
/// the file is missing / empty). `skip_index` — when set, the model is
/// told to write topic files only (no two-step indexing).
/// `searching_past_context` — when set, the model is shown grep
/// examples for memory and transcript search (`buildSearchingPastContextSection`).
/// `transcript_dir` is the project's session-transcript root used to
/// substitute `{TRANSCRIPT_DIR}` in the searching-past-context block;
/// `None` leaves the placeholder for the model to fill.
#[allow(clippy::too_many_arguments)]
pub fn build_system_prompt_section(
    variant: SystemPromptVariant,
    memory_dir: &Path,
    team_dir: Option<&Path>,
    index_content: Option<&str>,
    team_index_content: Option<&str>,
    skip_index: bool,
    searching_past_context: bool,
    transcript_dir: Option<&Path>,
    extra_guidelines: Option<&str>,
) -> String {
    if matches!(variant, SystemPromptVariant::Kairos) {
        return build_kairos_prompt(
            memory_dir,
            skip_index,
            searching_past_context,
            transcript_dir,
        );
    }

    let combined = matches!(variant, SystemPromptVariant::Combined);
    let mut sections = Vec::new();
    sections.push("# auto memory".to_string());

    let dir_blurb = if let Some(team) = team_dir
        && combined
    {
        format!(
            "You have a persistent, file-based memory system with two directories: a private directory at `{}` and a shared team directory at `{}`. Both directories already exist — write to them directly with the Write tool (do not run mkdir or check for their existence).",
            memory_dir.display(),
            team.display(),
        )
    } else {
        format!(
            "You have a persistent, file-based memory system at `{}`. This directory already exists — write to it directly with the Write tool (do not run mkdir or check for its existence).",
            memory_dir.display(),
        )
    };
    sections.push(dir_blurb);

    sections.push(BEHAVIOR_GUIDANCE.to_string());

    if combined && let Some(team) = team_dir {
        sections.push(format!(
            "## Memory scope\n\nThere are two scope levels:\n\n- private: memories that are private between you and the current user. They persist across conversations with only this specific user and are stored at the root `{}`.\n- team: memories that are shared with and contributed by all of the users who work within this project directory. Team memories are synced at the beginning of every session and they are stored at `{}`.",
            memory_dir.display(),
            team.display(),
        ));
    }

    sections.push(if combined {
        TYPES_COMBINED.to_string()
    } else {
        TYPES_INDIVIDUAL.to_string()
    });

    let mut what_not = WHAT_NOT_TO_SAVE.to_string();
    if combined {
        // Appends the secrets bullet as part of the WHAT_NOT_TO_SAVE
        // block in combined mode.
        what_not.push('\n');
        what_not.push_str(COMBINED_TEAM_SECRET_ADDENDUM);
    }
    sections.push(what_not);

    sections.push(if skip_index {
        if combined {
            combined_how_to_save_skip_index()
        } else {
            HOW_TO_SAVE_SKIP_INDEX.to_string()
        }
    } else if combined {
        combined_how_to_save()
    } else {
        how_to_save()
    });

    sections.push(if combined {
        combined_when_to_access()
    } else {
        WHEN_TO_ACCESS.to_string()
    });

    sections.push(PERSISTENCE_GUIDANCE.to_string());

    if let Some(guidance) = extra_guidelines
        && !guidance.trim().is_empty()
    {
        sections.push(guidance.to_string());
    }

    if searching_past_context {
        sections.push(render_searching_past_context(memory_dir, transcript_dir));
    }

    if let Some(body) = index_content
        && !body.trim().is_empty()
    {
        sections.push("## MEMORY.md".to_string());
        sections.push(body.to_string());
    } else {
        sections.push(
            "## MEMORY.md\n\nYour MEMORY.md is currently empty. When you save new memories, they will appear here."
                .to_string(),
        );
    }

    if combined {
        if let Some(team_body) = team_index_content
            && !team_body.trim().is_empty()
        {
            sections.push("## Team MEMORY.md".to_string());
            sections.push(team_body.to_string());
        } else {
            sections.push(
                "## Team MEMORY.md\n\nYour team MEMORY.md is currently empty. When you save new team memories, they will appear here."
                    .to_string(),
            );
        }
    }

    sections.join("\n\n")
}

/// Build the KAIROS daily-log prompt — used when `kairos_mode` is set.
///
/// Honors `skip_index` (drops the `## MEMORY.md` orientation block)
/// and appends the searching-past-context block when enabled.
pub fn build_kairos_prompt(
    memory_dir: &Path,
    skip_index: bool,
    searching_past_context: bool,
    transcript_dir: Option<&Path>,
) -> String {
    let log_pattern = memory_dir
        .join("logs")
        .join("YYYY")
        .join("MM")
        .join("YYYY-MM-DD.md");
    let mem = memory_dir.display();
    let log = log_pattern.display();

    let mut sections = Vec::new();
    sections.push("# auto memory".to_string());
    sections.push(format!(
        "You have a persistent, file-based memory system found at: `{mem}`"
    ));
    sections.push(format!(
        "This session is long-lived. As you work, record anything worth remembering by **appending** to today's daily log file:\n\n`{log}`\n\nSubstitute today's date (from `currentDate` in your context) for `YYYY-MM-DD`. When the date rolls over mid-session, start appending to the new day's file."
    ));
    sections.push("Write each entry as a short timestamped bullet. Create the file (and parent directories) on first write if it does not exist. Do not rewrite or reorganize the log — it is append-only. A separate nightly process distills these logs into `MEMORY.md` and topic files.".to_string());
    sections.push("## What to log\n- User corrections and preferences (\"use bun, not npm\"; \"stop summarizing diffs\")\n- Facts about the user, their role, or their goals\n- Project context that is not derivable from the code (deadlines, incidents, decisions and their rationale)\n- Pointers to external systems (dashboards, Linear projects, Slack channels)\n- Anything the user explicitly asks you to remember".to_string());
    sections.push(WHAT_NOT_TO_SAVE.to_string());

    if !skip_index {
        sections.push(
            "## MEMORY.md\n`MEMORY.md` is the distilled index (maintained nightly from your logs) and is loaded into your context automatically. Read it for orientation, but do not edit it directly — record new information in today's log instead."
                .to_string(),
        );
    }

    if searching_past_context {
        sections.push(render_searching_past_context(memory_dir, transcript_dir));
    }

    sections.join("\n\n")
}

fn render_searching_past_context(memory_dir: &Path, transcript_dir: Option<&Path>) -> String {
    let mem = memory_dir.display().to_string();
    let trans = transcript_dir
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<your sessions directory>".to_string());
    SEARCHING_PAST_CONTEXT
        .replace("{MEMORY_DIR}", &mem)
        .replace("{TRANSCRIPT_DIR}", &trans)
}

/// Combined-variant "How to save" copy.
fn combined_how_to_save() -> String {
    let example = memory_frontmatter_example();
    format!(
        "## How to save memories\n\nSaving a memory is a two-step process:\n\n**Step 1** — write the memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:\n\n{example}\n\n**Step 2** — add a pointer to that file in the same directory's `MEMORY.md`. Each directory (private and team) has its own `MEMORY.md` index — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. They have no frontmatter. Never write memory content directly into a `MEMORY.md`.\n\n- Both `MEMORY.md` indexes are loaded into your conversation context — lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep them concise\n- Keep the name, description, and type fields in memory files up-to-date with the content\n- Organize memory semantically by topic, not chronologically\n- Update or remove memories that turn out to be wrong or outdated\n- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one."
    )
}

fn combined_how_to_save_skip_index() -> String {
    let example = memory_frontmatter_example();
    format!(
        "## How to save memories\n\nWrite each memory to its own file in the chosen directory (private or team, per the type's scope guidance) using this frontmatter format:\n\n{example}\n\n- Keep the name, description, and type fields in memory files up-to-date with the content\n- Organize memory semantically by topic, not chronologically\n- Update or remove memories that turn out to be wrong or outdated\n- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one."
    )
}

fn combined_when_to_access() -> String {
    "## When to access memories\n- When memories (personal or team) seem relevant, or the user references prior work with them or others in their organization.\n- You MUST access memory when the user explicitly asks you to check, recall, or remember.\n- If the user says to *ignore* or *not use* memory: proceed as if MEMORY.md were empty. Do not apply remembered facts, cite, compare against, or mention memory content.\n- Memory records can become stale over time. Use memory as context for what was true at a given point in time. Before answering the user or building assumptions based solely on information in memory records, verify that the memory is still correct and up-to-date by reading the current state of the files or resources. If a recalled memory conflicts with current information, trust what you observe now — and update or remove the stale memory rather than acting on it.\n\n## Before recommending from memory\n\nA memory that names a specific function, file, or flag is a claim that it existed *when the memory was written*. It may have been renamed, removed, or never merged. Before recommending it:\n\n- If the memory names a file path: check the file exists.\n- If the memory names a function or flag: grep for it.\n- If the user is about to act on your recommendation (not just asking about history), verify first.\n\n\"The memory says X exists\" is not the same as \"X exists now.\"\n\nA memory that summarizes repo state (activity logs, architecture snapshots) is frozen in time. If the user asks about *recent* or *current* state, prefer `git log` or reading the code over recalling the snapshot.".to_string()
}

fn memory_frontmatter_example() -> String {
    "```markdown\n---\nname: {{memory name}}\ndescription: {{one-line description — used to decide relevance in future conversations, so be specific}}\ntype: {{user, feedback, project, reference}}\n---\n\n{{memory content — for feedback/project types, structure as: rule/fact, then **Why:** and **How to apply:** lines}}\n```".to_string()
}

/// Build the extraction-agent system prompt.
///
/// The `manifest` block is rendered by `scan::format_memory_manifest`
/// and pre-injected so the agent doesn't spend a turn on `ls`.
/// `combined` switches to the team-aware copy.
pub fn build_extract_prompt(
    message_count: i32,
    manifest: &str,
    skip_index: bool,
    combined: bool,
) -> String {
    let how_to = if skip_index {
        if combined {
            combined_how_to_save_skip_index()
        } else {
            HOW_TO_SAVE_SKIP_INDEX.to_string()
        }
    } else if combined {
        combined_how_to_save()
    } else {
        how_to_save()
    };

    let mut what_not = WHAT_NOT_TO_SAVE.to_string();
    if combined {
        what_not.push('\n');
        what_not.push_str(COMBINED_TEAM_SECRET_ADDENDUM);
    }

    let types = if combined {
        TYPES_COMBINED
    } else {
        TYPES_INDIVIDUAL
    };
    let count = message_count;
    // Substitute the message-count placeholder — the count appears
    // twice (opener + budget reminder).
    let guidance = EXTRACT_GUIDANCE.replace("{MESSAGE_COUNT}", &count.to_string());
    // Wrap the manifest with the `## Existing memory files` header +
    // the "Check this list before writing" trailing nudge, only when
    // there's actual content. An empty manifest drops the section
    // entirely.
    let manifest_block = if manifest.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n\n## Existing memory files\n\n{manifest}\n\nCheck this list before writing — update an existing file rather than creating a duplicate."
        )
    };
    format!("{guidance}{manifest_block}\n\n{types}\n\n{what_not}\n\n{how_to}")
}

/// Build the auto-dream consolidation agent prompt.
///
/// 4-phase structure: Orient / Gather / Consolidate / Prune.
/// Placeholders `{MEMORY_ROOT}` and `{TRANSCRIPT_DIR}` in the verbatim
/// template are substituted at build time so the model sees concrete paths.
///
/// The `## Additional context` block carries the bash read-only
/// constraint reminder + sessions-since-last list. The constraint
/// reminder is appended even when `sessions_since_last` is empty so a
/// forced /dream call still gets the heads-up.
pub fn build_dream_prompt(
    memory_dir: &Path,
    transcript_dir: &Path,
    sessions_since_last: &[String],
) -> String {
    let mem = memory_dir.display().to_string();
    let trans = transcript_dir.display().to_string();
    let body = DREAM_GUIDANCE
        .replace("{MEMORY_ROOT}", &mem)
        .replace("{TRANSCRIPT_DIR}", &trans);
    let bash_constraint = "**Tool constraints for this run:** Bash is restricted to read-only commands (`ls`, `find`, `grep`, `cat`, `stat`, `wc`, `head`, `tail`, and similar). Anything that writes, redirects to a file, or modifies state will be denied. Plan your exploration with this in mind — no need to probe.";
    if sessions_since_last.is_empty() {
        format!("{body}\n\n## Additional context\n\n{bash_constraint}")
    } else {
        let count = sessions_since_last.len();
        let list = sessions_since_last
            .iter()
            .map(|s| format!("- {s}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "{body}\n\n## Additional context\n\n{bash_constraint}\n\nSessions since last consolidation ({count}):\n{list}"
        )
    }
}

/// The verbatim 9-section session-memory template.
pub fn build_session_memory_template() -> &'static str {
    SESSION_TEMPLATE
}

/// Rough token estimator — `Math.round(len / 4)`, rounded half-up
/// (not floor). The difference is at most one token but matters when
/// a section is right at the budget boundary.
pub fn rough_token_estimate(s: &str) -> i64 {
    let len = s.len() as i64;
    // For len ≥ 0, `(len + 2) / 4` (integer division) is equivalent
    // to `Math.round(len / 4)` for positive halves.
    (len + 2) / 4
}

/// Walk a 9-section session-memory document and return
/// `(section_header, token_estimate)` for every `# Section`. Used by
/// [`generate_section_reminders`] to decide which sections need
/// condensing.
pub fn analyze_section_sizes(content: &str) -> Vec<(String, i64)> {
    let mut sections: Vec<(String, i64)> = Vec::new();
    let mut current_header: String = String::new();
    let mut current_body: Vec<&str> = Vec::new();
    for line in content.lines() {
        if line.starts_with("# ") {
            if !current_header.is_empty() && !current_body.is_empty() {
                let body = current_body.join("\n");
                sections.push((current_header.clone(), rough_token_estimate(body.trim())));
            }
            current_header = line.to_string();
            current_body.clear();
        } else {
            current_body.push(line);
        }
    }
    if !current_header.is_empty() && !current_body.is_empty() {
        let body = current_body.join("\n");
        sections.push((current_header, rough_token_estimate(body.trim())));
    }
    sections
}

/// Build the per-section + total-budget reminder block appended to
/// `build_session_memory_update_prompt` — without this the model has
/// no signal that sections are over-budget and will keep growing the
/// file until compact-time truncation fires.
///
/// Returns an empty string when nothing's over-budget.
pub fn generate_section_reminders(
    section_sizes: &[(String, i64)],
    total_tokens: i64,
    max_section_tokens: i64,
    max_total_tokens: i64,
) -> String {
    let over_budget = total_tokens > max_total_tokens;
    let mut oversized: Vec<&(String, i64)> = section_sizes
        .iter()
        .filter(|(_, t)| *t > max_section_tokens)
        .collect();
    oversized.sort_by(|a, b| b.1.cmp(&a.1));

    if oversized.is_empty() && !over_budget {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();
    if over_budget {
        parts.push(format!(
            "\n\nCRITICAL: The session memory file is currently ~{total_tokens} tokens, which exceeds the maximum of {max_total_tokens} tokens. You MUST condense the file to fit within this budget. Aggressively shorten oversized sections by removing less important details, merging related items, and summarizing older entries. Prioritize keeping \"Current State\" and \"Errors & Corrections\" accurate and detailed."
        ));
    }
    if !oversized.is_empty() {
        let lines: Vec<String> = oversized
            .iter()
            .map(|(s, t)| format!("- \"{s}\" is ~{t} tokens (limit: {max_section_tokens})"))
            .collect();
        let prefix = if over_budget {
            "Oversized sections to condense"
        } else {
            "IMPORTANT: The following sections exceed the per-section limit and MUST be condensed"
        };
        parts.push(format!("\n\n{prefix}:\n{}", lines.join("\n")));
    }
    parts.join("")
}

/// Substitute `{{var}}` placeholders in a template — single-pass
/// replacement so user content containing `{{varName}}` can't trigger
/// second-round substitution. Variables not present in the map are
/// left as-is.
pub fn substitute_variables(template: &str, variables: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len()
            && bytes[i] == b'{'
            && bytes[i + 1] == b'{'
            && let Some(rel) = template[i + 2..].find("}}")
        {
            let key = &template[i + 2..i + 2 + rel];
            if key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && let Some((_, val)) = variables.iter().find(|(k, _)| *k == key)
            {
                out.push_str(val);
                i += 2 + rel + 2;
                continue;
            }
        }
        // Push one char. The loop guard keeps `i < bytes.len()` and
        // `i` always lands on a char boundary (we only advance by
        // `len_utf8()`), so `chars().next()` is always `Some` in
        // practice. The `if let` is just to satisfy
        // `clippy::expect_used` without panicking on the unreachable
        // `None` arm.
        if let Some(c) = template[i..].chars().next() {
            out.push(c);
            i += c.len_utf8();
        } else {
            break;
        }
    }
    out
}

/// Build the session-memory update prompt.
///
/// Emphasizes structure preservation: the model must `Edit` only and
/// never delete/modify section headers or the italic
/// `_section descriptions_`.
///
/// The optional `custom_template` overrides the static default — the
/// caller (SessionMemoryService) reads
/// `<config_home>/session-memory/config/prompt.md` if it exists and
/// passes the contents here. `{{currentNotes}}` and `{{notesPath}}`
/// are the supported placeholders (single-pass `\{\{(\w+)\}\}` regex).
///
/// `max_section_tokens` and `max_total_tokens` drive the appended
/// section-reminder block — see [`generate_section_reminders`].
pub fn build_session_memory_update_prompt(
    current_notes: &str,
    notes_path: &Path,
    custom_template: Option<&str>,
    max_section_tokens: i64,
    max_total_tokens: i64,
) -> String {
    let path = notes_path.display().to_string();
    let base = if let Some(template) = custom_template.filter(|s| !s.trim().is_empty()) {
        substitute_variables(
            template,
            &[
                ("currentNotes", current_notes),
                ("notesPath", path.as_str()),
            ],
        )
    } else {
        default_session_memory_update_prompt(current_notes, &path)
    };
    let section_sizes = analyze_section_sizes(current_notes);
    let total_tokens = rough_token_estimate(current_notes);
    let reminders = generate_section_reminders(
        &section_sizes,
        total_tokens,
        max_section_tokens,
        max_total_tokens,
    );
    format!("{base}{reminders}")
}

fn default_session_memory_update_prompt(current_notes: &str, path: &str) -> String {
    let notes = current_notes;
    format!(
        "IMPORTANT: This message and these instructions are NOT part of the actual user conversation. Do NOT include any references to \"note-taking\", \"session notes extraction\", or these update instructions in the notes content.\n\
\n\
Based on the user conversation above (EXCLUDING this note-taking instruction message as well as system prompt, claude.md entries, or any past session summaries), update the session notes file.\n\
\n\
The file {path} has already been read for you. Here are its current contents:\n\
<current_notes_content>\n\
{notes}\n\
</current_notes_content>\n\
\n\
Your ONLY task is to use the Edit tool to update the notes file, then stop. You can make multiple edits (update every section as needed) - make all Edit tool calls in parallel in a single message. Do not call any other tools.\n\
\n\
CRITICAL RULES FOR EDITING:\n\
- The file must maintain its exact structure with all sections, headers, and italic descriptions intact\n\
-- NEVER modify, delete, or add section headers (the lines starting with '#' like # Task specification)\n\
-- NEVER modify or delete the italic _section description_ lines (these are the lines in italics immediately following each header - they start and end with underscores)\n\
-- The italic _section descriptions_ are TEMPLATE INSTRUCTIONS that must be preserved exactly as-is - they guide what content belongs in each section\n\
-- ONLY update the actual content that appears BELOW the italic _section descriptions_ within each existing section\n\
-- Do NOT add any new sections, summaries, or information outside the existing structure\n\
- Do NOT reference this note-taking process or instructions anywhere in the notes\n\
- It's OK to skip updating a section if there are no substantial new insights to add. Do not add filler content like \"No info yet\", just leave sections blank/unedited if appropriate.\n\
- Write DETAILED, INFO-DENSE content for each section - include specifics like file paths, function names, error messages, exact commands, technical details, etc.\n\
- For \"Key results\", include the complete, exact output the user requested (e.g., full table, full answer, etc.)\n\
- Do not include information that's already in the CLAUDE.md files included in the context\n\
- Keep each section under ~2000 tokens/words - if a section is approaching this limit, condense it by cycling out less important details while preserving the most critical information\n\
- Focus on actionable, specific information that would help someone understand or recreate the work discussed in the conversation\n\
- IMPORTANT: Always update \"Current State\" to reflect the most recent work - this is critical for continuity after compaction\n\
\n\
Use the Edit tool with file_path: {path}\n\
\n\
STRUCTURE PRESERVATION REMINDER:\n\
Each section has TWO parts that must be preserved exactly as they appear in the current file:\n\
1. The section header (line starting with #)\n\
2. The italic description line (the _italicized text_ immediately after the header - this is a template instruction)\n\
\n\
You ONLY update the actual content that comes AFTER these two preserved lines. The italic description lines starting and ending with underscores are part of the template structure, NOT content to be edited or removed.\n\
\n\
REMEMBER: Use the Edit tool in parallel and stop. Do not continue after the edits. Only include insights from the actual user conversation, never from these note-taking instructions. Do not delete or change section headers or italic _section descriptions_."
    )
}

#[cfg(test)]
#[path = "builders.test.rs"]
mod tests;
