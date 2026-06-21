use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

fn write_file(dir: &std::path::Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

#[test]
fn returns_empty_for_missing_dir() {
    let path = std::path::Path::new("/no/such/dir");
    assert!(scan_memory_files(path).is_empty());
}

#[test]
fn skips_memory_md_index_file() {
    let temp = tempdir().unwrap();
    write_file(
        temp.path(),
        "MEMORY.md",
        "# Memory Index\n\n- [a](a.md) — h\n",
    );
    write_file(
        temp.path(),
        "user_role.md",
        "---\nname: x\ndescription: d\ntype: user\n---\nbody\n",
    );
    let scanned = scan_memory_files(temp.path());
    assert_eq!(scanned.len(), 1);
    assert_eq!(scanned[0].filename, "user_role.md");
}

#[test]
fn parses_frontmatter_and_sorts_newest_first() {
    let temp = tempdir().unwrap();
    write_file(
        temp.path(),
        "old.md",
        "---\nname: old\ndescription: old desc\ntype: user\n---\nbody\n",
    );
    // Force an earlier mtime on `old.md`.
    let mtime = filetime::FileTime::from_unix_time(1_000_000, 0);
    filetime::set_file_mtime(temp.path().join("old.md"), mtime).unwrap();

    write_file(
        temp.path(),
        "new.md",
        "---\nname: new\ndescription: new desc\ntype: feedback\n---\nbody\n",
    );

    let scanned = scan_memory_files(temp.path());
    assert_eq!(scanned.len(), 2);
    assert_eq!(scanned[0].filename, "new.md");
    assert_eq!(scanned[1].filename, "old.md");
    let fm = scanned[0].frontmatter.as_ref().unwrap();
    assert_eq!(fm.memory_type, crate::store::MemoryEntryType::Feedback);
}

#[test]
fn manifest_formats_each_entry() {
    let temp = tempdir().unwrap();
    write_file(
        temp.path(),
        "x.md",
        "---\nname: x\ndescription: short hook\ntype: project\n---\nbody\n",
    );
    let scanned = scan_memory_files(temp.path());
    let m = format_memory_manifest(&scanned);
    // Line shape: `- [type] file (iso-ts): desc`
    assert!(m.starts_with("- [project] x.md ("), "got: {m}");
    assert!(m.contains("short hook"));
    // ISO-8601 with millisecond precision and trailing Z.
    let re = regex::Regex::new(r"\(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z\)").unwrap();
    assert!(re.is_match(&m), "expected ISO timestamp in: {m}");
}

#[test]
fn manifest_empty_input_returns_empty_string() {
    // An empty memory list yields `''` so the caller (extract prompt
    // builder) drops the whole `## Existing memory files` section
    // instead of rendering an empty stub.
    assert_eq!(format_memory_manifest(&[]), "");
}

#[test]
fn manifest_omits_type_tag_when_no_frontmatter() {
    let temp = tempdir().unwrap();
    write_file(temp.path(), "loose.md", "no frontmatter at all\n");
    let scanned = scan_memory_files(temp.path());
    let m = format_memory_manifest(&scanned);
    assert!(
        m.starts_with("- loose.md ("),
        "expected no [type] tag when frontmatter absent, got: {m}"
    );
    // ISO timestamp has internal `:` chars; the description suffix
    // is what we want to assert is absent — that's the `): ` pattern
    // that immediately follows the closing paren of the timestamp.
    assert!(
        !m.contains("): "),
        "expected no description suffix when frontmatter absent, got: {m}"
    );
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[test]
fn freshness_text_fresh_returns_empty() {
    // ≤1 day old → no caveat.
    assert_eq!(memory_freshness_text(now_ms()), "");
    assert_eq!(memory_freshness_text(now_ms() - 24 * 60 * 60 * 1000), "");
}

#[test]
fn freshness_text_stale_exact_text() {
    let forty_seven_days = now_ms() - 47 * 24 * 60 * 60 * 1000;
    assert_eq!(
        memory_freshness_text(forty_seven_days),
        "This memory is 47 days old. Memories are point-in-time observations, \
         not live state — claims about code behavior or file:line citations \
         may be outdated. Verify against current code before asserting as fact."
    );
}

#[test]
fn freshness_text_has_no_trailing_newline() {
    let stale = now_ms() - 10 * 24 * 60 * 60 * 1000;
    let text = memory_freshness_text(stale);
    assert!(
        !text.ends_with('\n'),
        "spacing is the caller's job, got: {text:?}"
    );
}
