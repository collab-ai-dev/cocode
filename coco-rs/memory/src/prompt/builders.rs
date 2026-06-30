//! Prompt builders — compose inlined prompt text with run-time inputs.

use std::path::Path;

use coco_config::MemoryStore;
use coco_config::StoreMode;
use coco_config::StoreScope;
use coco_types::ToolName;

const TYPES_INDIVIDUAL: &str = r#"## Types of memory

There are several discrete types of memory that you can store in your memory system:

<types>
<type>
    <name>user</name>
    <description>Contain information about the user's role, goals, responsibilities, and knowledge. Great user memories help you tailor your future behavior to the user's preferences and perspective. Your goal in reading and writing these memories is to build up an understanding of who the user is and how you can be most helpful to them specifically. For example, you should collaborate with a senior software engineer differently than a student who is coding for the very first time. Keep in mind, that the aim here is to be helpful to the user. Avoid writing memories about the user that could be viewed as a negative judgement or that are not relevant to the work you're trying to accomplish together.</description>
    <when_to_save>When you learn any details about the user's role, preferences, responsibilities, or knowledge</when_to_save>
    <how_to_use>When your work should be informed by the user's profile or perspective. For example, if the user is asking you to explain a part of the code, you should answer that question in a way that is tailored to the specific details that they will find most valuable or that helps them build their mental model in relation to domain knowledge they already have.</how_to_use>
    <examples>
    user: I'm a data scientist investigating what logging we have in place
    assistant: [saves user memory: user is a data scientist, currently focused on observability/logging]

    user: I've been writing Go for ten years but this is my first time touching the React side of this repo
    assistant: [saves user memory: deep Go expertise, new to React and this project's frontend — frame frontend explanations in terms of backend analogues]
    </examples>
</type>
<type>
    <name>feedback</name>
    <description>Guidance the user has given you about how to approach work — both what to avoid and what to keep doing. These are a very important type of memory to read and write as they allow you to remain coherent and responsive to the way you should approach work in the project. Record from failure AND success: if you only save corrections, you will avoid past mistakes but drift away from approaches the user has already validated, and may grow overly cautious.</description>
    <when_to_save>Any time the user corrects your approach ("no not that", "don't", "stop doing X") OR confirms a non-obvious approach worked ("yes exactly", "perfect, keep doing that", accepting an unusual choice without pushback). Corrections are easy to notice; confirmations are quieter — watch for them. In both cases, save what is applicable to future conversations, especially if surprising or not obvious from the code. Include *why* so you can judge edge cases later.</when_to_save>
    <how_to_use>Let these memories guide your behavior so that the user does not need to offer the same guidance twice.</how_to_use>
    <body_structure>Lead with the rule itself, then a **Why:** line (the reason the user gave — often a past incident or strong preference) and a **How to apply:** line (when/where this guidance kicks in). Knowing *why* lets you judge edge cases instead of blindly following the rule.</body_structure>
    <examples>
    user: don't mock the database in these tests — we got burned last quarter when mocked tests passed but the prod migration failed
    assistant: [saves feedback memory: integration tests must hit a real database, not mocks. Reason: prior incident where mock/prod divergence masked a broken migration]

    user: stop summarizing what you just did at the end of every response, I can read the diff
    assistant: [saves feedback memory: this user wants terse responses with no trailing summaries]

    user: yeah the single bundled PR was the right call here, splitting this one would've just been churn
    assistant: [saves feedback memory: for refactors in this area, user prefers one bundled PR over many small ones. Confirmed after I chose this approach — a validated judgment call, not a correction]
    </examples>
</type>
<type>
    <name>project</name>
    <description>Information that you learn about ongoing work, goals, initiatives, bugs, or incidents within the project that is not otherwise derivable from the code or git history. Project memories help you understand the broader context and motivation behind the work the user is doing within this working directory.</description>
    <when_to_save>When you learn who is doing what, why, or by when. These states change relatively quickly so try to keep your understanding of this up to date. Always convert relative dates in user messages to absolute dates when saving (e.g., "Thursday" → "2026-03-05"), so the memory remains interpretable after time passes.</when_to_save>
    <how_to_use>Use these memories to more fully understand the details and nuance behind the user's request and make better informed suggestions.</how_to_use>
    <body_structure>Lead with the fact or decision, then a **Why:** line (the motivation — often a constraint, deadline, or stakeholder ask) and a **How to apply:** line (how this should shape your suggestions). Project memories decay fast, so the why helps future-you judge whether the memory is still load-bearing.</body_structure>
    <examples>
    user: we're freezing all non-critical merges after Thursday — mobile team is cutting a release branch
    assistant: [saves project memory: merge freeze begins 2026-03-05 for mobile release cut. Flag any non-critical PR work scheduled after that date]

    user: the reason we're ripping out the old auth middleware is that legal flagged it for storing session tokens in a way that doesn't meet the new compliance requirements
    assistant: [saves project memory: auth middleware rewrite is driven by legal/compliance requirements around session token storage, not tech-debt cleanup — scope decisions should favor compliance over ergonomics]
    </examples>
</type>
<type>
    <name>reference</name>
    <description>Stores pointers to where information can be found in external systems. These memories allow you to remember where to look to find up-to-date information outside of the project directory.</description>
    <when_to_save>When you learn about resources in external systems and their purpose. For example, that bugs are tracked in a specific project in Linear or that feedback can be found in a specific Slack channel.</when_to_save>
    <how_to_use>When the user references an external system or information that may be in an external system.</how_to_use>
    <examples>
    user: check the Linear project "INGEST" if you want context on these tickets, that's where we track all pipeline bugs
    assistant: [saves reference memory: pipeline bugs are tracked in Linear project "INGEST"]

    user: the Grafana board at grafana.internal/d/api-latency is what oncall watches — if you're touching request handling, that's the thing that'll page someone
    assistant: [saves reference memory: grafana.internal/d/api-latency is the oncall latency dashboard — check it when editing request-path code]
    </examples>
</type>
</types>
"#;

const TYPES_COMBINED: &str = r#"## Types of memory

There are several discrete types of memory that you can store in your memory system. Each type below declares a <scope> of `private`, `team`, or guidance for choosing between the two.

<types>
<type>
    <name>user</name>
    <scope>always private</scope>
    <description>Contain information about the user's role, goals, responsibilities, and knowledge. Great user memories help you tailor your future behavior to the user's preferences and perspective. Your goal in reading and writing these memories is to build up an understanding of who the user is and how you can be most helpful to them specifically. For example, you should collaborate with a senior software engineer differently than a student who is coding for the very first time. Keep in mind, that the aim here is to be helpful to the user. Avoid writing memories about the user that could be viewed as a negative judgement or that are not relevant to the work you're trying to accomplish together.</description>
    <when_to_save>When you learn any details about the user's role, preferences, responsibilities, or knowledge</when_to_save>
    <how_to_use>When your work should be informed by the user's profile or perspective. For example, if the user is asking you to explain a part of the code, you should answer that question in a way that is tailored to the specific details that they will find most valuable or that helps them build their mental model in relation to domain knowledge they already have.</how_to_use>
    <examples>
    user: I'm a data scientist investigating what logging we have in place
    assistant: [saves private user memory: user is a data scientist, currently focused on observability/logging]

    user: I've been writing Go for ten years but this is my first time touching the React side of this repo
    assistant: [saves private user memory: deep Go expertise, new to React and this project's frontend — frame frontend explanations in terms of backend analogues]
    </examples>
</type>
<type>
    <name>feedback</name>
    <scope>default to private. Save as team only when the guidance is clearly a project-wide convention that every contributor should follow (e.g., a testing policy, a build invariant), not a personal style preference.</scope>
    <description>Guidance the user has given you about how to approach work — both what to avoid and what to keep doing. These are a very important type of memory to read and write as they allow you to remain coherent and responsive to the way you should approach work in the project. Record from failure AND success: if you only save corrections, you will avoid past mistakes but drift away from approaches the user has already validated, and may grow overly cautious. Before saving a private feedback memory, check that it doesn't contradict a team feedback memory — if it does, either don't save it or note the override explicitly.</description>
    <when_to_save>Any time the user corrects your approach ("no not that", "don't", "stop doing X") OR confirms a non-obvious approach worked ("yes exactly", "perfect, keep doing that", accepting an unusual choice without pushback). Corrections are easy to notice; confirmations are quieter — watch for them. In both cases, save what is applicable to future conversations, especially if surprising or not obvious from the code. Include *why* so you can judge edge cases later.</when_to_save>
    <how_to_use>Let these memories guide your behavior so that the user and other users in the project do not need to offer the same guidance twice.</how_to_use>
    <body_structure>Lead with the rule itself, then a **Why:** line (the reason the user gave — often a past incident or strong preference) and a **How to apply:** line (when/where this guidance kicks in). Knowing *why* lets you judge edge cases instead of blindly following the rule.</body_structure>
    <examples>
    user: don't mock the database in these tests — we got burned last quarter when mocked tests passed but the prod migration failed
    assistant: [saves team feedback memory: integration tests must hit a real database, not mocks. Reason: prior incident where mock/prod divergence masked a broken migration. Team scope: this is a project testing policy, not a personal preference]

    user: stop summarizing what you just did at the end of every response, I can read the diff
    assistant: [saves private feedback memory: this user wants terse responses with no trailing summaries. Private because it's a communication preference, not a project convention]

    user: yeah the single bundled PR was the right call here, splitting this one would've just been churn
    assistant: [saves private feedback memory: for refactors in this area, user prefers one bundled PR over many small ones. Confirmed after I chose this approach — a validated judgment call, not a correction]
    </examples>
</type>
<type>
    <name>project</name>
    <scope>private or team, but strongly bias toward team</scope>
    <description>Information that you learn about ongoing work, goals, initiatives, bugs, or incidents within the project that is not otherwise derivable from the code or git history. Project memories help you understand the broader context and motivation behind the work users are working on within this working directory.</description>
    <when_to_save>When you learn who is doing what, why, or by when. These states change relatively quickly so try to keep your understanding of this up to date. Always convert relative dates in user messages to absolute dates when saving (e.g., "Thursday" → "2026-03-05"), so the memory remains interpretable after time passes.</when_to_save>
    <how_to_use>Use these memories to more fully understand the details and nuance behind the user's request, anticipate coordination issues across users, make better informed suggestions.</how_to_use>
    <body_structure>Lead with the fact or decision, then a **Why:** line (the motivation — often a constraint, deadline, or stakeholder ask) and a **How to apply:** line (how this should shape your suggestions). Project memories decay fast, so the why helps future-you judge whether the memory is still load-bearing.</body_structure>
    <examples>
    user: we're freezing all non-critical merges after Thursday — mobile team is cutting a release branch
    assistant: [saves team project memory: merge freeze begins 2026-03-05 for mobile release cut. Flag any non-critical PR work scheduled after that date]

    user: the reason we're ripping out the old auth middleware is that legal flagged it for storing session tokens in a way that doesn't meet the new compliance requirements
    assistant: [saves team project memory: auth middleware rewrite is driven by legal/compliance requirements around session token storage, not tech-debt cleanup — scope decisions should favor compliance over ergonomics]
    </examples>
</type>
<type>
    <name>reference</name>
    <scope>usually team</scope>
    <description>Stores pointers to where information can be found in external systems. These memories allow you to remember where to look to find up-to-date information outside of the project directory.</description>
    <when_to_save>When you learn about resources in external systems and their purpose. For example, that bugs are tracked in a specific project in Linear or that feedback can be found in a specific Slack channel.</when_to_save>
    <how_to_use>When the user references an external system or information that may be in an external system.</how_to_use>
    <examples>
    user: check the Linear project "INGEST" if you want context on these tickets, that's where we track all pipeline bugs
    assistant: [saves team reference memory: pipeline bugs are tracked in Linear project "INGEST"]

    user: the Grafana board at grafana.internal/d/api-latency is what oncall watches — if you're touching request handling, that's the thing that'll page someone
    assistant: [saves team reference memory: grafana.internal/d/api-latency is the oncall latency dashboard — check it when editing request-path code]
    </examples>
</type>
</types>
"#;

const WHAT_NOT_TO_SAVE: &str = r#"## What NOT to save in memory

- Code patterns, conventions, architecture, file paths, or project structure — these can be derived by reading the current project state.
- Git history, recent changes, or who-changed-what — `git log` / `git blame` are authoritative.
- Debugging solutions or fix recipes — the fix is in the code; the commit message has the context.
- Anything already documented in CLAUDE.md files.
- Ephemeral task details: in-progress work, temporary state, current conversation context.

These exclusions apply even when the user explicitly asks you to save. If they ask you to save a PR list or activity summary, ask what was *surprising* or *non-obvious* about it — that is the part worth keeping.
"#;

const HOW_TO_SAVE_TEMPLATE: &str = r#"## How to save memories

Saving a memory is a two-step process:

**Step 1** — write the memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:

```markdown
---
name: {{memory name}}
description: {{one-line description — used to decide relevance in future conversations, so be specific}}
type: {{user, feedback, project, reference}}
---

{{memory content — for feedback/project types, structure as: rule/fact, then **Why:** and **How to apply:** lines}}
```

**Step 2** — add a pointer to that file in `MEMORY.md`. `MEMORY.md` is an index, not a memory — each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. It has no frontmatter. Never write memory content directly into `MEMORY.md`.

- `MEMORY.md` is always loaded into your conversation context — lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep the index concise
- Keep the name, description, and type fields in memory files up-to-date with the content
- Organize memory semantically by topic, not chronologically
- Update or remove memories that turn out to be wrong or outdated
- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.
"#;

const HOW_TO_SAVE_SKIP_INDEX: &str = r#"## How to save memories

Save each memory to its own file (e.g., `user_role.md`, `feedback_testing.md`) using this frontmatter format:

```markdown
---
name: {{memory name}}
description: {{one-line description — used to decide relevance in future conversations, so be specific}}
type: {{user, feedback, project, reference}}
---

{{memory content — for feedback/project types, structure as: rule/fact, then **Why:** and **How to apply:** lines}}
```

- Keep the name, description, and type fields in memory files up-to-date with the content
- Organize memory semantically by topic, not chronologically
- Update or remove memories that turn out to be wrong or outdated
- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.
"#;

/// Build the personal-only "How to save memories" block with the
/// `{MAX_ENTRYPOINT_LINES}` placeholder substituted. One truth-of-record
/// for the line cap.
fn how_to_save() -> String {
    HOW_TO_SAVE_TEMPLATE.replace("{MAX_ENTRYPOINT_LINES}", &MAX_ENTRYPOINT_LINES.to_string())
}
const WHEN_TO_ACCESS: &str = r#"## When to access memories
- When memories seem relevant, or the user references prior-conversation work.
- You MUST access memory when the user explicitly asks you to check, recall, or remember.
- If the user says to *ignore* or *not use* memory: proceed as if MEMORY.md were empty. Do not apply remembered facts, cite, compare against, or mention memory content.
- Memory records can become stale over time. Use memory as context for what was true at a given point in time. Before answering the user or building assumptions based solely on information in memory records, verify that the memory is still correct and up-to-date by reading the current state of the files or resources. If a recalled memory conflicts with current information, trust what you observe now — and update or remove the stale memory rather than acting on it.

## Before recommending from memory

A memory that names a specific function, file, or flag is a claim that it existed *when the memory was written*. It may have been renamed, removed, or never merged. Before recommending it:

- If the memory names a file path: check the file exists.
- If the memory names a function or flag: grep for it.
- If the user is about to act on your recommendation (not just asking about history), verify first.

"The memory says X exists" is not the same as "X exists now."

A memory that summarizes repo state (activity logs, architecture snapshots) is frozen in time. If the user asks about *recent* or *current* state, prefer `git log` or reading the code over recalling the snapshot.
"#;

const DREAM_GUIDANCE: &str = r#"# Dream: Memory Consolidation

You are performing a dream — a reflective pass over your memory files. Synthesize what you've learned recently into durable, well-organized memories so that future sessions can orient quickly.

Memory directory: `{MEMORY_ROOT}`
This directory already exists — {WRITE_DIRECTLY} (do not run mkdir or check for its existence).

Session transcripts: `{TRANSCRIPT_DIR}` (large JSONL files — grep narrowly, don't read whole files)

---

## Phase 1 — Orient

- `ls` the memory directory to see what already exists
- Read `MEMORY.md` to understand the current index
- Skim existing topic files so you improve them rather than creating duplicates
- `ls -R logs/` — recent activity logs (one file per session under `YYYY/MM/DD/`). If a `sessions/` subdirectory also exists, review recent entries there too

## Phase 2 — Gather recent signal

Look for new information worth persisting. Sources in rough priority order:

1. **Session logs** (`logs/YYYY/MM/DD/<id>-<title>.md`) — the append-only activity stream, one file per session. Read the most recent 1–3 days of sessions (the filename title tells you what each was about); each line is prefix-coded (`>` user, `<` assistant, `.` tool call)
2. **Existing memories that drifted** — facts that contradict something you see in the codebase now
3. **Transcript search** — if you need specific context (e.g., "what was the error message from yesterday's build failure?"), grep the JSONL transcripts for narrow terms:
   `grep -rn "<narrow term>" {TRANSCRIPT_DIR}/ --include="*.jsonl" | tail -50`

Don't exhaustively read transcripts. Look only for things you already suspect matter.

## Phase 3 — Consolidate

For each thing worth remembering, write or update a memory file at the top level of the memory directory. Use the memory file format and type conventions from your system prompt's auto-memory section — it's the source of truth for what to save, how to structure it, and what NOT to save.

Focus on:
- Merging new signal into existing topic files rather than creating near-duplicates
- Converting relative dates ("yesterday", "last week") to absolute dates so they remain interpretable after time passes
- Deleting contradicted facts — if today's investigation disproves an old memory, fix it at the source

### Reconcile memories against CLAUDE.md

Project CLAUDE.md instructions are loaded in your system prompt. For each `feedback` or `project` memory, check whether it contradicts a CLAUDE.md instruction on the same topic:

- **Memory is stale** — CLAUDE.md and the memory describe different procedures for the same task: CLAUDE.md is the maintained, checked-in source. Delete the memory, or rewrite it to agree if it carries context worth keeping (the *why* is still useful but the *how* is wrong).
- **CLAUDE.md may be stale** — the memory is clearly dated after CLAUDE.md and explicitly corrects it: do NOT edit CLAUDE.md during a dream. Annotate the memory with "contradicts CLAUDE.md — verify which is current" and list it in your summary so the user can update CLAUDE.md.
- **Not a conflict** — the memory adds detail CLAUDE.md doesn't cover, or narrows a CLAUDE.md rule with a stated reason. Leave it.

A `feedback` memory's "Why: the user corrected me" framing is not evidence it's newer than CLAUDE.md — CLAUDE.md may have been updated since.

## Phase 4 — Prune and index

Update `MEMORY.md` so it stays under 200 lines AND under ~25KB. It's an **index**, not a dump — each entry should be one line under ~150 characters: `- [Title](file.md) — one-line hook`. Never write memory content directly into it.

- Remove pointers to memories that are now stale, wrong, or superseded
- Demote verbose entries: if an index line is over ~200 chars, it's carrying content that belongs in the topic file — shorten the line, move the detail
- Add pointers to newly important memories
- Resolve contradictions — if two files disagree, fix the wrong one

---

Return a brief summary of what you consolidated, updated, or pruned. If nothing changed (memories are already tight), say so.
"#;

const SESSION_TEMPLATE: &str = r#"# Session Title
_A short and distinctive 5-10 word descriptive title for the session. Super info dense, no filler_

# Current State
_What is actively being worked on right now? Pending tasks not yet completed. Immediate next steps._

# Task specification
_What did the user ask to build? Any design decisions or other explanatory context_

# Files and Functions
_What are the important files? In short, what do they contain and why are they relevant?_

# Workflow
_What bash commands are usually run and in what order? How to interpret their output if not obvious?_

# Errors & Corrections
_Errors encountered and how they were fixed. What did the user correct? What approaches failed and should not be tried again?_

# Codebase and System Documentation
_What are the important system components? How do they work/fit together?_

# Learnings
_What has worked well? What has not? What to avoid? Do not duplicate items from other sections_

# Key results
_If the user asked a specific output such as an answer to a question, a table, or other document, repeat the exact result here_

# Worklog
_Step by step, what was attempted, done? Very terse summary for each step_
"#;

const SEARCHING_PAST_CONTEXT: &str = r#"## Searching past context

When looking for past context:

1. Search topic files in your memory directory:
```
Grep with pattern="<search term>" path="{MEMORY_DIR}" glob="*.md"
```
2. Session transcript logs (last resort — large files, slow):
```
Grep with pattern="<search term>" path="{TRANSCRIPT_DIR}/" glob="*.jsonl"
```
Use narrow search terms (error messages, file paths, function names) rather than broad keywords.
"#;

const DREAM_TEAM_GUIDANCE: &str = "## Team memory (`team/` subdirectory)\n\
\n\
The `team/` subdirectory holds memories shared across everyone working in this repo. Other teammates' Claude sessions write here too — treat it differently from your personal files:\n\
\n\
- **Phase 1:** `ls team/` and skim it alongside your personal files. A teammate may have already captured something you'd otherwise duplicate.\n\
- **Phase 3:** Merge near-duplicates *within* `team/` the same way you would personal memories. If a personal memory restates a team memory, delete the personal one.\n\
- **Phase 4 — be conservative pruning `team/`:**\n\
  - DO delete or fix a team memory that is clearly contradicted by the current code, or that a newer team memory marks as superseded.\n\
  - DO NOT delete a team memory just because you don't recognize it or it isn't relevant to *your* recent sessions — a teammate may rely on it.\n\
  - When unsure, leave it. A stale team memory costs little; deleting a teammate's load-bearing note costs a lot.\n\
\n\
Do not promote personal memories into `team/` during a dream — that's a deliberate choice the user makes via `/remember`, not something to do reflexively.";

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
    /// `COCO_MEMORY_STORES` team-only path: no writable user-scoped
    /// store, so the prompt must not expose a private memory directory.
    TeamOnly,
    /// KAIROS daily-log mode (assistant-mode append-only).
    Kairos,
}

/// Model-facing file mutation tools for prompt copy. Derived by callers
/// from `ToolOverrides`, not from model names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMutationPromptTools {
    pub write_tool: ToolName,
    pub edit_tool: ToolName,
}

impl FileMutationPromptTools {
    pub fn native() -> Self {
        Self {
            write_tool: ToolName::Write,
            edit_tool: ToolName::Edit,
        }
    }

    fn write_directly_phrase(self) -> String {
        if matches!(self.write_tool, ToolName::ApplyPatch) {
            format!("write to it directly with {}", self.write_tool.as_str())
        } else {
            format!(
                "write to it directly with the {} tool",
                self.write_tool.as_str()
            )
        }
    }

    fn write_directly_plural_phrase(self) -> String {
        if matches!(self.write_tool, ToolName::ApplyPatch) {
            format!("write to them directly with {}", self.write_tool.as_str())
        } else {
            format!(
                "write to them directly with the {} tool",
                self.write_tool.as_str()
            )
        }
    }

    fn extract_guidance(self, message_count: i32) -> String {
        let available_tools = if matches!(self.write_tool, ToolName::ApplyPatch)
            || matches!(self.edit_tool, ToolName::ApplyPatch)
        {
            format!(
                "Available tools: Read, Grep, Glob, read-only Bash (ls/find/cat/stat/wc/head/tail and similar), and {} for creating or updating `.md` paths inside the memory directory only. Bash `rm` is not permitted. All other tools — MCP, Agent, write-capable Bash, etc — will be denied.",
                ToolName::ApplyPatch.as_str(),
            )
        } else {
            format!(
                "Available tools: Read, Grep, Glob, read-only Bash (ls/find/cat/stat/wc/head/tail and similar), {} for creating `.md` files, and {} for updating `.md` files inside the memory directory only. Bash `rm` is not permitted. All other tools — MCP, Agent, write-capable Bash, etc — will be denied.",
                self.write_tool.as_str(),
                self.edit_tool.as_str(),
            )
        };

        let budget = if matches!(self.edit_tool, ToolName::ApplyPatch) {
            format!(
                "You have a limited turn budget. For updates, issue all Read calls in parallel for every file you might patch; then issue all {} calls in parallel. Do not interleave reads and patches across multiple turns.",
                ToolName::ApplyPatch.as_str(),
            )
        } else {
            format!(
                "You have a limited turn budget. {} requires a prior Read of the same file, so the efficient strategy is: turn 1 — issue all Read calls in parallel for every file you might update; turn 2 — issue all {}/{} calls in parallel. Do not interleave reads and writes across multiple turns.",
                self.edit_tool.as_str(),
                self.write_tool.as_str(),
                self.edit_tool.as_str(),
            )
        };

        format!(
            "You are now acting as the memory extraction subagent. Analyze the most recent ~{message_count} messages above and use them to update your persistent memory systems.\n\n{available_tools}\n\n{budget}\n\nYou MUST only use content from the last ~{message_count} messages to update your persistent memories. Do not waste any turns attempting to investigate or verify that content further — no grepping source files, no reading code to confirm a pattern exists, no git commands.\n\nIf the user explicitly asks you to remember something, save it immediately as whichever type fits best. If they ask you to forget something, find and remove the relevant entry."
        )
    }
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
    memory_stores: &[MemoryStore],
    tools: FileMutationPromptTools,
) -> String {
    if matches!(variant, SystemPromptVariant::Kairos) {
        return build_kairos_prompt(
            memory_dir,
            skip_index,
            searching_past_context,
            transcript_dir,
            tools,
        );
    }
    if matches!(variant, SystemPromptVariant::TeamOnly) {
        return build_team_only_prompt_section(
            team_dir.unwrap_or(memory_dir),
            skip_index,
            extra_guidelines,
            memory_stores,
            tools,
        );
    }

    let combined = matches!(variant, SystemPromptVariant::Combined);
    let mut sections = Vec::new();
    sections.push("# auto memory".to_string());

    let dir_blurb = if let Some(team) = team_dir
        && combined
    {
        format!(
            "You have a persistent, file-based memory system with two directories: a private directory at `{}` and a shared team directory at `{}`. Both directories already exist — {} (do not run mkdir or check for their existence).",
            memory_dir.display(),
            team.display(),
            tools.write_directly_plural_phrase(),
        )
    } else {
        format!(
            "You have a persistent, file-based memory system at `{}`. This directory already exists — {} (do not run mkdir or check for its existence).",
            memory_dir.display(),
            tools.write_directly_phrase(),
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

    // Mounted-store guidance (the `e0t` recall-dispatcher analogue at the
    // prose level). Enumerate parsed stores and render distinct guidance
    // for writable (rw) vs read-only (ro) mounts. Rendered only in
    // combined mode (team recall is enabled — mounted ⇒ enabled).
    if combined
        && let Some(team) = team_dir
        && let Some(section) = render_mounted_stores_section(memory_stores, team, tools)
    {
        sections.push(section);
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

fn build_team_only_prompt_section(
    team_dir: &Path,
    skip_index: bool,
    extra_guidelines: Option<&str>,
    memory_stores: &[MemoryStore],
    tools: FileMutationPromptTools,
) -> String {
    let team_rw: Vec<&MemoryStore> = memory_stores
        .iter()
        .filter(|s| matches!(s.scope, StoreScope::Team) && matches!(s.mode, StoreMode::Rw))
        .collect();
    let team_ro: Vec<&MemoryStore> = memory_stores
        .iter()
        .filter(|s| matches!(s.scope, StoreScope::Team) && matches!(s.mode, StoreMode::Ro))
        .collect();
    let writable = !team_rw.is_empty();
    let single_writable = team_rw.len() == 1;

    let mount_dir = |store: &MemoryStore| -> String {
        let name = store.mount.as_deref().unwrap_or("(unnamed)");
        format!("{}/", team_dir.join(name).display())
    };
    let index_path = |store: &MemoryStore| {
        let prompt_index = store.prompt_index.as_deref().unwrap_or("MEMORY.md");
        format!("{}{}", mount_dir(store), prompt_index)
    };

    let mut sections = Vec::new();
    sections.push("# Memory".to_string());

    if single_writable {
        sections.push(format!(
            "You have a persistent, file-based team memory directory at `{}`. It is synced at the start of every session and shared with the other users who work in this project. This directory already exists — {} (do not run mkdir or check for its existence).",
            mount_dir(team_rw[0]),
            tools.write_directly_phrase(),
        ));
    } else if writable {
        let list = team_rw
            .iter()
            .map(|s| format!("- `{}`", mount_dir(s)))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "You have a persistent, file-based team memory system with {} directories, each synced and shared with the other users in this project:\n{list}\nThese directories already exist — {} (do not run mkdir or check for their existence).",
            team_rw.len(),
            tools.write_directly_plural_phrase(),
        ));
    } else {
        sections.push(
            "You have read-only access to team memory synced from your project. You cannot persist new memories in this session."
                .to_string(),
        );
    }

    if !team_ro.is_empty() {
        let list = team_ro
            .iter()
            .map(|s| format!("`{}`", mount_dir(s)))
            .collect::<Vec<_>>()
            .join(", ");
        let target = if team_ro.len() == 1 { "it" } else { "these" };
        sections.push(format!(
            "You also have read-only team memory at {list}. Read from {target} when relevant, but do not write there — changes will not persist."
        ));
    }

    if writable {
        sections.push(BEHAVIOR_GUIDANCE.to_string());
    } else {
        sections.push(
            "If the user asks you to remember something, explain that memory is read-only in this session."
                .to_string(),
        );
    }

    sections.push(TYPES_COMBINED.to_string());
    if writable {
        let destination = if single_writable {
            format!("`{}`", mount_dir(team_rw[0]))
        } else {
            "one of the team directories listed above".to_string()
        };
        sections.push(format!(
            "There is no separate private memory directory in this session. Save every memory type to {destination}, bearing in mind it is shared with teammates."
        ));
    }

    let mut what_not = WHAT_NOT_TO_SAVE.to_string();
    what_not.push('\n');
    what_not.push_str(COMBINED_TEAM_SECRET_ADDENDUM);
    sections.push(what_not);

    if writable {
        sections.push(team_only_how_to_save(
            &team_rw,
            skip_index,
            &mount_dir,
            &index_path,
        ));
    }

    sections.push(team_only_when_to_access());
    sections.push(PERSISTENCE_GUIDANCE.to_string());

    if let Some(guidance) = extra_guidelines
        && !guidance.trim().is_empty()
    {
        sections.push(guidance.to_string());
    }

    sections.join("\n\n")
}

fn team_only_how_to_save<'a, F, G>(
    team_rw: &[&'a MemoryStore],
    skip_index: bool,
    mount_dir: &F,
    index_path: &G,
) -> String
where
    F: Fn(&'a MemoryStore) -> String,
    G: Fn(&'a MemoryStore) -> String,
{
    let single = team_rw.len() == 1;
    let destination = if single {
        format!("`{}`", mount_dir(team_rw[0]))
    } else {
        let dirs = team_rw
            .iter()
            .map(|s| format!("`{}`", mount_dir(s)))
            .collect::<Vec<_>>()
            .join(" or ");
        format!("the appropriate team directory ({dirs})")
    };

    if skip_index {
        let example = memory_frontmatter_example();
        return format!(
            "## How to save memories\n\nWrite each memory to its own file in {destination} using this frontmatter format:\n\n{example}\n\n- Keep the name, description, and type fields in memory files up-to-date with the content\n- Organize memory semantically by topic, not chronologically\n- Update or remove memories that turn out to be wrong or outdated\n- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.",
        );
    }

    let example = memory_frontmatter_example();
    let index_target = if single {
        format!("`{}`", index_path(team_rw[0]))
    } else {
        let indexes = team_rw
            .iter()
            .map(|s| format!("`{}`", index_path(s)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("the index file in that same directory ({indexes})")
    };
    let all_explicit_prompt_indexes = team_rw.iter().all(|s| s.prompt_index.is_some());
    let index_budget_note = if all_explicit_prompt_indexes {
        format!(
            "- The index file is loaded into your conversation context — lines after {MAX_ENTRYPOINT_LINES} will be truncated, so keep it concise"
        )
    } else {
        "- Keep the index concise so you can scan it quickly when recalling memories".to_string()
    };

    format!(
        "## How to save memories\n\nSaving a memory is a two-step process:\n\n**Step 1** — write the memory to its own file in {destination} using this frontmatter format:\n\n{example}\n\n**Step 2** — add a pointer to that file in {index_target}. Each entry should be one line, under ~150 characters: `- [Title](file.md) — one-line hook`. The index has no frontmatter. Never write memory content directly into the index.\n\n{index_budget_note}\n- Keep the name, description, and type fields in memory files up-to-date with the content\n- Organize memory semantically by topic, not chronologically\n- Update or remove memories that turn out to be wrong or outdated\n- Do not write duplicate memories. First check if there is an existing memory you can update before writing a new one.",
    )
}

fn team_only_when_to_access() -> String {
    "## When to access memories\n- When memories seem relevant, or the user references prior work with them or others in their organization.\n- You MUST access memory when the user explicitly asks you to check, recall, or remember.\n- If the user says to *ignore* or *not use* memory: Do not apply remembered facts, cite, compare against, or mention memory content.\n- Memory records can become stale over time. Use memory as context for what was true at a given point in time. Before answering the user or building assumptions based solely on information in memory records, verify that the memory is still correct and up-to-date by reading the current state of the files or resources. If a recalled memory conflicts with current information, trust what you observe now — and update or remove the stale memory rather than acting on it.\n\n## Before recommending from memory\n\nA memory that names a specific function, file, or flag is a claim that it existed *when the memory was written*. It may have been renamed, removed, or never merged. Before recommending it:\n\n- If the memory names a file path: check the file exists.\n- If the memory names a function or flag: grep for it.\n- If the user is about to act on your recommendation (not just asking about history), verify first.\n\n\"The memory says X exists\" is not the same as \"X exists now.\"\n\nA memory that summarizes repo state (activity logs, architecture snapshots) is frozen in time. If the user asks about *recent* or *current* state, prefer `git log` or reading the code over recalling the snapshot.".to_string()
}

/// Render the "## Mounted memory stores" prose for the parsed
/// `COCO_MEMORY_STORES` entries.
///
/// Splits team-scoped stores into writable (rw) and read-only (ro)
/// lists and renders distinct guidance: how/where to save for writable
/// mounts, "reference only — do not write here" for read-only mounts.
/// User-scoped stores are listed separately as private mounts.
///
/// Returns `None` when there are no stores (so the section is omitted
/// entirely). This is the prose-level analogue of CC's scope-aware
/// recall dispatcher. `MemoryStore::prompt_index` content is loaded by
/// `MemoryRuntime` and passed through `extra_guidelines`; this pure
/// renderer uses that field only to point writable team stores at the
/// correct mounted index file.
fn render_mounted_stores_section(
    stores: &[MemoryStore],
    team_dir: &Path,
    tools: FileMutationPromptTools,
) -> Option<String> {
    if stores.is_empty() {
        return None;
    }

    fn mount_name(store: &MemoryStore) -> &str {
        store.mount.as_deref().unwrap_or("(unnamed)")
    }

    let team_mount_dir = |store: &MemoryStore| -> String {
        let name = mount_name(store);
        format!("{}/", team_dir.join(name).display())
    };
    let team_index_path = |store: &MemoryStore| -> String {
        let index = store.prompt_index.as_deref().unwrap_or("MEMORY.md");
        format!("{}{index}", team_mount_dir(store))
    };
    let team_mount_label = |store: &MemoryStore| -> String {
        let name = mount_name(store);
        format!("- `{}` (mount `{name}`)", team_mount_dir(store))
    };
    let user_mount_label = |store: &MemoryStore| -> String {
        let name = store.mount.as_deref().unwrap_or("(unnamed)");
        format!("- `{name}` — {}", store.path.display())
    };

    let team_rw: Vec<&MemoryStore> = stores
        .iter()
        .filter(|s| matches!(s.scope, StoreScope::Team) && matches!(s.mode, StoreMode::Rw))
        .collect();
    let team_ro: Vec<&MemoryStore> = stores
        .iter()
        .filter(|s| matches!(s.scope, StoreScope::Team) && matches!(s.mode, StoreMode::Ro))
        .collect();
    let user_stores: Vec<&MemoryStore> = stores
        .iter()
        .filter(|s| matches!(s.scope, StoreScope::User))
        .collect();

    let mut parts: Vec<String> = Vec::new();
    parts.push("## Mounted memory stores".to_string());
    parts.push("Additional memory stores are mounted for this session. Team stores appear as local directories under the team memory directory; write to those mounted directories, not to the backing store paths.".to_string());

    if !team_rw.is_empty() {
        let list = team_rw
            .iter()
            .map(|s| team_mount_label(s))
            .collect::<Vec<_>>()
            .join("\n");
        let index_targets = team_rw
            .iter()
            .map(|s| format!("`{}`", team_index_path(s)))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!(
            "### Team stores (writable)\n\nThese shared stores are writable and already exist — {}:\n\n{list}\n\nWhen saving, add the pointer to the mounted store's index file ({index_targets}). Each entry should be one line, under ~150 characters. Never write memory content directly into the index.",
            tools.write_directly_plural_phrase(),
        ));
    }

    if !team_ro.is_empty() {
        let list = team_ro
            .iter()
            .map(|s| team_mount_label(s))
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!(
            "### Team stores (read-only)\n\nThese shared stores are read-only — reference their contents for relevant context but do not write there because changes will not persist:\n\n{list}"
        ));
    }

    if !user_stores.is_empty() {
        let list = user_stores
            .iter()
            .map(|s| {
                let mode = match s.mode {
                    StoreMode::Rw => "writable",
                    StoreMode::Ro => "read-only",
                };
                format!("{} ({mode})", user_mount_label(s))
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!(
            "### Private store\n\nA private memory store mounted for the current user only:\n\n{list}"
        ));
    }

    Some(parts.join("\n\n"))
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
    tools: FileMutationPromptTools,
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
    sections.push(format!("Record each entry as a short timestamped bullet. Create the file (and parent directories) on first write with {} if it does not exist. Do not rewrite or reorganize the log — it is append-only. A separate nightly process distills these logs into `MEMORY.md` and topic files.", tools.write_tool.as_str()));
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
    tools: FileMutationPromptTools,
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
    let guidance = tools.extract_guidance(message_count);
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
/// The `## Additional context` block carries the bash sandbox
/// constraint reminder + sessions-since-last list. The constraint
/// reminder is appended even when `sessions_since_last` is empty so a
/// forced /dream call still gets the heads-up.
pub fn build_dream_prompt(
    memory_dir: &Path,
    transcript_dir: &Path,
    sessions_since_last: &[String],
    team_memory_enabled: bool,
    tools: FileMutationPromptTools,
) -> String {
    let mem = memory_dir.display().to_string();
    let trans = transcript_dir.display().to_string();
    let mut body = DREAM_GUIDANCE
        .replace("{MEMORY_ROOT}", &mem)
        .replace("{TRANSCRIPT_DIR}", &trans)
        .replace("{WRITE_DIRECTLY}", &tools.write_directly_phrase());
    if team_memory_enabled {
        body = body.replacen("\n---\n", &format!("\n\n{DREAM_TEAM_GUIDANCE}\n\n---\n"), 1);
    }
    let bash_constraint = "**Tool constraints for this run:** Bash is restricted to read-only commands (`ls`, `find`, `grep`, `cat`, `stat`, `wc`, `head`, `tail`, and similar) plus deleting `.md` paths inside the memory directory. Anything else that writes, redirects to a file, or modifies state will be denied. Plan your exploration with this in mind — no need to probe.";
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
