use super::*;

#[test]
fn literal_prompt_context_preserves_text() {
    let context = PromptContext::build(PromptContextMode::literal(
        PromptContextLiteralSource::ConfigOverride,
        "custom system",
    ));

    assert_eq!(context.system_prompt(), "custom system");
    assert_eq!(context.sources.len(), 1);
    assert_eq!(context.sources[0].kind, PromptContextSourceKind::Literal);
    assert_eq!(
        context.sources[0].literal_source,
        Some(PromptContextLiteralSource::ConfigOverride)
    );
    assert_eq!(context.epoch.as_str().len(), 64);
}

#[test]
fn literal_prompt_context_is_utf8_safely_bounded() {
    let original = "界".repeat(MAX_LITERAL_SYSTEM_PROMPT_BYTES);
    let context = PromptContext::build(PromptContextMode::literal(
        PromptContextLiteralSource::Coordinator,
        original.as_str(),
    ));

    assert!(context.system_prompt().len() <= MAX_LITERAL_SYSTEM_PROMPT_BYTES);
    assert!(context.system_prompt().ends_with(LITERAL_TRUNCATED_MARKER));
    assert!(context.sources[0].truncated);
    assert_eq!(
        context.sources[0].literal_source,
        Some(PromptContextLiteralSource::Coordinator)
    );
    assert!(
        context.sources[0].rendered_size_bytes
            <= i64::try_from(MAX_LITERAL_SYSTEM_PROMPT_BYTES).expect("limit fits in i64")
    );
}

#[test]
fn compiled_prompt_context_bounds_without_flattening_cache_layout() {
    let mut prompt = crate::SystemPrompt::new();
    prompt.add_text("stable");
    prompt.add_cache_breakpoint();
    prompt.add_text("界".repeat(MAX_LITERAL_SYSTEM_PROMPT_BYTES));

    let context = PromptContext::build(PromptContextMode::compiled(&prompt));
    let parts = context.prompt.parts();

    assert!(context.system_prompt().len() <= MAX_LITERAL_SYSTEM_PROMPT_BYTES);
    assert_eq!(parts[0].cache_hint, crate::CacheHint::Breakpoint);
    assert!(context.sources[0].truncated);
    assert_eq!(context.sources[0].kind, PromptContextSourceKind::Compiled);
}

#[test]
fn default_workspace_context_includes_memory_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let memory_path = tmp.path().join("AGENTS.md");
    std::fs::write(&memory_path, "project rules").expect("write memory");

    let context = PromptContext::build(PromptContextMode::default_workspace(tmp.path()));

    assert!(
        context
            .system_prompt()
            .starts_with(DEFAULT_MAIN_SYSTEM_PROMPT)
    );
    assert!(context.system_prompt().contains("# "));
    assert!(context.system_prompt().contains("project rules"));
    assert!(
        context
            .sources
            .iter()
            .any(|source| source.path.as_ref() == Some(&memory_path))
    );
}

#[test]
fn default_workspace_context_bounds_large_memory_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("AGENTS.md"), "x".repeat(20_000)).expect("write memory");

    let context = PromptContext::build(PromptContextMode::default_workspace(tmp.path()));
    let memory_source = context
        .sources
        .iter()
        .find(|source| source.kind == PromptContextSourceKind::MemoryFile)
        .expect("memory source");

    assert!(memory_source.truncated);
    assert!(memory_source.rendered_size_bytes <= MAX_PROMPT_CONTEXT_SOURCE_BYTES as i64);
    assert!(context.system_prompt().contains("Memory file truncated"));
}

#[test]
fn default_workspace_context_bounds_aggregate_memory() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut cwd = tmp.path().to_path_buf();
    for idx in 0..8 {
        std::fs::write(
            cwd.join("AGENTS.md"),
            format!("{idx}{}", "x".repeat(20_000)),
        )
        .expect("write memory");
        cwd = cwd.join(format!("dir-{idx}"));
        std::fs::create_dir_all(&cwd).expect("create dir");
    }

    let context = PromptContext::build(PromptContextMode::default_workspace(&cwd));
    let rendered_memory_bytes: i64 = context
        .sources
        .iter()
        .filter(|source| source.kind == PromptContextSourceKind::MemoryFile)
        .map(|source| source.rendered_size_bytes)
        .sum();

    assert!(rendered_memory_bytes <= MAX_PROMPT_CONTEXT_MEMORY_BYTES as i64);
}
