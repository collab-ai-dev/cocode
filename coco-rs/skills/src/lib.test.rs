use std::path::PathBuf;

use super::*;

fn test_skill(name: &str, description: &str, prompt: &str, source: SkillSource) -> SkillDefinition {
    SkillDefinition {
        name: name.into(),
        display_name: None,
        description: description.into(),
        prompt: prompt.into(),
        source,
        aliases: vec![],
        allowed_tools: None,
        model: None,
        model_role: None,
        when_to_use: None,
        argument_names: vec![],
        paths: vec![],
        effort: None,
        context: SkillContext::Inline,
        agent: None,
        version: None,
        disabled: false,
        hooks: None,
        argument_hint: None,
        user_invocable: true,
        disable_model_invocation: false,
        shell: None,
        content_length: prompt.len() as i64,
        has_user_specified_description: true,
        progress_message: Some("running".to_string()),
        is_hidden: false,
        gated_by: None,
        files: std::collections::HashMap::new(),
        skill_root: None,
    }
}

#[test]
fn test_skill_manager_register_and_get() {
    let mgr = SkillManager::new();
    let mut skill = test_skill(
        "commit",
        "Create a git commit",
        "Create a commit with the staged changes.",
        SkillSource::Bundled,
    );
    skill.allowed_tools = Some(vec!["Bash".into(), "Read".into()]);
    mgr.register(skill);

    assert_eq!(mgr.len(), 1);
    let skill = mgr.get("commit").unwrap();
    assert_eq!(skill.description, "Create a git commit");
    assert!(matches!(skill.source, SkillSource::Bundled));
}

#[test]
fn test_skill_not_found() {
    let mgr = SkillManager::new();
    assert!(mgr.get("nonexistent").is_none());
}

#[test]
fn test_skill_from_project() {
    let mgr = SkillManager::new();
    let mut skill = test_skill(
        "deploy",
        "Deploy to production",
        "Run the deploy script.",
        SkillSource::Project {
            path: "/project/.coco/skills/deploy/SKILL.md".into(),
        },
    );
    skill.model = Some("anthropic/claude-opus-4-7".into());
    mgr.register(skill);

    let skill = mgr.get("deploy").unwrap();
    assert!(matches!(skill.source, SkillSource::Project { .. }));
    assert_eq!(skill.model.as_deref(), Some("anthropic/claude-opus-4-7"));
}

#[test]
fn test_skill_lookup_by_alias() {
    let mgr = SkillManager::new();
    let mut skill = test_skill(
        "commit",
        "Create a git commit",
        "Create a commit.",
        SkillSource::Bundled,
    );
    skill.aliases = vec!["ci".into(), "gc".into()];
    mgr.register(skill);

    assert!(mgr.get("commit").is_some());
    assert!(mgr.get("ci").is_some());
    assert!(mgr.get("gc").is_some());
    assert_eq!(mgr.get("ci").unwrap().name, "commit");
    assert!(mgr.get("nonexistent").is_none());
}

#[test]
fn test_load_from_markdown_basic() {
    // No frontmatter, no heading: file body is preserved verbatim and
    // the skill name comes from the file stem.
    // No frontmatter description → description is auto-extracted from the
    // first body line via `extractDescriptionFromMarkdown`, and
    // `has_user_specified_description` is false.
    let content = "Run the deployment pipeline.\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/deploy.md")).unwrap();

    assert_eq!(skill.name, "deploy");
    assert_eq!(skill.prompt, "Run the deployment pipeline.");
    assert_eq!(skill.description, "Run the deployment pipeline.");
    assert!(!skill.has_user_specified_description);
    assert!(skill.allowed_tools.is_none());
}

#[test]
fn test_load_from_markdown_with_frontmatter() {
    let content = "\
---
description: Review a pull request
allowed-tools: Bash, Read, Grep
model: anthropic/claude-opus-4-7
model_role: review
---

Carefully review the PR for correctness and style.
Check for bugs, security issues, and performance.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/review-pr.md")).unwrap();

    assert_eq!(skill.name, "review-pr");
    assert_eq!(skill.description, "Review a pull request");
    assert_eq!(
        skill.allowed_tools,
        Some(vec![
            "Bash".to_string(),
            "Read".to_string(),
            "Grep".to_string(),
        ])
    );
    assert_eq!(skill.model.as_deref(), Some("anthropic/claude-opus-4-7"));
    assert_eq!(skill.model_role, Some(coco_types::ModelRole::Review));
    assert!(skill.prompt.contains("Carefully review the PR"));
    assert!(skill.prompt.contains("security issues"));
}

#[test]
fn test_load_from_markdown_allowed_tools_underscore() {
    let content = "\
---
allowed_tools: Bash, Read
---

Do things.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/test.md")).unwrap();
    assert_eq!(
        skill.allowed_tools,
        Some(vec!["Bash".to_string(), "Read".to_string()])
    );
}

#[test]
fn test_load_from_markdown_empty_frontmatter() {
    let content = "---\n---\n\nDo the thing.\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/test-skill.md")).unwrap();

    assert_eq!(skill.name, "test-skill");
    assert_eq!(skill.prompt, "Do the thing.");
    // Body fallback: extracted description = first non-empty body line.
    assert_eq!(skill.description, "Do the thing.");
    assert!(!skill.has_user_specified_description);
}

#[test]
fn test_load_from_markdown_no_frontmatter_loads_body_as_prompt() {
    // A file without frontmatter is not an error — the whole
    // file becomes the skill body and the name comes from the file stem.
    let content = "This has no frontmatter, just body text.\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/bare.md")).unwrap();
    assert_eq!(skill.name, "bare");
    assert_eq!(skill.prompt, "This has no frontmatter, just body text.");
    // Body fallback supplies the description.
    assert_eq!(
        skill.description,
        "This has no frontmatter, just body text."
    );
    assert!(!skill.has_user_specified_description);
}

#[test]
fn test_load_from_markdown_empty_loads_empty_skill() {
    // Empty content yields an empty body, not an error. The name is still
    // derived from the file path. Description falls back
    // to the default label `'Skill'` (when content has no non-empty lines).
    let skill = parse_skill_markdown("", Path::new("/tmp/empty.md")).unwrap();
    assert_eq!(skill.name, "empty");
    assert!(skill.prompt.is_empty());
    assert_eq!(skill.description, "Skill");
    assert!(!skill.has_user_specified_description);
}

#[test]
fn test_load_from_markdown_aliases() {
    let content = "\
---
aliases: d, dep
---

Deploy app.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/deploy.md")).unwrap();
    assert_eq!(skill.aliases, vec!["d", "dep"]);
}

#[test]
fn test_load_from_markdown_hooks_json() {
    let content = "\
---
hooks: {\"PreToolUse\": \"echo hi\"}
---

Test skill.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/test.md")).unwrap();
    assert!(skill.hooks.is_some());
    let hooks = skill.hooks.unwrap();
    assert!(hooks.is_object());
    assert!(hooks.get("PreToolUse").is_some());
}

#[test]
fn test_load_from_markdown_hooks_string() {
    let content = "\
---
hooks: simple-hook
---

Test skill.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/test.md")).unwrap();
    assert!(skill.hooks.is_some());
    assert_eq!(
        skill.hooks.unwrap(),
        serde_json::Value::String("simple-hook".to_string())
    );
}

#[test]
fn test_load_from_markdown_shell_string() {
    let content = "\
---
shell: bash
---

Test skill.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/test.md")).unwrap();
    assert_eq!(
        skill.shell,
        Some(serde_json::Value::String("bash".to_string()))
    );
}

#[test]
fn test_load_from_markdown_shell_json() {
    let content = "\
---
shell: {\"type\": \"powershell\"}
---

Test skill.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/test.md")).unwrap();
    assert!(skill.shell.is_some());
    let shell = skill.shell.unwrap();
    assert!(shell.is_object());
}

#[test]
fn test_load_from_markdown_user_invocable_false() {
    let content = "\
---
user-invocable: false
---

Internal skill.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/internal.md")).unwrap();
    assert!(!skill.user_invocable);
}

#[test]
fn test_load_from_markdown_disable_model_invocation() {
    let content = "\
---
disable-model-invocation: true
---

Debug skill.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/debug.md")).unwrap();
    assert!(skill.disable_model_invocation);
}

#[test]
fn test_discover_skill_md_directory_format() {
    let dir = tempfile::tempdir().unwrap();

    // Create SKILL.md directory format: my-skill/SKILL.md
    let skill_dir = dir.path().join("my-skill");
    std::fs::create_dir(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: My skill\n---\nDo stuff.\n",
    )
    .unwrap();

    let skills = discover_skills(&[dir.path().to_path_buf()]);
    assert_eq!(skills.len(), 1);
    // Name always comes from the directory.
    assert_eq!(skills[0].name, "my-skill");
    assert_eq!(skills[0].description, "My skill");
}

#[test]
fn test_discover_skill_md_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();

    let skill_dir = dir.path().join("case-test");
    std::fs::create_dir(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("skill.md"), "# heading\n\nSkill content.\n").unwrap();

    let skills = discover_skills(&[dir.path().to_path_buf()]);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "case-test");
}

#[test]
fn test_discover_skills_ignores_flat_md_in_skills_format() {
    let dir = tempfile::tempdir().unwrap();

    // Flat .md file should be ignored in SkillMdOnly format
    std::fs::write(dir.path().join("flat.md"), "# flat\n\nFlat skill.\n").unwrap();

    let skills = discover_skills(&[dir.path().to_path_buf()]);
    assert!(skills.is_empty());
}

#[test]
fn test_discover_skills_legacy_format_flat_md() {
    let dir = tempfile::tempdir().unwrap();

    // Legacy format supports flat .md files
    std::fs::write(dir.path().join("flat.md"), "# flat\n\nFlat skill.\n").unwrap();

    let skills = discover_skills_with_format(&[dir.path().to_path_buf()], SkillDirFormat::Legacy);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "flat");
}

#[test]
fn test_discover_skills_deduplicates_by_path() {
    let dir = tempfile::tempdir().unwrap();

    let skill_dir = dir.path().join("my-skill");
    std::fs::create_dir(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# heading\n\nContent.\n").unwrap();

    // Discover the same directory twice - should deduplicate
    let skills = discover_skills(&[dir.path().to_path_buf(), dir.path().to_path_buf()]);
    assert_eq!(skills.len(), 1);
}

#[test]
fn test_discover_skills_nonexistent_dir() {
    let skills = discover_skills(&[PathBuf::from("/nonexistent/path/xyz")]);
    assert!(skills.is_empty());
}

#[test]
fn test_load_from_dirs_with_legacy() {
    let mgr = SkillManager::new();
    let commands_path = tempfile::tempdir().unwrap();
    let actual_cmd_dir = commands_path.path().join("commands");
    std::fs::create_dir(&actual_cmd_dir).unwrap();
    std::fs::write(
        actual_cmd_dir.join("legacy.md"),
        "# legacy\n\nLegacy skill.\n",
    )
    .unwrap();
    mgr.load_from_dirs(&[actual_cmd_dir]);

    assert_eq!(mgr.len(), 0);
    assert!(mgr.get("legacy").is_none());
}

#[test]
fn test_load_skill_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("greet.md");
    std::fs::write(&path, "---\ndescription: Say hello\n---\nHello world.\n").unwrap();

    let skill = load_skill_from_file(&path).unwrap();
    assert_eq!(skill.name, "greet");
    assert_eq!(skill.description, "Say hello");
    assert_eq!(skill.prompt, "Hello world.");
}

#[test]
fn test_load_from_markdown_extended_frontmatter() {
    // Frontmatter `arguments` is whitespace-separated (see
    // `parseArgumentNames`). Comma-separation is the legacy disk format;
    // we keep the legacy alias keys but the canonical key is `arguments`.
    let content = "\
---
description: Deploy to production
when-to-use: When the user asks to deploy
arguments: env region
allowed-tools: Bash, Read
paths: src/**/*.rs, deploy/**
effort: high
context: fork
agent: general-purpose
version: 1.2.0
disabled: false
---

Run the deployment pipeline for the specified environment.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/deploy.md")).unwrap();

    assert_eq!(skill.name, "deploy");
    assert_eq!(skill.description, "Deploy to production");
    assert_eq!(
        skill.when_to_use.as_deref(),
        Some("When the user asks to deploy")
    );
    assert_eq!(skill.argument_names, vec!["env", "region"]);
    // Trailing `/**` is stripped because the `ignore` library treats a bare
    // path as matching both the path itself and its descendants.
    assert_eq!(skill.paths, vec!["src/**/*.rs", "deploy"]);
    assert_eq!(skill.effort, Some(coco_types::ReasoningEffort::High));
    assert_eq!(skill.context, SkillContext::Fork);
    assert_eq!(skill.agent.as_deref(), Some("general-purpose"));
    assert_eq!(skill.version.as_deref(), Some("1.2.0"));
    assert!(!skill.disabled);
}

#[test]
fn test_disabled_skill_skipped_in_discovery() {
    let dir = tempfile::tempdir().unwrap();

    let active_dir = dir.path().join("active");
    std::fs::create_dir(&active_dir).unwrap();
    std::fs::write(active_dir.join("SKILL.md"), "Active skill.\n").unwrap();

    let disabled_dir = dir.path().join("disabled");
    std::fs::create_dir(&disabled_dir).unwrap();
    std::fs::write(
        disabled_dir.join("SKILL.md"),
        "---\ndisabled: true\n---\nDisabled skill.\n",
    )
    .unwrap();

    let skills = discover_skills(&[dir.path().to_path_buf()]);
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "active");
}

// ── Brace expansion tests ──

#[test]
fn test_expand_braces_no_braces() {
    assert_eq!(expand_braces("*.rs"), vec!["*.rs"]);
}

#[test]
fn test_expand_braces_simple() {
    let mut result = expand_braces("*.{ts,tsx}");
    result.sort();
    assert_eq!(result, vec!["*.ts", "*.tsx"]);
}

#[test]
fn test_expand_braces_nested() {
    let mut result = expand_braces("{a,{b,c}}");
    result.sort();
    assert_eq!(result, vec!["a", "b", "c"]);
}

#[test]
fn test_expand_braces_with_prefix_suffix() {
    let mut result = expand_braces("src/*.{js,ts}");
    result.sort();
    assert_eq!(result, vec!["src/*.js", "src/*.ts"]);
}

#[test]
fn test_expand_braces_unclosed() {
    // Unclosed brace returns as-is
    assert_eq!(expand_braces("*.{ts"), vec!["*.{ts"]);
}

// ── Token-budgeted listing tests ──

#[test]
fn test_inject_skill_listing_empty() {
    let result = inject_skill_listing(&[], 8000, &coco_config::SkillOverrideTiers::default());
    assert!(result.listing.is_empty());
    assert_eq!(result.included, 0);
    assert_eq!(result.total, 0);
}

#[test]
fn test_inject_skill_listing_includes_bundled() {
    let skill = test_skill("commit", "Create a commit", "prompt", SkillSource::Bundled);
    let refs: Vec<&SkillDefinition> = vec![&skill];
    let result = inject_skill_listing(&refs, 8000, &coco_config::SkillOverrideTiers::default());

    assert!(result.listing.contains("/commit"));
    assert!(result.listing.contains("Create a commit"));
    assert_eq!(result.included, 1);
    assert_eq!(result.total, 1);
}

#[test]
fn test_inject_skill_listing_budget_enforced() {
    let bundled = test_skill("commit", "Create a commit", "p", SkillSource::Bundled);
    let user = test_skill(
        "long-skill",
        &"x".repeat(1000),
        "p",
        SkillSource::User {
            path: "/tmp/s.md".into(),
        },
    );
    let refs: Vec<&SkillDefinition> = vec![&bundled, &user];
    // Budget too small for user skill
    let result = inject_skill_listing(&refs, 100, &coco_config::SkillOverrideTiers::default());
    assert_eq!(result.included, 1); // only bundled
    assert_eq!(result.total, 2);
}

#[test]
fn test_inject_skill_listing_with_when_to_use() {
    let mut skill = test_skill("test", "Description", "p", SkillSource::Bundled);
    skill.when_to_use = Some("When doing X".to_string());
    let refs: Vec<&SkillDefinition> = vec![&skill];
    let result = inject_skill_listing(&refs, 8000, &coco_config::SkillOverrideTiers::default());

    assert!(result.listing.contains("When doing X"));
}

#[test]
fn test_inject_skill_listing_multibyte_description_truncates_safely() {
    // A long multi-byte description must truncate on a char boundary, not a
    // byte index — the old `&s[..MAX-3]` form panicked mid-codepoint here.
    let long_desc = "日".repeat(400); // 400 chars / 1200 bytes, over the 250 cap
    let skill = test_skill("multibyte", &long_desc, "p", SkillSource::Bundled);
    let refs: Vec<&SkillDefinition> = vec![&skill];
    let result = inject_skill_listing(
        &refs,
        1_000_000,
        &coco_config::SkillOverrideTiers::default(),
    );
    assert!(result.listing.contains("multibyte"));
    assert!(
        result.listing.contains("..."),
        "long description should be truncated"
    );
}

// ── Managed skills path tests ──

#[test]
fn test_get_skill_paths_includes_managed() {
    let paths = get_skill_paths(Path::new("/home/user/.coco"), Path::new("/project"));
    assert!(paths.len() >= 4);
    // First should be managed
    let managed = &paths[0];
    assert!(
        managed.to_string_lossy().contains("claude-code")
            || managed.to_string_lossy().contains("ClaudeCode")
    );
}

#[test]
fn test_get_skill_paths_order() {
    let paths = get_skill_paths(Path::new("/home/user/.coco"), Path::new("/project"));
    assert_eq!(paths[1], PathBuf::from("/home/user/.coco/skills"));
    assert_eq!(paths[2], PathBuf::from("/project/.coco/skills"));
    assert!(
        !paths
            .iter()
            .skip(1)
            .any(|p| p.to_string_lossy().contains(".claude"))
    );
}

// ── Paths with brace expansion ──

#[test]
fn test_frontmatter_paths_brace_expansion() {
    let content = "\
---
paths: *.{ts,tsx}, src/**/*.{js,jsx}
---

Test.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/test.md")).unwrap();
    assert!(skill.paths.contains(&"*.ts".to_string()));
    assert!(skill.paths.contains(&"*.tsx".to_string()));
    assert!(skill.paths.contains(&"src/**/*.js".to_string()));
    assert!(skill.paths.contains(&"src/**/*.jsx".to_string()));
}

// ── R7-T10: discover_skill_dirs_for_paths ──
//
// Walks up from each file path collecting `<ancestor>/.coco/skills/`
// directories that exist. The walk stops at (but excludes) cwd, since
// cwd-level skills are loaded at startup.
// The tests below cover the core walk, the cwd boundary, deepest-first
// ordering, and the missing-dir fast path.

#[test]
fn test_discover_skill_dirs_finds_nested() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    // Create a nested project with a skills dir at the inner level only.
    let project = cwd.join("project");
    let inner = project.join("subdir");
    std::fs::create_dir_all(inner.join(".coco").join("skills")).unwrap();
    let file = inner.join("foo.rs");
    std::fs::write(&file, "// touched by Read").unwrap();

    let result = discover_skill_dirs_for_paths(&[file.as_path()], cwd);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], inner.join(".coco").join("skills"));
}

#[test]
fn test_discover_skill_dirs_skips_gitignored() {
    // #197: a `.coco/skills` under a gitignored dir (node_modules) is
    // skipped, while a sibling under a normal dir is kept.
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    std::fs::create_dir_all(cwd.join("node_modules/pkg/.coco/skills")).unwrap();
    std::fs::create_dir_all(cwd.join("src/.coco/skills")).unwrap();
    let nm_file = cwd.join("node_modules/pkg/index.js");
    let src_file = cwd.join("src/main.rs");
    std::fs::write(&nm_file, "x").unwrap();
    std::fs::write(&src_file, "y").unwrap();

    let result = discover_skill_dirs_for_paths(&[nm_file.as_path(), src_file.as_path()], cwd);
    assert!(
        result.iter().any(|d| d.starts_with(cwd.join("src"))),
        "non-ignored src skill dir must be kept: {result:?}"
    );
    assert!(
        !result
            .iter()
            .any(|d| d.to_string_lossy().contains("node_modules")),
        "gitignored node_modules skill dir must be skipped: {result:?}"
    );
}

#[test]
fn test_discover_skill_dirs_excludes_cwd_level() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    // Skills dir AT cwd should NOT be returned — cwd-level skills are
    // loaded at startup, the dynamic walker only finds nested ones.
    std::fs::create_dir_all(cwd.join(".coco").join("skills")).unwrap();
    let file = cwd.join("readme.md");
    std::fs::write(&file, "").unwrap();

    let result = discover_skill_dirs_for_paths(&[file.as_path()], cwd);
    assert!(
        result.is_empty(),
        "cwd-level skills should be excluded, got: {result:?}"
    );
}

#[test]
fn test_discover_skill_dirs_deepest_first() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    // Two skills dirs at different depths.
    let outer = cwd.join("project");
    let inner = outer.join("module");
    std::fs::create_dir_all(outer.join(".coco").join("skills")).unwrap();
    std::fs::create_dir_all(inner.join(".coco").join("skills")).unwrap();
    let file = inner.join("hot.rs");
    std::fs::write(&file, "").unwrap();

    let result = discover_skill_dirs_for_paths(&[file.as_path()], cwd);
    assert_eq!(result.len(), 2);
    // Inner (more components) must come before outer.
    assert_eq!(result[0], inner.join(".coco").join("skills"));
    assert_eq!(result[1], outer.join(".coco").join("skills"));
}

#[test]
fn test_discover_skill_dirs_no_skills_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let project = cwd.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let file = project.join("plain.rs");
    std::fs::write(&file, "").unwrap();

    let result = discover_skill_dirs_for_paths(&[file.as_path()], cwd);
    assert!(result.is_empty());
}

#[test]
fn test_arguments_field_whitespace_split_filters_numeric() {
    let content = "\
---
description: Test arg parsing
arguments: env region 42 user
---

Body
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/x.md")).unwrap();
    // Numeric `42` is filtered (would conflict with `$N` shorthand).
    assert_eq!(skill.argument_names, vec!["env", "region", "user"]);
}

#[test]
fn test_arguments_field_legacy_aliases_still_work() {
    let content = "\
---
description: Test arg parsing
argument-names: env region
---

Body
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/x.md")).unwrap();
    assert_eq!(skill.argument_names, vec!["env", "region"]);
}

#[test]
fn test_discover_skill_dirs_dedupes_across_paths() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let project = cwd.join("project");
    std::fs::create_dir_all(project.join(".coco").join("skills")).unwrap();
    let file1 = project.join("a.rs");
    let file2 = project.join("b.rs");
    std::fs::write(&file1, "").unwrap();
    std::fs::write(&file2, "").unwrap();

    let result = discover_skill_dirs_for_paths(&[file1.as_path(), file2.as_path()], cwd);
    // Same skills dir should only appear once even though both files
    // resolve to it.
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], project.join(".coco").join("skills"));
}

// ── SKILL.md compatibility ──
//
// YAML frontmatter at the top of the file; skill name comes from the
// directory. These tests cover that layout end-to-end, plus the
// YAML features it exercises (nested mappings, sequence syntax).

#[test]
fn test_load_from_markdown_ts_format_frontmatter_first() {
    // No `# Name` heading — frontmatter sits at the top of the file.
    let content = "\
---
name: lark-base
version: 1.2.0
description: \"Operate Lark Base via lark-cli\"
---

Body content explaining the skill.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/lark-base.md")).unwrap();
    assert_eq!(skill.name, "lark-base");
    assert_eq!(skill.description, "Operate Lark Base via lark-cli");
    assert_eq!(skill.version.as_deref(), Some("1.2.0"));
    assert!(skill.prompt.starts_with("Body content"));
}

#[test]
fn test_load_from_markdown_ts_format_nested_metadata_ignored() {
    // The `metadata` field is not defined in the spec, but a real YAML parser
    // must tolerate (and silently drop) unknown nested shapes — the rest of
    // the file should still load.
    let content = "\
---
name: lark-base
description: \"Lark Base operations\"
metadata:
  requires:
    bins: [\"lark-cli\"]
  cliHelp: \"lark-cli base --help\"
---

Body.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/lark-base.md")).unwrap();
    assert_eq!(skill.name, "lark-base");
    assert_eq!(skill.description, "Lark Base operations");
    assert_eq!(skill.prompt, "Body.");
}

#[test]
fn test_load_from_markdown_ts_format_yaml_list_allowed_tools() {
    // Real YAML supports list syntax for allowed-tools; both forms must
    // produce the same result.
    let content = "\
---
name: review-pr
description: Review a PR
allowed-tools:
  - Bash
  - Read
  - Grep
---

Review the diff.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/review.md")).unwrap();
    assert_eq!(
        skill.allowed_tools,
        Some(vec!["Bash".into(), "Read".into(), "Grep".into()])
    );
}

#[test]
fn test_load_from_markdown_ts_format_yaml_list_paths() {
    let content = "\
---
name: rust-skill
description: Rust skill
paths:
  - src/**/*.rs
  - tests/**
---

Body.
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/rust.md")).unwrap();
    // Trailing `/**` stripped per `parseSkillPaths`.
    assert_eq!(skill.paths, vec!["src/**/*.rs", "tests"]);
}

#[test]
fn test_discover_ts_format_skill_md_takes_name_from_directory() {
    // The actual lark-base scenario: SKILL.md inside a named directory.
    // Name should come from the directory, not the (absent) heading or
    // the frontmatter `name` field.
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("lark-base");
    std::fs::create_dir(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\n\
name: this-is-overridden-by-dir\n\
version: 1.2.0\n\
description: \"Operate Lark Base\"\n\
metadata:\n  \
  requires:\n    \
    bins: [\"lark-cli\"]\n\
---\n\n\
Body of the skill.\n",
    )
    .unwrap();

    let skills = discover_skills(&[dir.path().to_path_buf()]);
    assert_eq!(skills.len(), 1, "lark-base SKILL.md should load");
    assert_eq!(
        skills[0].name, "lark-base",
        "name should come from the directory in SKILL.md format"
    );
    assert_eq!(skills[0].description, "Operate Lark Base");
    assert_eq!(skills[0].version.as_deref(), Some("1.2.0"));
}

#[test]
fn test_load_from_markdown_plain_prose_loads_as_body() {
    // Plain prose is not an error — it becomes the skill body.
    // The only way `parse_skill_markdown` returns Err is if the path has
    // no usable file name (covered by `derive_skill_name_from_path`).
    let content = "Just some plain text, not a skill at all.\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/bad.md")).unwrap();
    assert_eq!(skill.name, "bad");
    assert_eq!(skill.prompt, "Just some plain text, not a skill at all.");
}

#[test]
fn test_display_name_from_frontmatter_name() {
    // The path-derived name is unchanged; display_name overrides only the
    // user-facing surface.
    let content = "\
---
name: \"My Pretty Name\"
description: A skill
---
body
";
    let skill = parse_skill_markdown(content, Path::new("/tmp/raw-name.md")).unwrap();
    assert_eq!(skill.name, "raw-name", "name comes from path stem");
    assert_eq!(
        skill.display_name.as_deref(),
        Some("My Pretty Name"),
        "display_name comes from frontmatter `name` field"
    );
    assert_eq!(
        skill.user_facing_name(),
        "My Pretty Name",
        "user_facing_name prefers display_name over name"
    );
}

#[test]
fn test_user_facing_name_falls_back_to_name() {
    // With no frontmatter `name`, display_name is None and the canonical
    // name is used.
    let content = "---\ndescription: A skill\n---\nbody\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/raw-name.md")).unwrap();
    assert!(skill.display_name.is_none());
    assert_eq!(skill.user_facing_name(), "raw-name");
}

#[test]
fn test_display_name_does_not_change_lookup_identity() {
    // SKILL.md in a directory: name comes from the directory; the
    // frontmatter `name` populates display_name but does NOT change
    // how the skill is keyed in the manager.
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("internal-id");
    std::fs::create_dir(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: \"Pretty Display\"\ndescription: x\n---\nbody\n",
    )
    .unwrap();

    let skills = discover_skills(&[dir.path().to_path_buf()]);
    assert_eq!(skills.len(), 1);
    let s = &skills[0];
    // Lookup name = directory; display_name = frontmatter name.
    assert_eq!(s.name, "internal-id");
    assert_eq!(s.display_name.as_deref(), Some("Pretty Display"));
    assert_eq!(s.user_facing_name(), "Pretty Display");
}

// ── has_user_specified_description / extract_description_from_markdown ──

#[test]
fn test_has_user_specified_description_true_when_frontmatter_set() {
    let content = "---\ndescription: Explicit user-supplied desc\n---\nbody\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/x.md")).unwrap();
    assert_eq!(skill.description, "Explicit user-supplied desc");
    assert!(skill.has_user_specified_description);
}

#[test]
fn test_has_user_specified_description_false_when_extracted_from_body() {
    // No frontmatter description → fallback to first non-empty body line.
    let content = "---\nname: foo\n---\n\n# My Skill Heading\nMore text.\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/x.md")).unwrap();
    // Leading `# ` is stripped before storing the description.
    assert_eq!(skill.description, "My Skill Heading");
    assert!(!skill.has_user_specified_description);
}

#[test]
fn test_extract_description_caps_at_100_chars() {
    let long = "a".repeat(200);
    let got = extract_description_from_markdown(&long, "Skill");
    assert!(got.ends_with("..."), "long text gets ellipsis: {got}");
    assert_eq!(got.chars().count(), 100, "exactly 97 chars + '...'");
}

#[test]
fn test_extract_description_falls_back_to_default() {
    let got = extract_description_from_markdown("\n\n   \n", "Skill");
    assert_eq!(got, "Skill");
}

#[test]
fn test_progress_message_defaults_to_running() {
    let content = "---\ndescription: x\n---\nbody\n";
    let skill = parse_skill_markdown(content, Path::new("/tmp/x.md")).unwrap();
    assert_eq!(
        skill.progress_message.as_deref(),
        Some("running"),
        "progressMessage defaults to 'running'"
    );
}

#[test]
fn test_finance_skills_real_world_example_loads() {
    // Verbatim SKILL.md from the alirezarezvani/claude-skills `finance`
    // bundle. Exercises: quoted strings in frontmatter, YAML sequences
    // (`tags`, `agents`), unknown fields (`author`, `license`, `tags`,
    // `agents`) silently ignored, multi-segment version (`1.0.0` parsed as
    // string), body containing its own `# Finance Skills` heading.
    let content = "\
---
name: \"finance-skills\"
description: \"Financial analyst agent skill and plugin for Claude Code, Codex, Gemini CLI, Cursor, OpenClaw. Ratio analysis, DCF valuation, budget variance, rolling forecasts. 4 Python tools (stdlib-only).\"
version: 1.0.0
author: Alireza Rezvani
license: MIT
tags:
  - finance
  - financial-analysis
agents:
  - claude-code
  - codex-cli
---

# Finance Skills

Production-ready financial analysis skill for strategic decision-making.

## Quick Start

### Claude Code
```
/read finance/financial-analyst/SKILL.md
```

### Codex CLI
```bash
npx agent-skills-cli add alirezarezvani/claude-skills/finance
```

## Skills Overview

| Skill | Folder | Focus |
|-------|--------|-------|
| Financial Analyst | `financial-analyst/` | Ratio analysis, DCF, budget variance, forecasting |

## Python Tools

4 scripts, all stdlib-only:

```bash
python3 financial-analyst/scripts/ratio_calculator.py --help
python3 financial-analyst/scripts/dcf_valuation.py --help
```

## Rules

- Load only the specific skill SKILL.md you need
- Always validate financial outputs against source data
";

    // Discover via the SKILL.md-in-directory convention so the name comes
    // from the directory (the frontmatter `name` field is ignored for
    // skill identity).
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("finance-skills");
    std::fs::create_dir(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), content).unwrap();

    let skills = discover_skills(&[dir.path().to_path_buf()]);
    assert_eq!(skills.len(), 1, "finance-skills SKILL.md must load");

    let s = &skills[0];
    assert_eq!(s.name, "finance-skills");
    assert!(
        s.description
            .starts_with("Financial analyst agent skill and plugin"),
        "description must come from frontmatter (got: {:?})",
        s.description
    );
    assert!(
        s.description.contains("DCF valuation"),
        "description must be the full quoted string"
    );
    assert_eq!(s.version.as_deref(), Some("1.0.0"));

    // Body fields the schema does not model are silently ignored — the
    // file still loads. Body is preserved verbatim including the heading
    // `# Finance Skills` (Markdown headings are not stripped from the body).
    assert!(s.prompt.starts_with("# Finance Skills"));
    assert!(s.prompt.contains("Quick Start"));
    assert!(s.prompt.contains("Python Tools"));

    // Defaults that should not have been disturbed by unknown fields.
    assert!(s.user_invocable);
    assert!(!s.disable_model_invocation);
    assert!(!s.disabled);
    assert!(s.allowed_tools.is_none());
    assert!(s.argument_names.is_empty());
}

// ── Conditional activation (paths frontmatter) ──
//
// Skills with non-empty `paths` are hidden from `visible()` / `get()` /
// `listing()` until a file the model touches matches one of their
// gitignore-style patterns.

fn conditional_skill(name: &str, paths: Vec<&str>) -> SkillDefinition {
    let mut s = test_skill(
        name,
        "conditional skill",
        "do conditional work",
        SkillSource::Project {
            path: format!("/proj/.coco/skills/{name}/SKILL.md").into(),
        },
    );
    s.paths = paths.into_iter().map(str::to_string).collect();
    s
}

#[test]
fn test_conditional_skill_hidden_before_activation() {
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));
    mgr.register(test_skill("always-on", "always", "x", SkillSource::Bundled));

    // visible() and len() / all() exclude conditional skills until
    // activated. `conditional_skill_count` reports them separately.
    assert_eq!(mgr.len(), 1, "only the unconditional skill is in disk");
    assert_eq!(mgr.conditional_skill_count(), 1);
    assert!(mgr.get("rust-lint").is_none(), "hidden from get() too");
    assert!(mgr.get("always-on").is_some());
    let visible = mgr.visible(&coco_types::Features::with_defaults());
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].name, "always-on");
}

#[test]
fn test_all_including_conditional_covers_unactivated_skills() {
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));
    mgr.register(test_skill("always-on", "always", "x", SkillSource::Bundled));

    // all() excludes the un-activated conditional skill — that's the
    // model-listing contract. all_including_conditional() reveals it
    // for the `/skills` dialog so users can override before
    // activation.
    let visible_to_model: Vec<String> = mgr.all().iter().map(|s| s.name.clone()).collect();
    assert_eq!(visible_to_model, vec!["always-on"]);

    let mut full: Vec<String> = mgr
        .all_including_conditional()
        .iter()
        .map(|s| s.name.clone())
        .collect();
    full.sort();
    assert_eq!(full, vec!["always-on", "rust-lint"]);
}

#[test]
fn test_conditional_skill_activates_on_matching_path() {
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));
    assert_eq!(mgr.conditional_skill_count(), 1);

    // Matching cwd-relative file path → activation.
    let cwd = PathBuf::from("/proj");
    let activated = mgr.activate_for_paths(&[cwd.join("src/lib.rs")], &cwd);
    assert_eq!(activated, vec!["rust-lint"]);

    // Now visible.
    assert!(mgr.get("rust-lint").is_some());
    assert_eq!(mgr.conditional_skill_count(), 0);
    let visible = mgr.visible(&coco_types::Features::with_defaults());
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].name, "rust-lint");
}

#[test]
fn test_conditional_skill_not_activated_by_non_matching_path() {
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));

    let cwd = PathBuf::from("/proj");
    let activated = mgr.activate_for_paths(&[cwd.join("docs/README.md")], &cwd);
    assert!(activated.is_empty());
    assert!(mgr.get("rust-lint").is_none(), "still hidden");
    assert_eq!(mgr.conditional_skill_count(), 1);
}

#[test]
fn test_activation_idempotent_returns_only_new_names() {
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("a", vec!["src/**/*.rs"]));
    mgr.register(conditional_skill("b", vec!["docs/**"]));

    let cwd = PathBuf::from("/proj");
    let first = mgr.activate_for_paths(&[cwd.join("src/lib.rs")], &cwd);
    assert_eq!(first, vec!["a"]);

    // Second call with the same path: nothing new to activate.
    let second = mgr.activate_for_paths(&[cwd.join("src/lib.rs")], &cwd);
    assert!(second.is_empty());

    // A third call with a path matching `b` activates only `b`.
    let third = mgr.activate_for_paths(&[cwd.join("docs/intro.md")], &cwd);
    assert_eq!(third, vec!["b"]);
}

#[test]
fn test_activation_persists_across_reload() {
    // `activatedConditionalSkillNames` survives `clearSkillCaches`. On
    // reload, an already-activated skill is re-categorized as unconditional.
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));

    let cwd = PathBuf::from("/proj");
    mgr.activate_for_paths(&[cwd.join("src/lib.rs")], &cwd);
    assert!(mgr.get("rust-lint").is_some());

    // Reload (e.g. file-watcher fires after the SKILL.md is touched).
    mgr.reload_disk_skills(vec![conditional_skill("rust-lint", vec!["src/**/*.rs"])]);
    assert!(mgr.get("rust-lint").is_some(), "activation survives reload");
    assert_eq!(mgr.conditional_skill_count(), 0);
}

#[test]
fn test_activation_skips_paths_outside_cwd() {
    // Relative paths starting with `..` and absolute paths outside cwd
    // are skipped silently.
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));

    let cwd = PathBuf::from("/proj");
    // Absolute path NOT under cwd → skipped.
    let activated = mgr.activate_for_paths(&[PathBuf::from("/other/src/lib.rs")], &cwd);
    assert!(activated.is_empty());
    // Relative escaping path → skipped.
    let activated = mgr.activate_for_paths(&[PathBuf::from("../src/lib.rs")], &cwd);
    assert!(activated.is_empty());
    // Still hidden.
    assert!(mgr.get("rust-lint").is_none());
}

#[test]
fn test_activation_relative_path_works() {
    // Cwd-relative file paths are gitignore-matched as-is using the
    // raw `relativePath` string when it's already relative.
    let mgr = SkillManager::new();
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));

    let cwd = PathBuf::from("/proj");
    let activated = mgr.activate_for_paths(&[PathBuf::from("src/lib.rs")], &cwd);
    assert_eq!(activated, vec!["rust-lint"]);
}

#[test]
fn test_activation_brace_expanded_pattern_matches() {
    // After `expand_braces`, `*.{ts,tsx}` produces two patterns. Either
    // file extension activates the skill.
    let mgr = SkillManager::new();
    let content = "\
---
description: TS skill
paths: '*.{ts,tsx}'
---
body
";
    let skill = parse_skill_markdown(content, Path::new("/proj/.coco/skills/ts/SKILL.md")).unwrap();
    mgr.register(skill);

    let cwd = PathBuf::from("/proj");
    let activated = mgr.activate_for_paths(&[cwd.join("foo.tsx")], &cwd);
    assert_eq!(activated, vec!["ts"]);
}

#[test]
fn test_listing_delta_surfaces_newly_activated_skill() {
    // The reminder pipeline surfaces newly-visible skills via
    // `take_unannounced_skills` delta. A conditional skill activated
    // mid-session shows up in the next call's delta.
    let mgr = SkillManager::new();
    mgr.register(test_skill("always", "x", "y", SkillSource::Bundled));
    mgr.register(conditional_skill("rust-lint", vec!["src/**/*.rs"]));

    // First listing call: only the unconditional skill announces.
    let names: Vec<&str> = vec!["always"];
    let (delta, is_initial) = mgr.take_unannounced_skills(None, &names);
    assert!(is_initial);
    assert_eq!(delta, vec!["always".to_string()]);

    // Activate the conditional skill — now `visible()` returns both.
    let cwd = PathBuf::from("/proj");
    let activated = mgr.activate_for_paths(&[cwd.join("src/lib.rs")], &cwd);
    assert_eq!(activated, vec!["rust-lint"]);

    // Second listing: only the newly-visible name is in the delta.
    let names: Vec<&str> = vec!["always", "rust-lint"];
    let (delta, is_initial) = mgr.take_unannounced_skills(None, &names);
    assert!(!is_initial);
    assert_eq!(delta, vec!["rust-lint".to_string()]);
}

#[test]
fn test_reset_announcements_reannounces_after_reload() {
    // skills-198: after a reload/clear, the announcement map is wiped so
    // every skill re-announces (an edited same-named skill must surface
    // again).
    let mgr = SkillManager::new();
    let names: Vec<&str> = vec!["alpha", "beta"];

    let (delta, is_initial) = mgr.take_unannounced_skills(None, &names);
    assert!(is_initial);
    assert_eq!(delta, vec!["alpha".to_string(), "beta".to_string()]);

    // No reset: nothing new to announce.
    let (delta, is_initial) = mgr.take_unannounced_skills(None, &names);
    assert!(!is_initial);
    assert!(delta.is_empty());

    // After reset, the next pass re-announces everything as initial.
    mgr.reset_announcements();
    let (delta, is_initial) = mgr.take_unannounced_skills(None, &names);
    assert!(is_initial);
    assert_eq!(delta, vec!["alpha".to_string(), "beta".to_string()]);
}

#[test]
fn probe_bare_dir_pattern_matches_files_inside() {
    // The `ignore` library walks parents: `ignore().add(['build']).ignores('build/foo.rs')`
    // returns true. Verifies the `paths: build/**` → stripped to `build` case.
    let mgr = SkillManager::new();
    let mut skill = conditional_skill("build-skill", vec![]);
    skill.paths = vec!["build".to_string()];
    mgr.register(skill);
    let cwd = PathBuf::from("/proj");
    let activated = mgr.activate_for_paths(&[cwd.join("build/foo.rs")], &cwd);
    assert_eq!(
        activated,
        vec!["build-skill"],
        "TS parity: bare-dir pattern `build` must match `build/foo.rs`"
    );
}

#[test]
fn probe_paths_slash_double_star_stripped_then_matches_inside() {
    // End-to-end: `paths: build/**` parses to `["build"]` (the `/**`
    // suffix is stripped per `parseSkillPaths`). Activation against
    // `build/foo.rs` should fire.
    let mgr = SkillManager::new();
    let content = "\
---
description: x
paths: build/**
---
body
";
    let skill =
        parse_skill_markdown(content, Path::new("/proj/.coco/skills/buildskill/SKILL.md")).unwrap();
    assert_eq!(skill.paths, vec!["build".to_string()]);
    mgr.register(skill);

    let cwd = PathBuf::from("/proj");
    let activated = mgr.activate_for_paths(&[cwd.join("build/foo.rs")], &cwd);
    assert_eq!(activated, vec!["buildskill"]);
}

#[test]
fn test_register_with_empty_paths_is_unconditional() {
    // A skill that explicitly serializes `paths: []` after frontmatter
    // parse (e.g. all-`**` patterns stripped to empty) should land in
    // the visible bucket, not the conditional one.
    let mgr = SkillManager::new();
    let mut s = conditional_skill("regular", vec!["**"]);
    // Parse-time invariant: all-`**` collapses to empty. Reproduce that
    // post-condition directly.
    s.paths = vec![];
    mgr.register(s);
    assert_eq!(mgr.conditional_skill_count(), 0);
    assert!(mgr.get("regular").is_some());
}

#[test]
fn build_session_skill_manager_includes_bundled() {
    // Regression: the per-session catalog must fold in bundled skills so
    // `/context` usage detail and the command registry see them — not just
    // the `/skills` dialog's throwaway manager. Nonexistent dirs isolate
    // the bundled contribution.
    let config_home = PathBuf::from("/nonexistent-coco-config");
    let cwd = PathBuf::from("/nonexistent-coco-cwd");
    let manager = build_session_skill_manager(&config_home, &cwd, &SkillLoadGates::all_enabled());
    let all = manager.all();
    let names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
    assert!(
        names.contains(&"keybindings-help"),
        "bundled skills missing from session manager: {names:?}"
    );
}

#[test]
fn build_session_skill_manager_loads_parent_project_coco_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let project = tmp.path().join("project");
    let nested = project.join("packages").join("app");
    let parent_skill = project.join(".coco").join("skills").join("parent-skill");
    std::fs::create_dir_all(&parent_skill).unwrap();
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(
        parent_skill.join("SKILL.md"),
        "---\ndescription: parent project skill\n---\n\nbody",
    )
    .unwrap();

    let manager =
        build_session_skill_manager(&config_home, &nested, &SkillLoadGates::all_enabled());
    let skill = manager
        .get("parent-skill")
        .expect("parent project skill should load");
    assert!(matches!(skill.source, SkillSource::Project { .. }));
}

#[test]
fn build_session_skill_manager_ignores_legacy_project_claude_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let project = tmp.path().join("project");
    let legacy_skill = project.join(".claude").join("skills").join("legacy-skill");
    std::fs::create_dir_all(&legacy_skill).unwrap();
    std::fs::write(
        legacy_skill.join("SKILL.md"),
        "---\ndescription: legacy project skill\n---\n\nbody",
    )
    .unwrap();

    let manager =
        build_session_skill_manager(&config_home, &project, &SkillLoadGates::all_enabled());
    assert!(manager.get("legacy-skill").is_none());
}

#[test]
fn build_session_skill_manager_loads_legacy_commands_dir_skill_md() {
    // skills-195: the deprecated `.coco/commands` directory is folded in,
    // supporting the SKILL.md directory format.
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let project = tmp.path().join("project");
    let cmd_dir = project
        .join(".coco")
        .join("commands")
        .join("legacy-cmd-dir");
    std::fs::create_dir_all(&cmd_dir).unwrap();
    std::fs::write(
        cmd_dir.join("SKILL.md"),
        "---\ndescription: legacy command (dir form)\n---\n\nbody",
    )
    .unwrap();

    let manager =
        build_session_skill_manager(&config_home, &project, &SkillLoadGates::all_enabled());
    let skill = manager
        .get("legacy-cmd-dir")
        .expect("legacy commands-dir SKILL.md should load");
    assert!(matches!(skill.source, SkillSource::Project { .. }));
}

#[test]
fn build_session_skill_manager_loads_legacy_commands_dir_flat_md() {
    // skills-195: flat `.md` files in `.coco/commands` also load (legacy
    // parser compatibility) — name derives from the file stem.
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let project = tmp.path().join("project");
    let cmd_dir = project.join(".coco").join("commands");
    std::fs::create_dir_all(&cmd_dir).unwrap();
    std::fs::write(
        cmd_dir.join("flatcmd.md"),
        "---\ndescription: legacy command (flat md)\n---\n\nbody",
    )
    .unwrap();

    let manager =
        build_session_skill_manager(&config_home, &project, &SkillLoadGates::all_enabled());
    assert!(
        manager.get("flatcmd").is_some(),
        "flat .md legacy command should load"
    );
}

#[test]
fn build_session_skill_manager_dedups_skill_and_legacy_command_by_realpath() {
    // skills-195: a skill reachable from both `.coco/skills` and
    // `.coco/commands` via the *same physical file* (symlinked dir) loads
    // exactly once — deduped by canonical realpath, not just by name.
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let project = tmp.path().join("project");

    // Real skill lives under .coco/skills/shared/SKILL.md.
    let skills_root = project.join(".coco").join("skills");
    let shared = skills_root.join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        shared.join("SKILL.md"),
        "---\ndescription: shared via realpath\n---\n\nbody",
    )
    .unwrap();

    // Point .coco/commands at the SAME directory tree via a symlink, so the
    // legacy loader rediscovers the identical SKILL.md file.
    let commands_root = project.join(".coco").join("commands");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&skills_root, &commands_root).unwrap();
    #[cfg(not(unix))]
    {
        // No symlink support: fall back to copying — the name-dedup still
        // collapses the duplicate even if realpaths differ.
        std::fs::create_dir_all(commands_root.join("shared")).unwrap();
        std::fs::copy(
            shared.join("SKILL.md"),
            commands_root.join("shared").join("SKILL.md"),
        )
        .unwrap();
    }

    let manager =
        build_session_skill_manager(&config_home, &project, &SkillLoadGates::all_enabled());
    let count = manager
        .all_including_conditional()
        .iter()
        .filter(|s| s.name == "shared")
        .count();
    assert_eq!(count, 1, "shared skill should register exactly once");
}

#[test]
fn project_dirs_up_to_home_stops_at_git_root() {
    // Layout: a stray `<tmp>/outer/.coco/skills` OUTSIDE the repo, a repo at
    // `<tmp>/outer/repo/.git`, and a nested cwd `<tmp>/outer/repo/sub`.
    // `getProjectDirsUpToHome` stops at the git root so the parent dir's
    // skills never leak into the project scope.
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path().join("outer");
    let repo = outer.join("repo");
    let sub = repo.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    std::fs::create_dir_all(outer.join(".coco").join("skills")).unwrap();

    let dirs = project_dirs_up_to_home(&sub, "skills");
    assert!(dirs.contains(&sub.join(".coco").join("skills")));
    assert!(
        dirs.contains(&repo.join(".coco").join("skills")),
        "the git root dir itself is included"
    );
    assert!(
        !dirs.contains(&outer.join(".coco").join("skills")),
        "must stop at the git root and NOT leak the parent dir outside the repo: {dirs:?}"
    );
}

/// Plant a `<root>/<name>/SKILL.md` skill and return the root dir.
fn plant_skill(root: &std::path::Path, name: &str) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\ndescription: {name} skill\n---\n\nbody"),
    )
    .unwrap();
}

#[test]
fn build_session_skill_manager_user_gate_skips_user_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    plant_skill(&config_home.join("skills"), "user-skill");

    let gates = SkillLoadGates {
        user_enabled: false,
        ..SkillLoadGates::all_enabled()
    };
    let manager = build_session_skill_manager(&config_home, &cwd, &gates);
    assert!(manager.get("user-skill").is_none());

    // Enabled again → it loads.
    let manager = build_session_skill_manager(&config_home, &cwd, &SkillLoadGates::all_enabled());
    assert!(manager.get("user-skill").is_some());
}

#[test]
fn build_session_skill_manager_project_gate_skips_project_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let cwd = tmp.path().join("project");
    plant_skill(&cwd.join(".coco").join("skills"), "proj-skill");

    let gates = SkillLoadGates {
        project_enabled: false,
        additional_dirs_enabled: false,
        ..SkillLoadGates::all_enabled()
    };
    let manager = build_session_skill_manager(&config_home, &cwd, &gates);
    assert!(manager.get("proj-skill").is_none());
}

#[test]
fn build_session_skill_manager_loads_additional_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let add_root = tmp.path().join("extra");
    plant_skill(&add_root.join(".coco").join("skills"), "extra-skill");

    let gates = SkillLoadGates {
        additional_dirs: vec![add_root.clone()],
        ..SkillLoadGates::all_enabled()
    };
    let manager = build_session_skill_manager(&config_home, &cwd, &gates);
    assert!(manager.get("extra-skill").is_some());

    // additional_dirs_enabled = false → skipped.
    let gates = SkillLoadGates {
        additional_dirs: vec![add_root],
        additional_dirs_enabled: false,
        ..SkillLoadGates::all_enabled()
    };
    let manager = build_session_skill_manager(&config_home, &cwd, &gates);
    assert!(manager.get("extra-skill").is_none());
}

#[test]
fn build_session_skill_manager_locked_skips_all_but_managed() {
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let cwd = tmp.path().join("project");
    plant_skill(&config_home.join("skills"), "user-skill");
    plant_skill(&cwd.join(".coco").join("skills"), "proj-skill");

    // skills_locked forces managed-only: user + project are skipped even
    // though their per-scope gates are enabled.
    let gates = SkillLoadGates {
        skills_locked: true,
        ..SkillLoadGates::all_enabled()
    };
    let manager = build_session_skill_manager(&config_home, &cwd, &gates);
    assert!(manager.get("user-skill").is_none());
    assert!(manager.get("proj-skill").is_none());
    // Bundled skills still load (never gated).
    assert!(manager.get("keybindings-help").is_some());
}

#[test]
fn build_session_skill_manager_managed_gate_off_skips_managed() {
    // managed_enabled = false mirrors COCO_DISABLE_POLICY_SKILLS=1. Bundled
    // skills still load; we only assert managed is skipped via the gate path
    // (the managed dir is a fixed system path, so we just exercise the branch).
    let tmp = tempfile::tempdir().unwrap();
    let config_home = tmp.path().join("config");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();

    let gates = SkillLoadGates {
        managed_enabled: false,
        ..SkillLoadGates::all_enabled()
    };
    let manager = build_session_skill_manager(&config_home, &cwd, &gates);
    // No panic, bundled still present.
    assert!(manager.get("keybindings-help").is_some());
}
