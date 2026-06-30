use super::*;
use std::path::Path;

fn native_tools() -> FileMutationPromptTools {
    FileMutationPromptTools::native()
}

fn patch_tools() -> FileMutationPromptTools {
    FileMutationPromptTools {
        write_tool: coco_types::ToolName::ApplyPatch,
        edit_tool: coco_types::ToolName::ApplyPatch,
    }
}

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
        &[],
        native_tools(),
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
        &[],
        native_tools(),
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
fn system_prompt_renders_native_file_mutation_tools_by_default() {
    let p = build_system_prompt_section(
        SystemPromptVariant::Auto,
        Path::new("/m"),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        &[],
        native_tools(),
    );

    assert!(p.contains("Write tool"));
    assert!(!p.contains("apply_patch"));
}

#[test]
fn system_prompt_renders_apply_patch_file_mutation_tool() {
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
        &[],
        patch_tools(),
    );

    assert!(p.contains("apply_patch"));
    assert!(!p.contains("Write"));
    assert!(!p.contains("Edit"));
}

fn store(path: &str, mode: StoreMode, scope: StoreScope) -> MemoryStore {
    store_with_prompt_index(path, mode, scope, None)
}

fn store_with_prompt_index(
    path: &str,
    mode: StoreMode,
    scope: StoreScope,
    prompt_index: Option<&str>,
) -> MemoryStore {
    MemoryStore {
        path: coco_config::parse_memory_stores(&format!("[\"{path}\"]"))
            .into_iter()
            .next()
            .expect("absolute path")
            .path,
        mode,
        scope,
        mount: Some(
            std::path::Path::new(path)
                .file_name()
                .expect("name")
                .to_string_lossy()
                .into_owned(),
        ),
        prompt_index: prompt_index.map(str::to_string),
        prompt_index_max_bytes: None,
    }
}

#[test]
fn mounted_stores_render_rw_ro_and_user_sections() {
    let stores = vec![
        store("/mnt/team-rw", StoreMode::Rw, StoreScope::Team),
        store("/mnt/team-ro", StoreMode::Ro, StoreScope::Team),
        store("/mnt/user-priv", StoreMode::Rw, StoreScope::User),
    ];
    let p = build_system_prompt_section(
        SystemPromptVariant::Combined,
        Path::new("/m"),
        Some(Path::new("/m/team")),
        None,
        None,
        false,
        false,
        None,
        None,
        &stores,
        native_tools(),
    );
    assert!(p.contains("## Mounted memory stores"));
    assert!(p.contains("### Team stores (writable)"));
    assert!(p.contains("/m/team/team-rw/"));
    assert!(p.contains("(mount `team-rw`)"));
    assert!(p.contains("/m/team/team-rw/MEMORY.md"));
    assert!(!p.contains("/mnt/team-rw"));
    assert!(p.contains("### Team stores (read-only)"));
    assert!(p.contains("/m/team/team-ro/"));
    assert!(p.contains("do not write there because changes will not persist"));
    assert!(!p.contains("/mnt/team-ro"));
    assert!(p.contains("### Private store"));
    assert!(p.contains("/mnt/user-priv"));
}

#[test]
fn mounted_stores_use_prompt_index_targets_for_writable_team_stores() {
    let stores = vec![store_with_prompt_index(
        "/mnt/team-rw",
        StoreMode::Rw,
        StoreScope::Team,
        Some("index/MEMORY.md"),
    )];
    let p = build_system_prompt_section(
        SystemPromptVariant::Combined,
        Path::new("/m"),
        Some(Path::new("/m/team")),
        None,
        None,
        false,
        false,
        None,
        None,
        &stores,
        native_tools(),
    );
    assert!(p.contains("/m/team/team-rw/index/MEMORY.md"));
    assert!(!p.contains("/m/team/team-rw/MEMORY.md"));
    assert!(!p.contains("/mnt/team-rw"));
}

#[test]
fn team_only_variant_omits_private_directory_and_uses_team_store_targets() {
    let stores = vec![
        store_with_prompt_index(
            "/mnt/team-rw",
            StoreMode::Rw,
            StoreScope::Team,
            Some("index/MEMORY.md"),
        ),
        store("/mnt/team-ro", StoreMode::Ro, StoreScope::Team),
    ];
    let p = build_system_prompt_section(
        SystemPromptVariant::TeamOnly,
        Path::new("/m"),
        Some(Path::new("/m/team")),
        Some("- [private](private.md) — should not render"),
        Some("- [root team](root.md) — should not render"),
        false,
        false,
        None,
        Some("extra mounted index"),
        &stores,
        native_tools(),
    );

    assert!(p.starts_with("# Memory"));
    assert!(!p.contains("private directory at"));
    assert!(!p.contains("## MEMORY.md"));
    assert!(!p.contains("Team MEMORY.md"));
    assert!(p.contains("persistent, file-based team memory directory"));
    assert!(p.contains("There is no separate private memory directory"));
    assert!(p.contains("/m/team/team-rw/"));
    assert!(p.contains("/m/team/team-rw/index/MEMORY.md"));
    assert!(p.contains("read-only team memory at `/m/team/team-ro/`"));
    assert!(p.contains("extra mounted index"));
}

#[test]
fn team_only_read_only_stores_do_not_render_save_instructions() {
    let stores = vec![store("/mnt/team-ro", StoreMode::Ro, StoreScope::Team)];
    let p = build_system_prompt_section(
        SystemPromptVariant::TeamOnly,
        Path::new("/m"),
        Some(Path::new("/m/team")),
        None,
        None,
        false,
        false,
        None,
        None,
        &stores,
        native_tools(),
    );

    assert!(p.contains("read-only access to team memory"));
    assert!(p.contains("explain that memory is read-only in this session"));
    assert!(!p.contains("## How to save memories"));
}

#[test]
fn mounted_stores_omitted_in_auto_variant() {
    let stores = vec![store("/mnt/team-rw", StoreMode::Rw, StoreScope::Team)];
    let p = build_system_prompt_section(
        SystemPromptVariant::Auto,
        Path::new("/m"),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        &stores,
        native_tools(),
    );
    // Store prose is combined-only; Auto variant must not render it.
    assert!(!p.contains("## Mounted memory stores"));
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
        &[],
        native_tools(),
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
        &[],
        native_tools(),
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
        &[],
        native_tools(),
    );
    // Placeholder visible to the model when projectDir isn't resolvable.
    assert!(p.contains("<your sessions directory>"));
}

#[test]
fn kairos_variant_describes_daily_log_pattern() {
    let p = build_kairos_prompt(Path::new("/m"), false, false, None, native_tools());
    assert!(p.contains("# auto memory"));
    assert!(p.contains("daily log"));
    assert!(p.contains("YYYY-MM-DD.md"));
    assert!(p.contains("append-only"));
    // Default (skip_index = false): the orientation block is present.
    assert!(p.contains("## MEMORY.md"));
}

#[test]
fn kairos_skip_index_omits_memory_md_block() {
    let p = build_kairos_prompt(Path::new("/m"), true, false, None, native_tools());
    assert!(!p.contains("## MEMORY.md"));
}

#[test]
fn kairos_searching_past_context_appends_block() {
    let p = build_kairos_prompt(
        Path::new("/m"),
        false,
        true,
        Some(Path::new("/transcripts")),
        native_tools(),
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
        native_tools(),
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
fn extract_prompt_renders_native_file_mutation_tools() {
    let p = build_extract_prompt(5, "", false, false, native_tools());

    assert!(p.contains("Write for creating"));
    assert!(p.contains("Edit for updating"));
    assert!(p.contains("Edit requires a prior Read"));
    assert!(!p.contains("apply_patch"));
}

#[test]
fn extract_prompt_renders_apply_patch_without_native_tool_names() {
    let p = build_extract_prompt(5, "", false, false, patch_tools());

    assert!(p.contains("apply_patch for creating or updating"));
    assert!(p.contains("issue all apply_patch calls in parallel"));
    assert!(!p.contains("Write"));
    assert!(!p.contains("Edit"));
    assert!(!p.contains("Edit requires a prior Read"));
}

#[test]
fn extract_prompt_omits_manifest_section_when_empty() {
    // When `existingMemories` is empty the whole `## Existing memory files`
    // section is dropped. Rust caller passes `""` from `format_memory_manifest`
    // when the dir is empty.
    let p = build_extract_prompt(5, "", false, false, native_tools());
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
    let p = build_extract_prompt(10, "", false, true, native_tools());
    assert!(p.contains("avoid saving sensitive data within shared team memories"));
    assert!(p.contains("<scope>always private</scope>"));
}

#[test]
fn dream_prompt_includes_four_phases() {
    let p = build_dream_prompt(Path::new("/m"), Path::new("/p"), &[], false, native_tools());
    assert!(p.contains("Phase 1 — Orient"));
    assert!(p.contains("Phase 2 — Gather recent signal"));
    assert!(p.contains("Phase 3 — Consolidate"));
    assert!(p.contains("Phase 4 — Prune and index"));
    // Memory + transcript paths substituted into the body.
    assert!(p.contains("Memory directory: `/m`"));
    assert!(p.contains("Session transcripts: `/p`"));
    assert!(p.contains("Session logs"));
    assert!(p.contains("logs/YYYY/MM/DD/<id>-<title>.md"));
    assert!(p.contains("Reconcile memories against CLAUDE.md"));
    assert!(!p.contains("Team memory (`team/` subdirectory)"));
}

#[test]
fn dream_prompt_includes_team_guidance_when_enabled() {
    let p = build_dream_prompt(Path::new("/m"), Path::new("/p"), &[], true, native_tools());
    assert!(p.contains("Team memory (`team/` subdirectory)"));
    assert!(p.contains("ls team/"));
    assert!(p.contains("be conservative pruning `team/`"));
}

#[test]
fn dream_prompt_includes_bash_sandbox_constraint_in_extra_block() {
    // The `extra` block always includes the Bash sandbox constraint
    // reminder so the dream subagent doesn't waste turns on
    // unsupported writes/redirects that the dream canUseTool would deny.
    let p = build_dream_prompt(Path::new("/m"), Path::new("/p"), &[], false, native_tools());
    assert!(
        p.contains("Tool constraints for this run"),
        "expected bash sandbox constraint in dream prompt's extra block, got: {p}"
    );
    assert!(p.contains("Bash is restricted to read-only"));
    assert!(p.contains("plus deleting `.md` paths inside the memory directory"));
}

#[test]
fn dream_prompt_renders_apply_patch_directory_blurb() {
    let p = build_dream_prompt(Path::new("/m"), Path::new("/p"), &[], false, patch_tools());

    assert!(p.contains("write to it directly with apply_patch"));
    assert!(!p.contains("Write"));
    assert!(!p.contains("Edit"));
}

#[test]
fn dream_prompt_appends_session_list_after_constraint() {
    let p = build_dream_prompt(
        Path::new("/m"),
        Path::new("/p"),
        &["s1".into(), "s2".into()],
        false,
        native_tools(),
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
