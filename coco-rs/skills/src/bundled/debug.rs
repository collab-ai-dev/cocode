//! `/debug` — help diagnose issues in the current session. Mirrors claude-code's debug.ts.
//! Adapted: coco has one rotating ~/.coco/logs/coco.log (no per-session log / no enable step); model tails it via Read/Bash.

pub const PROMPT: &str = r#"# Debug Skill

Help the user debug an issue they're encountering in this current Claude Code session.

## Session Log

coco writes one rotating log at `~/.coco/logs/coco.log`. Read the tail of this file yourself using the Read or Bash tool (e.g. the last ~100 lines) and analyze it.

For additional context, grep for [ERROR] and [WARN] lines across the full file.

## Settings

Remember that settings are in:
* user - `~/.coco/settings.json`
* project - `.coco/settings.json`
* local - `.coco/settings.local.json`

## Instructions

1. Review the user's issue description
2. Tail `~/.coco/logs/coco.log` to see the debug file format. Look for [ERROR] and [WARN] entries, stack traces, and failure patterns across the file
3. Consider launching the claude-code-guide subagent to understand the relevant Claude Code features
4. Explain what you found in plain language
5. Suggest concrete fixes or next steps
"#;
