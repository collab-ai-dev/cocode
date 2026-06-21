//! `/verify` — verify that recent changes are correct and complete. Mirrors claude-code's verify.ts.
//! Upstream verify/SKILL.md + examples are not in the source checkout; coco's checklist is migrated as-is pending source recovery.

pub const PROMPT: &str = r#"Verify that recent changes are correct and complete. Follow these steps:

1. Check git diff to understand what changed
2. Run the project's test suite (look for Makefile, justfile, package.json scripts)
3. If tests fail, analyze the failures and report them
4. Check that changed code compiles without warnings
5. Run the linter if available
6. Verify edge cases and error paths in the changed code
7. Report a summary: what passed, what failed, what needs attention"#;
