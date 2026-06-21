//! Sanity checks on the byte-faithful built-in prompt bodies.
//!
//! These don't assert the entire prompt verbatim (the strings are
//! ~1-7 KB each); instead they pin signature phrases that drift would
//! break, and the embedded-search variants that conditionally swap tool
//! names.
//!
//! Tool names go through [`coco_types::ToolName`] in both the prompt
//! sources and these assertions — never hard-code a tool name string.

use coco_types::ToolName;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn general_purpose_prompt_includes_shared_prefix_and_guidelines() {
    // `generalPurposeAgent.ts:3` SHARED_PREFIX text + line 7 guidelines header.
    let p = general_purpose_system_prompt();
    assert!(p.starts_with("You are Coco, a CLI coding assistant."));
    assert!(p.contains("Complete the task fully\u{2014}don't gold-plate"));
    assert!(p.contains("Your strengths:"));
    assert!(p.contains("Guidelines:"));
    assert!(p.contains("NEVER proactively create documentation files"));
    // Tool-name reference should resolve from `ToolName`, not be hardcoded.
    assert!(p.contains(&format!("Use {} when you know", ToolName::Read.as_str())));
}

#[test]
fn statusline_setup_prompt_carries_ps1_pattern() {
    let p = STATUSLINE_SETUP_SYSTEM_PROMPT;
    // The literal backslashes survive the Rust raw-string (no `\\n`
    // escape collapse).
    assert!(p.contains(r#"/(?:^|\n)\s*(?:export\s+)?PS1\s*=\s*["']([^"']+)["']/m"#));
    assert!(p.contains("\\u → $(whoami)"));
    assert!(p.contains("ANSI color codes"));
    assert!(p.starts_with("You are a status line setup agent for Coco"));
    assert!(p.contains("~/.coco/settings.json"));
    assert!(p.contains("~/.coco/statusline-command.sh"));
    assert!(p.contains("\"provider\": \"string\""));
    assert!(p.contains("\"cost\": {"));
    assert!(p.contains("\"context_window\": {"));
    assert!(p.contains("\"used\": number | null"));
    assert!(p.contains("\"total\": number | null"));
    assert!(p.contains("\"percent\": number | null"));
    assert!(p.contains("\"exceeds_200k_tokens\": boolean"));
    assert!(p.contains("\"permission_mode\": \"string\""));
    assert!(p.contains("\"lsp\": {"));
    assert!(p.contains("\"connected_servers\": [\"string\"]"));
    assert!(!p.contains("~/.claude"));
    assert!(!p.contains("\"session_name\""));
    assert!(!p.contains("\"transcript_path\""));
    assert!(!p.contains("\"rate_limits\""));
    assert!(!p.contains("\"vim\""));
    assert!(!p.contains("\"agent\""));
    assert!(!p.contains("\"worktree\""));
}

#[test]
fn explore_prompt_default_uses_glob_grep() {
    let glob = ToolName::Glob.as_str();
    let grep = ToolName::Grep.as_str();
    let bash = ToolName::Bash.as_str();
    let p = explore_system_prompt(false);
    assert!(p.contains("READ-ONLY MODE"));
    assert!(p.contains(&format!("- Use {glob} for broad file pattern matching")));
    assert!(p.contains(&format!(
        "- Use {grep} for searching file contents with regex"
    )));
    assert!(!p.contains(&format!("`find` via {bash}")));
    assert!(!p.contains(&format!("`grep` via {bash}")));
}

#[test]
fn explore_prompt_embedded_uses_find_grep_via_bash() {
    let glob = ToolName::Glob.as_str();
    let grep = ToolName::Grep.as_str();
    let bash = ToolName::Bash.as_str();
    let p = explore_system_prompt(true);
    assert!(p.contains(&format!(
        "- Use `find` via {bash} for broad file pattern matching"
    )));
    assert!(p.contains(&format!(
        "- Use `grep` via {bash} for searching file contents with regex"
    )));
    assert!(
        p.contains(", grep,"),
        "embedded variant should list grep in Bash hint"
    );
    assert!(!p.contains(&format!("- Use {glob}")));
    assert!(!p.contains(&format!("- Use {grep} ")));
}

#[test]
fn plan_prompt_default_lists_glob_grep_read() {
    let glob = ToolName::Glob.as_str();
    let grep = ToolName::Grep.as_str();
    let read = ToolName::Read.as_str();
    let p = plan_system_prompt(false);
    assert!(p.contains("software architect"));
    assert!(p.contains(&format!("{glob}, {grep}, and {read}")));
    assert!(!p.contains(&format!("`find`, `grep`, and {read}")));
}

#[test]
fn plan_prompt_embedded_lists_find_grep_read() {
    let glob = ToolName::Glob.as_str();
    let grep = ToolName::Grep.as_str();
    let read = ToolName::Read.as_str();
    let p = plan_system_prompt(true);
    assert!(p.contains(&format!("`find`, `grep`, and {read}")));
    assert!(!p.contains(&format!("{glob}, {grep}, and {read}")));
}

#[test]
fn verification_prompt_includes_required_sections() {
    let p = verification_system_prompt();
    assert!(p.contains("verification specialist"));
    assert!(p.contains("DO NOT MODIFY THE PROJECT"));
    assert!(p.contains("ADVERSARIAL PROBES"));
    assert!(p.contains("VERDICT: PASS"));
    assert!(p.contains("VERDICT: FAIL"));
    assert!(p.contains("VERDICT: PARTIAL"));
    // Verify the runtime substitution of `${BASH_TOOL_NAME}` and
    // `${WEB_FETCH_TOOL_NAME}` actually fired (no leftover `__BASH__`
    // / `__WEB_FETCH__` sentinels).
    assert!(p.contains(&format!("via {} redirection", ToolName::Bash.as_str())));
    assert!(p.contains(&format!(
        ", {}, or other MCP tools",
        ToolName::WebFetch.as_str()
    )));
    assert!(!p.contains("__BASH__"));
    assert!(!p.contains("__WEB_FETCH__"));
}

#[test]
fn verification_critical_reminder_matches_ts() {
    assert_eq!(
        VERIFICATION_CRITICAL_SYSTEM_REMINDER,
        "CRITICAL: This is a VERIFICATION-ONLY task. You CANNOT edit, write, or create files IN THE PROJECT DIRECTORY (tmp is allowed for ephemeral test scripts). You MUST end with VERDICT: PASS, VERDICT: FAIL, or VERDICT: PARTIAL."
    );
}

#[test]
fn coco_guide_default_uses_glob_grep_in_local_hint() {
    let read = ToolName::Read.as_str();
    let glob = ToolName::Glob.as_str();
    let grep = ToolName::Grep.as_str();
    let p = coco_guide_system_prompt(false);
    assert!(p.contains(&format!("{read}, {glob}, and {grep}")));
    assert!(p.contains("https://code.claude.com/docs/en/claude_code_docs_map.md"));
    assert!(p.contains("https://platform.claude.com/llms.txt"));
}

#[test]
fn coco_guide_embedded_uses_find_grep_in_local_hint() {
    let read = ToolName::Read.as_str();
    let glob = ToolName::Glob.as_str();
    let grep = ToolName::Grep.as_str();
    let p = coco_guide_system_prompt(true);
    assert!(p.contains(&format!("{read}, `find`, and `grep`")));
    assert!(!p.contains(&format!("{read}, {glob}, and {grep}")));
}
