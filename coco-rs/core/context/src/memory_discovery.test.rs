use pretty_assertions::assert_eq;

use crate::memory_discovery::MemoryFileSource;
use crate::memory_discovery::discover_memory_files;

#[test]
fn discovers_project_root_claude_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "# Test").unwrap();

    let files = discover_memory_files(dir.path());
    let root = files.iter().find(|f| {
        f.source == MemoryFileSource::Project
            && f.path.file_name().unwrap() == std::ffi::OsStr::new("CLAUDE.md")
    });
    assert!(root.is_some(), "expected CLAUDE.md to load as Project");
    assert!(root.unwrap().content.contains("# Test"));
}

#[test]
fn discovers_agents_md_at_project_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Agents").unwrap();

    let files = discover_memory_files(dir.path());
    let root = files.iter().find(|f| {
        f.source == MemoryFileSource::Project
            && f.path.file_name().unwrap() == std::ffi::OsStr::new("AGENTS.md")
    });
    assert!(root.is_some(), "expected AGENTS.md to load as Project");
    assert!(root.unwrap().content.contains("# Agents"));
}

#[test]
fn discovers_both_claude_and_agents_when_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "c").unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "a").unwrap();

    let files = discover_memory_files(dir.path());
    let names: Vec<&str> = files
        .iter()
        .filter(|f| f.source == MemoryFileSource::Project && f.path.parent() == Some(dir.path()))
        .map(|f| f.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert!(names.contains(&"CLAUDE.md"));
    assert!(names.contains(&"AGENTS.md"));
}

#[test]
fn case_insensitive_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Claude.md"), "x").unwrap();

    let files = discover_memory_files(dir.path());
    let hit = files
        .iter()
        .find(|f| f.source == MemoryFileSource::Project && f.path.parent() == Some(dir.path()));
    assert!(hit.is_some(), "Claude.md should match case-insensitively");
}

#[test]
fn discovers_local_claude_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("CLAUDE.local.md"), "local config").unwrap();

    let files = discover_memory_files(dir.path());
    let local = files.iter().find(|f| f.source == MemoryFileSource::Local);
    assert!(local.is_some());
}

#[test]
fn discovers_agents_local_md() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.local.md"), "local").unwrap();

    let files = discover_memory_files(dir.path());
    let local = files.iter().find(|f| f.source == MemoryFileSource::Local);
    assert!(local.is_some(), "expected AGENTS.local.md to load as Local");
}

#[test]
fn discovers_dot_coco_config_dir() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join(".coco");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(cfg.join("CLAUDE.md"), "config").unwrap();

    let files = discover_memory_files(dir.path());
    let cfg_file = files
        .iter()
        .find(|f| f.source == MemoryFileSource::ProjectConfig);
    assert!(cfg_file.is_some());
}

#[test]
fn empty_dir_has_no_project_files() {
    let dir = tempfile::tempdir().unwrap();
    let files = discover_memory_files(dir.path());
    assert!(
        files.iter().all(|f| f.source != MemoryFileSource::Project),
        "empty CWD should not produce Project-source entries"
    );
}

#[test]
fn does_not_load_immediate_children_anymore() {
    // Phase 5a regression test: the eager phase walks root→CWD inclusive
    // only. Children of CWD must NOT be eager-loaded; they're the job of
    // the per-file trigger pipeline (Phase 2). Without this guard, we'd
    // double-load every CLAUDE.md the trigger pipeline finds.
    let dir = tempfile::tempdir().unwrap();
    let child = dir.path().join("subproject");
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(child.join("CLAUDE.md"), "child").unwrap();

    let files = discover_memory_files(dir.path());
    let child_loaded = files.iter().any(|f| f.path == child.join("CLAUDE.md"));
    assert!(
        !child_loaded,
        "immediate child CLAUDE.md must not be eager-loaded"
    );
}

#[test]
fn walks_root_to_cwd() {
    // Build /tmp_root/proj/sub. CWD = /tmp_root/proj/sub.
    // Eager should load /tmp_root/proj/CLAUDE.md (parent of CWD) and
    // /tmp_root/proj/sub/CLAUDE.md (CWD itself), but the temp prefix
    // isn't a memory dir.
    let root = tempfile::tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(proj.join("CLAUDE.md"), "proj").unwrap();
    std::fs::write(sub.join("CLAUDE.md"), "sub").unwrap();

    let files = discover_memory_files(&sub);
    let project_paths: Vec<_> = files
        .iter()
        .filter(|f| f.source == MemoryFileSource::Project)
        .map(|f| f.path.clone())
        .collect();
    // Both should appear, with the deeper one (CWD) loaded after the
    // ancestor — "later = higher attention priority" semantics.
    let proj_idx = project_paths
        .iter()
        .position(|p| p == &proj.join("CLAUDE.md"));
    let sub_idx = project_paths
        .iter()
        .position(|p| p == &sub.join("CLAUDE.md"));
    assert!(proj_idx.is_some(), "ancestor CLAUDE.md missing");
    assert!(sub_idx.is_some(), "CWD CLAUDE.md missing");
    assert!(
        proj_idx.unwrap() < sub_idx.unwrap(),
        "ancestor should load before CWD (root→CWD order)"
    );
}

#[test]
fn eager_loads_unconditional_project_rules_not_conditional() {
    let dir = tempfile::tempdir().unwrap();
    let rules = dir.path().join(".coco").join("rules");
    std::fs::create_dir_all(&rules).unwrap();
    // Unconditional rule (no `paths:` frontmatter) — must be in turn-1 prompt.
    std::fs::write(rules.join("style.md"), "Always use tabs.").unwrap();
    // Conditional rule (`paths:`) — must NOT eager-load (it's lazy).
    std::fs::write(
        rules.join("ts.md"),
        "---\npaths: \"**/*.ts\"\n---\nTS-only rule.",
    )
    .unwrap();

    let files = discover_memory_files(dir.path());
    assert!(
        files.iter().any(|f| f.content.contains("Always use tabs")),
        "unconditional .coco/rules must be eager-loaded"
    );
    assert!(
        !files.iter().any(|f| f.content.contains("TS-only rule")),
        "conditional .coco/rules must NOT be eager-loaded"
    );
}

#[test]
fn discovery_is_deterministic_across_turns() {
    // Prompt-cache safety: re-reading the same on-disk files every turn (at
    // build_prompt time) must produce byte-identical output, or the cached
    // system-prompt prefix would thrash. Order comes from sorted
    // find_memory_files / collect_rule_files, content from read_to_string —
    // so unchanged files yield an identical result and the LLM prompt cache
    // stays valid.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("CLAUDE.md"), "root").unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "agents").unwrap();
    let rules = dir.path().join(".coco").join("rules");
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::write(rules.join("b.md"), "rule b").unwrap();
    std::fs::write(rules.join("a.md"), "rule a").unwrap();

    let snapshot = || -> Vec<(std::path::PathBuf, String, MemoryFileSource)> {
        discover_memory_files(dir.path())
            .into_iter()
            .map(|f| (f.path, f.content, f.source))
            .collect()
    };
    let first = snapshot();
    for _ in 0..5 {
        assert_eq!(
            first,
            snapshot(),
            "discovery must be deterministic per turn"
        );
    }
}

#[test]
fn dedupes_canonical_path() {
    // Two entries pointing at the same file (via symlink): only one load.
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("CLAUDE.md");
    std::fs::write(&real, "x").unwrap();

    let files = discover_memory_files(dir.path());
    let count = files
        .iter()
        .filter(|f| f.path.canonicalize().ok() == Some(real.canonicalize().unwrap()))
        .count();
    assert_eq!(count, 1, "expected exactly one load of {}", real.display());
}

/// Run `git` in `cwd`, isolated from the developer's global/system config so
/// the test is reproducible regardless of host git settings. Panics if git is
/// unavailable (the workspace already relies on real git in tests).
fn git(cwd: &std::path::Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("git must be available for this test");
    assert!(status.success(), "git {args:?} failed");
}

fn git_init_repo(root: &std::path::Path) {
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@t"]);
    git(root, &["config", "user.name", "t"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn nested_worktree_skips_main_repo_checked_in_but_keeps_local() {
    // A git worktree nested at <main>/.coco/worktrees/<slug> (coco's agent
    // worktree layout). git checks the branch's tracked memory out into the
    // worktree, so <main>/CLAUDE.md and <wt>/CLAUDE.md hold the SAME content at
    // DISTINCT paths — the canonical-path dedup can't catch it. We must skip
    // the main repo's checked-in copy (Project + rules) while still loading the
    // gitignored CLAUDE.local.md that only exists in the main repo.
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path();
    git_init_repo(main);
    std::fs::write(main.join("CLAUDE.md"), "MAIN-ROOT-MEMORY").unwrap();
    let rules = main.join(".coco").join("rules");
    std::fs::create_dir_all(&rules).unwrap();
    std::fs::write(rules.join("style.md"), "TABS-RULE").unwrap();
    git(main, &["add", "."]);
    git(main, &["commit", "-q", "-m", "init"]);

    // Nested worktree (checks out CLAUDE.md + the rule into <wt>).
    let wt = main.join(".coco").join("worktrees").join("wt");
    git(main, &["worktree", "add", "-q", wt.to_str().unwrap()]);

    // Gitignored local file — only in the main repo, never in the worktree.
    std::fs::write(main.join("CLAUDE.local.md"), "MAIN-LOCAL").unwrap();

    let files = discover_memory_files(&wt);

    // The worktree's own checked-in copies load…
    assert!(
        files.iter().any(|f| f.path == wt.join("CLAUDE.md")),
        "worktree CLAUDE.md should load"
    );
    // …but the main repo's checked-in copies above the worktree are skipped.
    assert!(
        !files.iter().any(|f| f.path == main.join("CLAUDE.md")),
        "main-repo CLAUDE.md must be skipped in a nested worktree"
    );
    // The duplicated rule content loads exactly once (the worktree's copy).
    let rule_hits = files
        .iter()
        .filter(|f| f.content.contains("TABS-RULE"))
        .count();
    assert_eq!(rule_hits, 1, "duplicated unconditional rule must load once");
    // The gitignored local file is still loaded despite the skip.
    assert!(
        files
            .iter()
            .any(|f| f.source == MemoryFileSource::Local && f.path == main.join("CLAUDE.local.md")),
        "main-repo CLAUDE.local.md must still load (gitignored, not duplicated)"
    );
}

#[test]
fn plain_git_repo_is_not_falsely_skipped() {
    // Control: a regular repo (no worktree) has gitRoot == canonicalRoot, so
    // nested_worktree_roots returns None and discovery is unchanged.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    git_init_repo(repo);
    std::fs::write(repo.join("CLAUDE.md"), "PLAIN").unwrap();

    let files = discover_memory_files(repo);
    assert!(
        files
            .iter()
            .any(|f| f.source == MemoryFileSource::Project && f.path == repo.join("CLAUDE.md")),
        "a plain git repo's CLAUDE.md must still load"
    );
}
