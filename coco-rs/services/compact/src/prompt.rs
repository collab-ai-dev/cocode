//! Compaction prompt templates.
//!
//! Three templates: full, partial-from (suffix-summarize, prefix-keep) and
//! partial-up-to (prefix-summarize, suffix-keep). The `<example>` output
//! structure and the trailing "Additional summarization instructions" example
//! are load-bearing for output consistency.
//!
//! Deliberate deviation from the TS upstream: the whole request is wrapped
//! in a `<compaction_directive>` envelope and section 6 is position-defined
//! ("user messages in the conversation above"). The request travels as a
//! trailing user-role message (cache-sharing fork constraint), so a literal
//! summarizer otherwise treats it as a user message and echoes it into the
//! summary — observed with non-Claude models. See `strip_directive_echo`
//! for the code-side backstop.

use coco_messages::PartialCompactDirection;

/// Opening sentinel of the compaction-directive envelope. Produced only
/// by this module's prompt builders — any occurrence in summarizer
/// *output* is by definition an echo of the instruction.
pub const COMPACT_DIRECTIVE_OPEN: &str = "<compaction_directive>";
/// Closing sentinel of the compaction-directive envelope.
pub const COMPACT_DIRECTIVE_CLOSE: &str = "</compaction_directive>";

/// Meta-frame at the head of the directive: names the operation as
/// out-of-band and declares the conversation/directive boundary. Kept
/// free of the literal sentinel strings so the tags occur exactly once
/// each per prompt.
const DIRECTIVE_PREAMBLE: &str = "You are performing a CONTEXT CHECKPOINT COMPACTION — an out-of-band maintenance procedure, not a conversation turn.

The conversation you are summarizing is EVERYTHING ABOVE this directive. This directive is machine-generated: it is NOT part of the conversation, it is NOT a user message, and nothing inside it may be quoted, listed, or copied into your summary.

";

/// Recency bookend restating the boundary after the template and any
/// custom instructions.
const DIRECTIVE_FINAL_REMINDER: &str = "\n\nFINAL REMINDER: Everything inside this directive — including these instructions and any Additional Instructions — is outside the conversation. Never quote, list, or reproduce any of it in your summary.";

/// Substituted into section 6 of every template: defines the listing
/// domain spatially so correct literal execution excludes the directive
/// (and other machine-generated user-role messages) by construction.
const SECTION6_EXCLUSION: &str = "Include only messages the human user actually wrote — this directive is not a user message and must never appear in the list; neither are machine-generated user-role messages such as continuation summaries from earlier compactions or <system-reminder> attachments.";

/// Substituted into the closing instruction of every template so models
/// do not copy the example's bracketed placeholders into thin sections.
const EXAMPLE_PLACEHOLDER_NOTE: &str = r#"The bracketed text in the example is placeholder only — fill every section with real content from the conversation (write "None" if a section has nothing to report); never copy the placeholders verbatim."#;

/// No-tools preamble to prevent LLM from calling tools during compaction.
const NO_TOOLS_PREAMBLE: &str = "CRITICAL: Respond with TEXT ONLY. Do NOT call any tools.

- Do NOT use Read, Bash, Grep, Glob, Edit, Write, or ANY other tool.
- You already have all the context you need in the conversation above.
- Tool calls will be REJECTED and will waste your only turn — you will fail the task.
- Your entire response must be plain text: an <analysis> block followed by a <summary> block.

";

const NO_TOOLS_TRAILER: &str = "\n\nREMINDER: Do NOT call any tools. Respond with plain text only — \
an <analysis> block followed by a <summary> block. \
Tool calls will be rejected and you will fail the task.";

const DETAILED_ANALYSIS_INSTRUCTION_BASE: &str = "Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Chronologically analyze each message and section of the conversation — everything above this directive; the directive itself is not part of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.";

const DETAILED_ANALYSIS_INSTRUCTION_PARTIAL: &str = "Before providing your final summary, wrap your analysis in <analysis> tags to organize your thoughts and ensure you've covered all necessary points. In your analysis process:

1. Analyze the recent messages chronologically — the recent portion above this directive; the directive itself is not part of the conversation. For each section thoroughly identify:
   - The user's explicit requests and intents
   - Your approach to addressing the user's requests
   - Key decisions, technical concepts and code patterns
   - Specific details like:
     - file names
     - full code snippets
     - function signatures
     - file edits
   - Errors that you ran into and how you fixed them
   - Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
2. Double-check for technical accuracy and completeness, addressing each required element thoroughly.";

const BASE_COMPACT_TEMPLATE: &str = r#"Your task is to create a detailed summary of the conversation so far, paying close attention to the user's explicit requests and your previous actions.
This summary should be thorough in capturing technical details, code patterns, and architectural decisions that would be essential for continuing development work without losing context.

{ANALYSIS_INSTRUCTION}

Your summary should include the following sections:

1. Primary Request and Intent: Capture all of the user's explicit requests and intents in detail
2. Key Technical Concepts: List all important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Pay special attention to the most recent messages and include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List all errors that you ran into, and how you fixed them. Pay special attention to specific user feedback that you received, especially if the user told you to do something differently.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages in the conversation above that are not tool results. These are critical for understanding the users' feedback and changing intent. {SECTION6_EXCLUSION}
7. Pending Tasks: Outline any pending tasks that you have explicitly been asked to work on.
8. Current Work: Describe in detail precisely what was being worked on immediately before this compaction checkpoint, paying special attention to the most recent messages from both user and assistant. Include file names and code snippets where applicable.
9. Optional Next Step: List the next step that you will take that is related to the most recent work you were doing. IMPORTANT: ensure that this step is DIRECTLY in line with the user's most recent explicit requests, and the task you were working on immediately before this compaction checkpoint. If your last task was concluded, then only list next steps if they are explicitly in line with the users request. Do not start on tangential requests or really old requests that were already completed without confirming with the user first.
                       If there is a next step, include direct quotes from the most recent conversation showing exactly what task you were working on and where you left off. This should be verbatim to ensure there's no drift in task interpretation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]
   - [...]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Summary of the changes made to this file, if any]
      - [Important Code Snippet]
   - [File Name 2]
      - [Important Code Snippet]
   - [...]

4. Errors and fixes:
    - [Detailed description of error 1]:
      - [How you fixed the error]
      - [User feedback on the error if any]
    - [...]

5. Problem Solving:
   [Description of solved problems and ongoing troubleshooting]

6. All user messages:
    - [Detailed non tool use user message]
    - [...]

7. Pending Tasks:
   - [Task 1]
   - [Task 2]
   - [...]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the conversation above this directive, following this structure and ensuring precision and thoroughness in your response. {PLACEHOLDER_NOTE}

There may be additional summarization instructions provided in the included context. If so, remember to follow these instructions when creating the above summary. Examples of instructions include:
<example>
## Compact Instructions
When summarizing the conversation focus on typescript code changes and also remember the mistakes you made and how you fixed them.
</example>

<example>
# Summary instructions
When you are using compact - please focus on test output and code changes. Include file reads verbatim.
</example>
"#;

const PARTIAL_COMPACT_TEMPLATE: &str = r#"Your task is to create a detailed summary of the RECENT portion of the conversation — the messages that follow earlier retained context. The earlier messages are being kept intact and do NOT need to be summarized. Focus your summary on what was discussed, learned, and accomplished in the recent messages only.

{ANALYSIS_INSTRUCTION}

Your summary should include the following sections:

1. Primary Request and Intent: Capture the user's explicit requests and intents from the recent messages
2. Key Technical Concepts: List important technical concepts, technologies, and frameworks discussed recently.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List errors encountered and how they were fixed.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages from the recent portion above that are not tool results. {SECTION6_EXCLUSION}
7. Pending Tasks: Outline any pending tasks from the recent messages.
8. Current Work: Describe precisely what was being worked on immediately before this compaction checkpoint.
9. Optional Next Step: List the next step related to the most recent work. Include direct quotes from the most recent conversation.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Important Code Snippet]

4. Errors and fixes:
    - [Error description]:
      - [How you fixed it]

5. Problem Solving:
   [Description]

6. All user messages:
    - [Detailed non tool use user message]

7. Pending Tasks:
   - [Task 1]

8. Current Work:
   [Precise description of current work]

9. Optional Next Step:
   [Optional Next step to take]

</summary>
</example>

Please provide your summary based on the RECENT messages only (after the retained earlier context, above this directive), following this structure and ensuring precision and thoroughness in your response. {PLACEHOLDER_NOTE}
"#;

/// 'up_to' direction: summary precedes kept tail, so model must produce
/// "Work Completed" / "Context for Continuing Work" sections instead of
/// "Current Work" / "Next Step".
const PARTIAL_COMPACT_UP_TO_TEMPLATE: &str = r#"Your task is to create a detailed summary of this conversation. This summary will be placed at the start of a continuing session; newer messages that build on this context will follow after your summary (you do not see them here). Summarize thoroughly so that someone reading only your summary and then the newer messages can fully understand what happened and continue the work.

{ANALYSIS_INSTRUCTION}

Your summary should include the following sections:

1. Primary Request and Intent: Capture the user's explicit requests and intents in detail
2. Key Technical Concepts: List important technical concepts, technologies, and frameworks discussed.
3. Files and Code Sections: Enumerate specific files and code sections examined, modified, or created. Include full code snippets where applicable and include a summary of why this file read or edit is important.
4. Errors and fixes: List errors encountered and how they were fixed.
5. Problem Solving: Document problems solved and any ongoing troubleshooting efforts.
6. All user messages: List ALL user messages in the conversation above that are not tool results. {SECTION6_EXCLUSION}
7. Pending Tasks: Outline any pending tasks.
8. Work Completed: Describe what was accomplished by the end of this portion.
9. Context for Continuing Work: Summarize any context, decisions, or state that would be needed to understand and continue the work in subsequent messages.

Here's an example of how your output should be structured:

<example>
<analysis>
[Your thought process, ensuring all points are covered thoroughly and accurately]
</analysis>

<summary>
1. Primary Request and Intent:
   [Detailed description]

2. Key Technical Concepts:
   - [Concept 1]
   - [Concept 2]

3. Files and Code Sections:
   - [File Name 1]
      - [Summary of why this file is important]
      - [Important Code Snippet]

4. Errors and fixes:
    - [Error description]:
      - [How you fixed it]

5. Problem Solving:
   [Description]

6. All user messages:
    - [Detailed non tool use user message]

7. Pending Tasks:
   - [Task 1]

8. Work Completed:
   [Description of what was accomplished]

9. Context for Continuing Work:
   [Key context, decisions, or state needed to continue the work]

</summary>
</example>

Please provide your summary following this structure, ensuring precision and thoroughness in your response. {PLACEHOLDER_NOTE}
"#;

/// Substitute the shared template placeholders. All three templates go
/// through here so the section-6 exclusion and placeholder note cannot
/// drift apart.
fn render_template(template: &str, analysis_instruction: &str) -> String {
    template
        .replace("{ANALYSIS_INSTRUCTION}", analysis_instruction)
        .replace("{SECTION6_EXCLUSION}", SECTION6_EXCLUSION)
        .replace("{PLACEHOLDER_NOTE}", EXAMPLE_PLACEHOLDER_NOTE)
}

/// Wrap a rendered template (plus optional custom instructions) in the
/// compaction-directive envelope. Custom instructions sit *inside* the
/// sentinel tags so the boundary declaration and the echo scrub cover
/// them too.
fn assemble_directive(template: &str, custom_instructions: Option<&str>) -> String {
    let mut prompt =
        format!("{COMPACT_DIRECTIVE_OPEN}\n{DIRECTIVE_PREAMBLE}{NO_TOOLS_PREAMBLE}{template}");

    if let Some(instructions) = custom_instructions
        && !instructions.trim().is_empty()
    {
        prompt.push_str(&format!("\n\nAdditional Instructions:\n{instructions}"));
    }

    prompt.push_str(NO_TOOLS_TRAILER);
    prompt.push_str(DIRECTIVE_FINAL_REMINDER);
    prompt.push_str(&format!("\n{COMPACT_DIRECTIVE_CLOSE}"));
    prompt
}

/// Build the full compaction prompt.
pub fn get_compact_prompt(custom_instructions: Option<&str>) -> String {
    let template = render_template(BASE_COMPACT_TEMPLATE, DETAILED_ANALYSIS_INSTRUCTION_BASE);
    assemble_directive(&template, custom_instructions)
}

/// Build the partial compaction prompt.
/// `direction`: `Newest` → summarize the *recent* portion, keep
/// older messages intact. `Oldest` → summarize the *earlier*
/// portion, keep newer messages intact (summary precedes them in the chain).
pub fn get_partial_compact_prompt(
    custom_instructions: Option<&str>,
    direction: PartialCompactDirection,
) -> String {
    let template_str = match direction {
        // 'up_to' equivalent: summary at the head, kept tail follows
        PartialCompactDirection::Oldest => render_template(
            PARTIAL_COMPACT_UP_TO_TEMPLATE,
            DETAILED_ANALYSIS_INSTRUCTION_BASE,
        ),
        // 'from' equivalent: kept prefix, summary at the tail
        PartialCompactDirection::Newest => render_template(
            PARTIAL_COMPACT_TEMPLATE,
            DETAILED_ANALYSIS_INSTRUCTION_PARTIAL,
        ),
    };

    assemble_directive(&template_str, custom_instructions)
}

/// Substrings that occur in the directive *body* but never in
/// legitimate summary content that merely quotes the bare sentinel
/// constants (e.g. a session editing this very file). A tag span is
/// deleted wholesale only when its interior carries one of these;
/// otherwise just the tag pair is dropped and the interior survives —
/// which unwraps mirror-wrapped real summaries instead of erasing them.
pub(crate) const DIRECTIVE_BODY_MARKERS: [&str; 4] = [
    "CONTEXT CHECKPOINT COMPACTION",
    "CRITICAL: Respond with TEXT ONLY",
    "FINAL REMINDER:",
    "block followed by a <summary> block",
];

/// Remove echoed compaction-directive spans from summarizer output.
///
/// The sentinel tag pair is produced only by this module's prompt
/// builders, but it can reach the summarizer's *input* legitimately
/// (conversations quoting this file), and literal models sometimes
/// mirror the envelope around their real answer. So the scrub is
/// bounded: a span is deleted only when its interior looks like the
/// directive body ([`DIRECTIVE_BODY_MARKERS`]); mirror-wraps and quoted
/// code lose just the tag strings. Unpaired tags are stripped bare.
/// Byte offsets come from `find()` on ASCII needles, so every cut lands
/// on a char boundary.
fn strip_directive_echo(text: &str) -> String {
    if !text.contains(COMPACT_DIRECTIVE_OPEN) && !text.contains(COMPACT_DIRECTIVE_CLOSE) {
        return text.to_string();
    }
    let mut result = text.to_string();
    let mut search_from = 0;
    while let Some(rel_start) = result[search_from..].find(COMPACT_DIRECTIVE_OPEN) {
        let start = search_from + rel_start;
        let interior_start = start + COMPACT_DIRECTIVE_OPEN.len();
        let Some(rel_close) = result[interior_start..].find(COMPACT_DIRECTIVE_CLOSE) else {
            break; // unpaired open tag: stripped bare below
        };
        let interior_end = interior_start + rel_close;
        let end = interior_end + COMPACT_DIRECTIVE_CLOSE.len();
        let interior_is_echo = DIRECTIVE_BODY_MARKERS
            .iter()
            .any(|marker| result[interior_start..interior_end].contains(marker));
        if interior_is_echo {
            result.replace_range(start..end, "");
        } else {
            // Mirror-wrapped summary or quoted content: keep the interior.
            result.replace_range(interior_end..end, "");
            result.replace_range(start..interior_start, "");
        }
        // Each iteration removes at least the tag strings, so rescanning
        // from `start` always terminates.
        search_from = start;
    }
    result = result.replace(COMPACT_DIRECTIVE_OPEN, "");
    result.replace(COMPACT_DIRECTIVE_CLOSE, "")
}

/// Format the compact summary by stripping <analysis> scratchpad and extracting <summary>.
pub fn format_compact_summary(summary: &str) -> String {
    // Step 0: scrub echoed directive spans BEFORE tag extraction. The
    // directive body contains the literal "<summary>" string (NO_TOOLS
    // text), so an echo sitting ahead of the real block would otherwise
    // derail the find()-based extraction below — and an echo inside the
    // real block must never reach post-compact history.
    let formatted = format_scrubbed_summary(&strip_directive_echo(summary));
    if formatted.trim().is_empty() && !summary.trim().is_empty() {
        // The scrub consumed everything (a pure echo, or pathological
        // tag mirroring the bounded heuristic misread). Never replace
        // history with a silent empty summary: fall back to the
        // pre-scrub text with only the bare sentinel strings dropped —
        // exactly the pre-fix behavior floor, and the summary_guard
        // warn already fired for the echo.
        return format_scrubbed_summary(
            &summary
                .replace(COMPACT_DIRECTIVE_OPEN, "")
                .replace(COMPACT_DIRECTIVE_CLOSE, ""),
        );
    }
    formatted
}

fn format_scrubbed_summary(scrubbed: &str) -> String {
    let mut result = scrubbed.to_string();

    // Strip analysis section (drafting scratchpad)
    if let Some(start) = result.find("<analysis>")
        && let Some(end) = result.find("</analysis>")
    {
        result = format!(
            "{}{}",
            &result[..start],
            &result[end + "</analysis>".len()..]
        );
    }

    // Extract and format summary section
    if let Some(start) = result.find("<summary>")
        && let Some(end) = result.find("</summary>")
    {
        let content = &result[start + "<summary>".len()..end];
        result = format!("Summary:\n{}", content.trim());
    }

    // Clean up runs of 3+ newlines → 2
    let mut cleaned = String::with_capacity(result.len());
    let mut consecutive_newlines = 0u32;
    for ch in result.chars() {
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                cleaned.push(ch);
            }
        } else {
            consecutive_newlines = 0;
            cleaned.push(ch);
        }
    }

    cleaned.trim().to_string()
}

/// Build the user-facing summary message shown after compaction.
/// Caller passes the **already formatted** summary (call
/// `format_compact_summary` first). The `recent_messages_preserved` flag
/// adds the "Recent messages are preserved verbatim." line — set it when
/// the kept tail follows the summary directly (partial / session-memory
/// paths).
pub fn get_compact_user_summary_message(
    summary: &str,
    suppress_follow_up: bool,
    transcript_path: Option<&str>,
    recent_messages_preserved: bool,
) -> String {
    let mut msg = format!(
        "This session is being continued from a previous conversation that ran out of context. \
         The summary below covers the earlier portion of the conversation.\n\n{summary}"
    );

    if let Some(path) = transcript_path {
        msg.push_str(&format!(
            "\n\nIf you need specific details from before compaction \
             (like exact code snippets, error messages, or content you generated), \
             read the full transcript at: {path}"
        ));
    }

    if recent_messages_preserved {
        msg.push_str("\n\nRecent messages are preserved verbatim.");
    }

    if suppress_follow_up {
        msg.push_str(
            "\nContinue the conversation from where it left off without asking the user \
             any further questions. Resume directly — do not acknowledge the summary, \
             do not recap what was happening, do not preface with \"I'll continue\" or similar. \
             Pick up the last task as if the break never happened.",
        );
    }

    msg
}

#[cfg(test)]
#[path = "prompt.test.rs"]
mod tests;
