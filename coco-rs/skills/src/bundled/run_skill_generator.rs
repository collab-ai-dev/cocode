//! `/run-skill-generator` — run the skill-generator workflow to create or refine a SKILL.md. Mirrors claude-code's runSkillGenerator.ts.
//! No upstream runSkillGenerator.ts; coco-authored content migrated as-is.

pub const PROMPT: &str = r#"Run the skill-generator workflow to create or refine a SKILL.md.

Steps:
1. Ask the user what skill they want (name, purpose, when-to-use, side effects).
2. Search the codebase for related patterns and existing skills to avoid duplication.
3. Draft a SKILL.md at `.coco/skills/<skill-name>/SKILL.md` with proper frontmatter (name, description, when_to_use, allowed-tools, argument-hint, paths if applicable).
4. Show the draft to the user, iterate on feedback.
5. Once approved, write the file and tell the user how to invoke it (`/<skill-name>`).

If the user already has a skill they want to refine, read it first and propose specific edits with rationale.
"#;
