//! `/dream` — run the KAIROS auto-dream memory consolidation routine. Mirrors claude-code's dream.ts.
//! No upstream dream.ts in checkout (ships as gated .js); coco content migrated as-is.

pub const PROMPT: &str = r#"Run the KAIROS auto-dream memory consolidation routine.

Review recent session memory entries, identify patterns, contradictions, and stale information, and consolidate them into a coherent set of preserved memories. Drop information that's no longer relevant; merge duplicates; flag contradictions for the user to resolve.

Output a brief report listing what was preserved, merged, dropped, and flagged.
"#;
