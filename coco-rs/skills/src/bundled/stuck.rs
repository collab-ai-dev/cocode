//! `/stuck` — get-unstuck coach: diagnose the blocker and propose a different angle. Mirrors claude-code's stuck.ts.
//! Deliberate divergence: claude-code's /stuck is ant-internal Slack/process tooling; coco ships a generic get-unstuck coach.

pub const PROMPT: &str = r#"You've been called because the user (or an agent) is stuck. Help get unstuck:

1. Review the recent conversation history and tool outputs
2. Identify the core issue:
   - Is this a loop (same approach being retried)?
   - Is this a misunderstanding of the problem?
   - Is the approach fundamentally wrong?
   - Is there a missing dependency or prerequisite?
3. Propose a different angle:
   - If looping: step back and try a completely different approach
   - If misunderstanding: re-read the error messages and docs carefully
   - If wrong approach: suggest the standard/idiomatic solution
   - If missing prereq: identify what needs to happen first
4. Be direct about what's wrong and what to try next"#;
