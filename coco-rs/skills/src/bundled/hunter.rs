//! `/hunter` — run a deep bug-finding review on the current branch or specified files. Mirrors claude-code's hunter.ts.
//! No upstream hunter.ts; coco-authored content migrated as-is.

pub const PROMPT: &str = r#"Run a deep bug-finding review on the current branch or specified files.

Look for:
- Off-by-one errors, incorrect bounds checks, signed/unsigned mismatches
- Missing error handling, swallowed errors, panics on user input
- Race conditions, deadlocks, lost wakeups, TOCTOU issues
- Resource leaks (file handles, sockets, locks not released on error path)
- Privilege escalation, path traversal, injection (SQL, shell, HTML)
- Logic errors that pass tests but produce wrong output on edge cases

Report each finding with: location, severity (P0/P1/P2/P3), reproduction or proof, and suggested fix. Do NOT fix automatically — the user reviews first.
"#;
