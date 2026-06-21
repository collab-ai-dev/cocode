use super::*;
use std::path::Path;

#[test]
fn auto_variant_includes_individual_types_and_index() {
    let p = build_system_prompt_section(
        SystemPromptVariant::Auto,
        Path::new("/m"),
        None,
        Some("- [a](a.md) — h"),
        None,
        false,
        false,
        None,
        None,
    );
    assert!(p.contains("# auto memory"));
    assert!(p.contains("<types>"));
    assert!(p.contains("<name>user</name>"));
    assert!(p.contains("## MEMORY.md"));
    assert!(p.contains("- [a](a.md) — h"));
    // Two-step instructions present when skip_index = false.
    assert!(p.contains("two-step process"));
    // Searching-past-context off by default.
    assert!(!p.contains("Searching past context"));
    // Memory-vs-other-persistence guidance is shared across variants.
    assert!(p.contains("Memory and other forms of persistence"));
    // Combined-only block must not appear in Auto.
    assert!(!p.contains("## Memory scope"));
    assert!(!p.contains("<scope>always private</scope>"));
}

#[test]
fn combined_variant_includes_scope_taxonomy_and_team_block() {
    let p = build_system_prompt_section(
        SystemPromptVariant::Combined,
        Path::new("/m"),
        Some(Path::new("/m/team")),
        Some("- [a](a.md) — h"),
        Some("- [t](t.md) — team hook"),
        false,
        false,
        None,
        None,
    );
    assert!(p.contains("private directory at `/m`"));
    assert!(p.contains("team directory at `/m/team`"));
    assert!(p.contains("## Memory scope"));
    assert!(p.contains("Team MEMORY.md"));
    assert!(p.contains("- [t](t.md) — team hook"));
    // Combined uses the scope-tagged taxonomy.
    assert!(p.contains("<scope>always private</scope>"));
    // Sensitive-data addendum kicks in only in combined mode.
    assert!(p.contains("avoid saving sensitive data within shared team memories"));
    // Combined-specific when-to-access intro line.
    assert!(p.contains("personal or team"));
}

#[test]
fn skip_index_omits_two_step_block() {
    let p = build_system_prompt_section(
        SystemPromptVariant::Auto,
        Path::new("/m"),
        None,
        None,
        None,
        true,
        false,
        None,
        None,
    );
    assert!(!p.contains("two-step process"));
}

#[test]
fn searching_past_context_substitutes_memory_and_transcript_dir() {
    let p = build_system_prompt_section(
        SystemPromptVariant::Auto,
        Path::new("/mem/dir"),
        None,
        None,
        None,
        false,
        true,
        Some(Path::new("/sess/proj")),
        None,
    );
    assert!(p.contains("## Searching past context"));
    assert!(p.contains("/mem/dir"));
    assert!(p.contains("/sess/proj"));
    assert!(p.contains("narrow search terms"));
}

#[test]
fn searching_past_context_keeps_placeholder_when_transcript_unset() {
    let p = build_system_prompt_section(
        SystemPromptVariant::Auto,
        Path::new("/m"),
        None,
        None,
        None,
        false,
        true,
        None,
        None,
    );
    // Placeholder visible to the model when projectDir isn't resolvable.
    assert!(p.contains("<your sessions directory>"));
}

#[test]
fn kairos_variant_describes_daily_log_pattern() {
    let p = build_kairos_prompt(Path::new("/m"), false, false, None);
    assert!(p.contains("# auto memory"));
    assert!(p.contains("daily log"));
    assert!(p.contains("YYYY-MM-DD.md"));
    assert!(p.contains("append-only"));
    // Default (skip_index = false): the orientation block is present.
    assert!(p.contains("## MEMORY.md"));
}

#[test]
fn kairos_skip_index_omits_memory_md_block() {
    let p = build_kairos_prompt(Path::new("/m"), true, false, None);
    assert!(!p.contains("## MEMORY.md"));
}

#[test]
fn kairos_searching_past_context_appends_block() {
    let p = build_kairos_prompt(
        Path::new("/m"),
        false,
        true,
        Some(Path::new("/transcripts")),
    );
    assert!(p.contains("## Searching past context"));
    assert!(p.contains("/transcripts"));
}

#[test]
fn extract_prompt_includes_manifest_and_message_count() {
    // Manifest is now the line list only (no header). Builder wraps
    // it with `## Existing memory files` + the trailing nudge.
    let p = build_extract_prompt(
        40,
        "- [project] foo.md (2026-05-09T08:00:00.000Z): hook",
        false,
        false,
    );
    // The count appears twice — both the "most recent" line and the
    // budget-reminder line should reflect it.
    assert!(p.contains("most recent ~40 messages"));
    assert!(p.contains("last ~40 messages"));
    assert!(p.contains("## Existing memory files"));
    assert!(p.contains("- [project] foo.md"));
    assert!(p.contains("Check this list before writing"));
    assert!(p.contains("turn budget"));
    assert!(!p.contains("{MESSAGE_COUNT}"));
}

#[test]
fn extract_prompt_omits_manifest_section_when_empty() {
    // When `existingMemories` is empty the whole `## Existing memory files`
    // section is dropped. Rust caller passes `""` from `format_memory_manifest`
    // when the dir is empty.
    let p = build_extract_prompt(5, "", false, false);
    assert!(
        !p.contains("Existing memory files"),
        "expected manifest section to be omitted entirely when input is empty, got: {p}"
    );
    assert!(
        !p.contains("Check this list before writing"),
        "trailing nudge should also be dropped when no manifest"
    );
}

#[test]
fn extract_combined_includes_team_secret_addendum() {
    let p = build_extract_prompt(10, "", false, true);
    assert!(p.contains("avoid saving sensitive data within shared team memories"));
    assert!(p.contains("<scope>always private</scope>"));
}

#[test]
fn dream_prompt_includes_four_phases() {
    let p = build_dream_prompt(Path::new("/m"), Path::new("/p"), &[]);
    assert!(p.contains("Phase 1 — Orient"));
    assert!(p.contains("Phase 2 — Gather recent signal"));
    assert!(p.contains("Phase 3 — Consolidate"));
    assert!(p.contains("Phase 4 — Prune and index"));
    // Memory + transcript paths substituted into the body.
    assert!(p.contains("Memory directory: `/m`"));
    assert!(p.contains("Session transcripts: `/p`"));
}

#[test]
fn dream_prompt_includes_bash_readonly_constraint_in_extra_block() {
    // The `extra` block always includes the read-only Bash constraint
    // reminder so the dream subagent doesn't waste turns on
    // writes/redirects that `createAutoMemCanUseTool` would deny.
    let p = build_dream_prompt(Path::new("/m"), Path::new("/p"), &[]);
    assert!(
        p.contains("Tool constraints for this run"),
        "expected bash-readonly constraint in dream prompt's extra block, got: {p}"
    );
    assert!(p.contains("Bash is restricted to read-only"));
}

#[test]
fn dream_prompt_appends_session_list_after_constraint() {
    let p = build_dream_prompt(
        Path::new("/m"),
        Path::new("/p"),
        &["s1".into(), "s2".into()],
    );
    assert!(p.contains("Tool constraints for this run"));
    assert!(p.contains("Sessions since last consolidation (2)"));
    assert!(p.contains("- s1"));
    assert!(p.contains("- s2"));
}

#[test]
fn session_template_has_ten_section_headers() {
    // `DEFAULT_SESSION_MEMORY_TEMPLATE` has exactly 10 H1 sections:
    // Session Title, Current State, Task specification, Files and
    // Functions, Workflow, Errors & Corrections, Codebase and System
    // Documentation, Learnings, Key results, Worklog.
    let template = build_session_memory_template();
    let headers = template.lines().filter(|l| l.starts_with("# ")).count();
    assert_eq!(headers, 10);
    assert!(template.contains("# Session Title"));
    assert!(template.contains("# Current State"));
    assert!(template.contains("# Worklog"));
    // Italic descriptions must survive — they are template instructions
    // the session-memory update prompt explicitly forbids deleting.
    assert!(template.contains("_A short and distinctive 5-10 word"));
}

#[test]
fn session_memory_update_prompt_emphasizes_structure_preservation() {
    let p = build_session_memory_update_prompt(
        "# Session Title\n_x_",
        Path::new("/n.md"),
        None,
        2_000,
        12_000,
    );
    assert!(p.contains("CRITICAL RULES FOR EDITING"));
    assert!(p.contains("italic _section description_"));
    assert!(p.contains("STRUCTURE PRESERVATION REMINDER"));
    assert!(p.contains("/n.md"));
}

#[test]
fn session_memory_update_prompt_appends_oversized_section_warning() {
    // When a section exceeds the per-section budget
    // (`generateSectionReminders`), the prompt appends a sorted list of
    // the oversized sections so the model knows to condense them. Use a
    // 50-byte section limit (≈12 tokens) so the body easily exceeds it.
    let big_body = "x".repeat(2_000);
    let notes = format!("# Session Title\n_hint_\n\n# Worklog\n{big_body}\n");
    let p = build_session_memory_update_prompt(
        &notes,
        Path::new("/n.md"),
        None,
        /*per_section_tokens=*/ 12,
        /*total_tokens=*/ 1_000_000,
    );
    assert!(
        p.contains("MUST be condensed"),
        "expected oversized-section warning, got: {p}"
    );
    assert!(
        p.contains("# Worklog"),
        "expected the oversized section name in the warning"
    );
}

#[test]
fn session_memory_update_prompt_appends_total_budget_warning() {
    let notes = "# x\n_y_\n".to_string() + &"a".repeat(60_000);
    let p = build_session_memory_update_prompt(
        &notes,
        Path::new("/n.md"),
        None,
        /*per_section_tokens=*/ 1_000_000,
        /*total_tokens=*/ 1_000,
    );
    assert!(
        p.contains("CRITICAL"),
        "expected total-budget CRITICAL warning, got: {p}"
    );
    assert!(p.contains("exceeds the maximum"));
}

#[test]
fn session_memory_update_prompt_uses_custom_template_with_substitution() {
    let template =
        "TEMPLATE: notes={{currentNotes}} path={{notesPath}} unknown={{notDefined}}".to_string();
    let p = build_session_memory_update_prompt(
        "abc",
        Path::new("/some/path.md"),
        Some(&template),
        2_000,
        12_000,
    );
    assert!(p.contains("notes=abc"));
    assert!(p.contains("path=/some/path.md"));
    // Unrecognised vars are left as-is so user content with
    // {{var}} syntax doesn't get clobbered.
    assert!(p.contains("unknown={{notDefined}}"));
}
