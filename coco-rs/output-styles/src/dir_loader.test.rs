use super::*;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

fn write_md(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    path
}

#[test]
fn missing_dir_returns_empty() {
    let styles = load_dir_styles(
        std::path::Path::new("/nonexistent/path/output-styles"),
        OutputStyleSource::UserSettings,
    );
    assert!(styles.is_empty());
}

#[test]
fn ignores_non_md_files() {
    let dir = tempdir().unwrap();
    write_md(dir.path(), "note.txt", "not a style");
    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);
    assert!(styles.is_empty());
}

#[test]
fn loads_nested_markdown_files_recursively() {
    let dir = tempdir().unwrap();
    let nested = dir.path().join("nested").join("deeper");
    std::fs::create_dir_all(&nested).unwrap();
    write_md(&nested, "concise.md", "# Nested\nBe brief.\n");

    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);

    assert_eq!(styles.len(), 1);
    assert_eq!(styles[0].name, "concise");
    assert_eq!(styles[0].description, "Nested");
}

#[test]
fn loads_filename_as_default_name() {
    let dir = tempdir().unwrap();
    write_md(dir.path(), "concise.md", "# Concise\nBe brief.\n");
    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);
    assert_eq!(styles.len(), 1);
    assert_eq!(styles[0].name, "concise");
    assert_eq!(styles[0].description, "Concise");
    assert_eq!(styles[0].source, OutputStyleSource::UserSettings);
    assert_eq!(styles[0].prompt, "# Concise\nBe brief.");
    assert!(styles[0].keep_coding_instructions.is_none());
}

#[test]
fn frontmatter_overrides_defaults() {
    let dir = tempdir().unwrap();
    write_md(
        dir.path(),
        "raw.md",
        "---\nname: Custom Name\ndescription: Override desc\nkeep-coding-instructions: false\n---\n# Body\nPrompt body line.\n",
    );
    let styles = load_dir_styles(dir.path(), OutputStyleSource::ProjectSettings);
    assert_eq!(styles.len(), 1);
    let s = &styles[0];
    assert_eq!(s.name, "Custom Name");
    assert_eq!(s.description, "Override desc");
    assert_eq!(s.keep_coding_instructions, Some(false));
    assert_eq!(s.source, OutputStyleSource::ProjectSettings);
    assert!(s.prompt.contains("# Body"));
    assert!(s.prompt.contains("Prompt body line."));
}

#[test]
fn keep_coding_instructions_accepts_string_bool() {
    let dir = tempdir().unwrap();
    write_md(
        dir.path(),
        "stringly.md",
        "---\nkeep-coding-instructions: \"true\"\n---\nbody\n",
    );
    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);
    assert_eq!(styles[0].keep_coding_instructions, Some(true));
}

#[test]
fn keep_coding_instructions_rejects_non_ts_bool_strings() {
    let dir = tempdir().unwrap();
    write_md(
        dir.path(),
        "stringly.md",
        "---\nkeep-coding-instructions: \"TRUE\"\n---\nbody\n",
    );
    write_md(
        dir.path(),
        "numeric.md",
        "---\nkeep-coding-instructions: 1\n---\nbody\n",
    );

    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);

    assert_eq!(styles.len(), 2);
    assert!(
        styles
            .iter()
            .all(|style| style.keep_coding_instructions.is_none())
    );
}

#[test]
fn description_strips_leading_hashes() {
    let dir = tempdir().unwrap();
    write_md(dir.path(), "h.md", "###  Triple-hash heading\n");
    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);
    assert_eq!(styles[0].description, "Triple-hash heading");
}

#[test]
fn description_truncates_long_lines() {
    let dir = tempdir().unwrap();
    let long_line = "A".repeat(150);
    write_md(dir.path(), "long.md", &format!("# {long_line}\n"));
    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);
    let desc = &styles[0].description;
    // 97 chars of A + "..." = 100 chars total
    assert_eq!(desc.chars().count(), 100);
    assert!(desc.ends_with("..."));
}

#[test]
fn malformed_file_is_skipped_other_files_load() {
    let dir = tempdir().unwrap();
    // First file: valid
    write_md(dir.path(), "ok.md", "# OK\nBody\n");
    // Second file: unreadable (simulate by creating a directory with .md suffix)
    let bad = dir.path().join("bad.md");
    std::fs::create_dir_all(&bad).unwrap();
    let styles = load_dir_styles(dir.path(), OutputStyleSource::UserSettings);
    assert_eq!(styles.len(), 1);
    assert_eq!(styles[0].name, "ok");
}
