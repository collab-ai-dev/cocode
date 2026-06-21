use std::collections::HashSet;
use std::fs;

use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::LoadedMemoryEntry;
use super::directories_to_process;
use super::traverse_for_file;
use crate::memory_discovery::MemoryFileSource;

/// Filter the entries that came from a particular directory's per-dir
/// load — used to assert the per-dir contents independently of how the
/// caller dedupes/canonicalizes paths.
fn entries_in<'a>(
    dir: &std::path::Path,
    entries: &'a [LoadedMemoryEntry],
) -> Vec<&'a LoadedMemoryEntry> {
    entries
        .iter()
        .filter(|e| e.path.parent() == Some(dir) || e.path.starts_with(dir.join(".coco")))
        .collect()
}

#[test]
fn directories_to_process_split_correctly() {
    // Build /root/proj/src/auth, treat /root/proj as CWD.
    // Trigger file: /root/proj/src/auth/handler.rs
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let src = proj.join("src");
    let auth = src.join("auth");
    fs::create_dir_all(&auth).unwrap();
    let file = auth.join("handler.rs");
    fs::write(&file, "").unwrap();

    let (nested, cwd_level) = directories_to_process(&file, &proj);

    let nested_canon: Vec<_> = nested.iter().map(|p| p.canonicalize().unwrap()).collect();
    assert_eq!(
        nested_canon,
        vec![src.canonicalize().unwrap(), auth.canonicalize().unwrap(),],
        "nested_dirs must be CWD-exclusive, file-parent-inclusive, in CWD→file order"
    );

    // cwd_level_dirs ends with CWD (proj). Must NOT include nested dirs.
    assert!(
        cwd_level
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .any(|p| p == proj.canonicalize().unwrap()),
        "cwd_level_dirs must include CWD"
    );
    assert!(
        !cwd_level
            .iter()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
            .any(|p| p == src.canonicalize().unwrap()),
        "cwd_level_dirs must NOT include descendants of CWD"
    );
}

#[test]
fn directories_to_process_file_outside_cwd() {
    // Trigger file outside CWD → empty nested_dirs (only Phase 1+4 fire).
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let elsewhere = root.path().join("other");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    let outside_file = elsewhere.join("file.txt");
    fs::write(&outside_file, "").unwrap();

    let (nested, cwd_level) = directories_to_process(&outside_file, &proj);
    assert!(
        nested.is_empty(),
        "file outside CWD must yield empty nested_dirs"
    );
    assert!(
        !cwd_level.is_empty(),
        "cwd_level_dirs always has at least CWD"
    );
}

#[test]
fn directories_to_process_file_directly_in_cwd() {
    // Trigger file directly in CWD → empty nested_dirs (CWD is exclusive).
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let file = proj.join("main.rs");
    fs::write(&file, "").unwrap();

    let (nested, _) = directories_to_process(&file, &proj);
    assert!(
        nested.is_empty(),
        "file directly in CWD must yield empty nested_dirs"
    );
}

#[test]
fn traverse_loads_intermediate_claude_md() {
    // CWD = /proj. Reading /proj/src/auth/handler.rs should load
    // /proj/src/CLAUDE.md and /proj/src/auth/CLAUDE.md, but NOT
    // /proj/CLAUDE.md (that's the eager phase's job).
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let src = proj.join("src");
    let auth = src.join("auth");
    fs::create_dir_all(&auth).unwrap();
    fs::write(proj.join("CLAUDE.md"), "proj").unwrap();
    fs::write(src.join("CLAUDE.md"), "src").unwrap();
    fs::write(auth.join("CLAUDE.md"), "auth").unwrap();
    let trigger = auth.join("handler.rs");
    fs::write(&trigger, "").unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);

    let paths: Vec<_> = entries
        .iter()
        .map(|e| e.path.canonicalize().unwrap())
        .collect();
    assert!(
        paths.contains(&src.join("CLAUDE.md").canonicalize().unwrap()),
        "expected src/CLAUDE.md in traversal, got: {paths:?}"
    );
    assert!(
        paths.contains(&auth.join("CLAUDE.md").canonicalize().unwrap()),
        "expected src/auth/CLAUDE.md in traversal, got: {paths:?}"
    );
    assert!(
        !paths.contains(&proj.join("CLAUDE.md").canonicalize().unwrap()),
        "proj/CLAUDE.md is the eager phase's responsibility — must NOT appear in traversal output"
    );
}

#[test]
fn traverse_loads_agents_md_alongside_claude_md() {
    // Both filenames in the same directory should both load.
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.md"), "claude").unwrap();
    fs::write(sub.join("AGENTS.md"), "agents").unwrap();
    let trigger = sub.join("file.rs");
    fs::write(&trigger, "").unwrap();
    // Production code canonicalizes paths in `directories_to_process`,
    // which on macOS resolves `/var/folders/...` (TempDir's default
    // root) to `/private/var/folders/...`. The test must canonicalize
    // `sub` before path-equality comparison or the assertion never
    // matches on macOS.
    let sub_canonical = std::fs::canonicalize(&sub).unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);

    let names: Vec<&str> = entries
        .iter()
        .filter(|e| e.path.parent() == Some(sub_canonical.as_path()))
        .map(|e| e.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert!(
        names.contains(&"CLAUDE.md"),
        "missing CLAUDE.md, got {names:?}"
    );
    assert!(
        names.contains(&"AGENTS.md"),
        "missing AGENTS.md, got {names:?}"
    );
}

#[test]
fn traverse_case_insensitive() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("Claude.md"), "x").unwrap();
    let trigger = sub.join("f.rs");
    fs::write(&trigger, "").unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);
    assert!(
        entries
            .iter()
            .any(|e| e.path.file_name().unwrap() == std::ffi::OsStr::new("Claude.md")),
        "Claude.md (mixed case) must match"
    );
}

#[test]
fn traverse_loads_local_files() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.local.md"), "local").unwrap();
    let trigger = sub.join("f.rs");
    fs::write(&trigger, "").unwrap();
    // See `traverse_loads_agents_md_alongside_claude_md` for the
    // macOS canonicalize-or-mismatch rationale.
    let sub_canonical = std::fs::canonicalize(&sub).unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);
    let local = entries.iter().find(|e| {
        e.source == MemoryFileSource::Local && e.path.parent() == Some(sub_canonical.as_path())
    });
    assert!(
        local.is_some(),
        "expected Local entry for sub/CLAUDE.local.md"
    );
}

#[test]
fn traverse_loads_dot_coco_config() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    let cfg = sub.join(".coco");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("CLAUDE.md"), "config").unwrap();
    let trigger = sub.join("f.rs");
    fs::write(&trigger, "").unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);
    let cfg_entry = entries
        .iter()
        .find(|e| e.source == MemoryFileSource::ProjectConfig);
    assert!(
        cfg_entry.is_some(),
        "expected ProjectConfig for sub/.coco/CLAUDE.md"
    );
}

#[test]
fn traverse_dedups_via_loaded_set() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.md"), "x").unwrap();
    let trigger = sub.join("f.rs");
    fs::write(&trigger, "").unwrap();

    let mut loaded = HashSet::new();
    let first = traverse_for_file(&trigger, &proj, &mut loaded);
    assert!(!first.is_empty());

    // Second call with the same `loaded` set should return nothing —
    // the path is already in the dedup set.
    let second = traverse_for_file(&trigger, &proj, &mut loaded);
    assert!(
        second.is_empty(),
        "second traversal with same dedup set must be a no-op, got {second:?}"
    );
}

#[test]
fn traverse_outside_cwd_returns_empty() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let elsewhere = root.path().join("other");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    fs::write(elsewhere.join("CLAUDE.md"), "x").unwrap();
    let trigger = elsewhere.join("file.rs");
    fs::write(&trigger, "").unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);
    // Phase 3 contributes nothing for files outside CWD; Phase 1/4 are
    // currently stubs. Net should be empty.
    assert!(
        entries.is_empty(),
        "file outside CWD with stubbed Phase 1/4 must yield empty entries, got {entries:?}"
    );
}

#[test]
fn traverse_orders_cwd_first_then_file() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let src = proj.join("src");
    let auth = src.join("auth");
    fs::create_dir_all(&auth).unwrap();
    fs::write(src.join("CLAUDE.md"), "src").unwrap();
    fs::write(auth.join("CLAUDE.md"), "auth").unwrap();
    let trigger = auth.join("f.rs");
    fs::write(&trigger, "").unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);
    let src_idx = entries
        .iter()
        .position(|e| {
            e.path.canonicalize().ok() == Some(src.join("CLAUDE.md").canonicalize().unwrap())
        })
        .expect("missing src/CLAUDE.md");
    let auth_idx = entries
        .iter()
        .position(|e| {
            e.path.canonicalize().ok() == Some(auth.join("CLAUDE.md").canonicalize().unwrap())
        })
        .expect("missing auth/CLAUDE.md");
    assert!(
        src_idx < auth_idx,
        "src/CLAUDE.md (CWD-side) must load before auth/CLAUDE.md (file-side); TS later=higher-attention"
    );
}

#[test]
fn loaded_entries_contain_file_contents() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.md"), "hello world").unwrap();
    let trigger = sub.join("f.rs");
    fs::write(&trigger, "").unwrap();
    // See `traverse_loads_agents_md_alongside_claude_md` for the
    // macOS canonicalize-or-mismatch rationale.
    let sub_canonical = std::fs::canonicalize(&sub).unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);
    let entry = entries
        .iter()
        .find(|e| e.path.parent() == Some(sub_canonical.as_path()))
        .expect("missing entry");
    assert_eq!(entry.content, "hello world");
    assert_eq!(entry.raw_content, "hello world");
}

#[test]
fn loaded_entries_keep_raw_content_when_visible_content_is_stripped() {
    let root = tempdir().unwrap();
    let proj = root.path().join("proj");
    let sub = proj.join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("CLAUDE.md"), "hello\n<!-- hidden -->\n").unwrap();
    let trigger = sub.join("f.rs");
    fs::write(&trigger, "").unwrap();

    let mut loaded = HashSet::new();
    let entries = traverse_for_file(&trigger, &proj, &mut loaded);
    let entry = entries
        .iter()
        .find(|e| e.path.file_name().unwrap() == std::ffi::OsStr::new("CLAUDE.md"))
        .expect("missing entry");
    assert_eq!(entry.content, "hello\n\n");
    assert_eq!(entry.raw_content, "hello\n<!-- hidden -->\n");
}

#[test]
fn unused_helper_silences_dead_code() {
    // Touch the test-only helper so an unused-fn warning doesn't trip.
    let dir = tempdir().unwrap();
    let _ = entries_in(dir.path(), &[]);
}
