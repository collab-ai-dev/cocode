use std::collections::HashSet;
use std::fs;

use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::MAX_INCLUDE_DEPTH;
use super::expand_imports;
use super::extract_include_paths;
use super::is_text_extension;
use super::is_valid_at_path;
use super::resolve_at_path;
use super::scan_at_paths;
use super::strip_html_comments;

#[test]
fn extract_simple_relative_path() {
    let body = "See @./other.md for details.";
    let paths = extract_include_paths(body);
    assert_eq!(paths, vec!["./other.md"]);
}

#[test]
fn extract_absolute_path() {
    let body = "Load @/etc/coco/policy.md please.";
    assert_eq!(extract_include_paths(body), vec!["/etc/coco/policy.md"]);
}

#[test]
fn extract_home_path() {
    let body = "User config: @~/notes/global.md";
    assert_eq!(extract_include_paths(body), vec!["~/notes/global.md"]);
}

#[test]
fn extract_bare_path_treated_as_relative() {
    let body = "Include @other_file.md here";
    assert_eq!(extract_include_paths(body), vec!["other_file.md"]);
}

#[test]
fn skips_email_like_tokens() {
    let body = "Contact me at user@example.com about this";
    assert!(extract_include_paths(body).is_empty());
}

#[test]
fn skips_fenced_code_blocks() {
    let body = "Real: @./real.md\n```\nFake: @./fake.md\n```\nMore: @./more.md";
    let paths = extract_include_paths(body);
    assert_eq!(paths, vec!["./real.md", "./more.md"]);
}

#[test]
fn skips_tilde_fenced_code_blocks() {
    let body = "Real: @./real.md\n~~~\n@./fake.md\n~~~\nMore: @./more.md";
    let paths = extract_include_paths(body);
    assert_eq!(paths, vec!["./real.md", "./more.md"]);
}

#[test]
fn skips_inline_code_spans() {
    let body = "Real: @./real.md and `inline @./fake.md` more";
    let paths = extract_include_paths(body);
    assert_eq!(paths, vec!["./real.md"]);
}

#[test]
fn strips_fragment_suffix() {
    let body = "See @./doc.md#section for the part.";
    assert_eq!(extract_include_paths(body), vec!["./doc.md"]);
}

#[test]
fn multiple_includes_in_one_line() {
    let body = "@./a.md and @./b.md and @./c.md";
    assert_eq!(
        extract_include_paths(body),
        vec!["./a.md", "./b.md", "./c.md"]
    );
}

#[test]
fn validates_path_shapes() {
    assert!(is_valid_at_path("./rel"));
    assert!(is_valid_at_path("~/home"));
    assert!(is_valid_at_path("/abs/path"));
    assert!(is_valid_at_path("bare"));
    assert!(is_valid_at_path("name.md"));
    assert!(!is_valid_at_path(""));
    assert!(!is_valid_at_path("@nested"));
    assert!(!is_valid_at_path("/"));
    // Pure punctuation rejected (handled by scan_at_paths via boundary).
    assert!(!is_valid_at_path("!path"));
}

#[test]
fn resolves_relative_to_base() {
    let base = std::path::Path::new("/proj/src");
    assert_eq!(
        resolve_at_path("./helper.md", base),
        Some(std::path::PathBuf::from("/proj/src/helper.md"))
    );
    assert_eq!(
        resolve_at_path("helper.md", base),
        Some(std::path::PathBuf::from("/proj/src/helper.md"))
    );
}

#[test]
fn resolves_absolute_unchanged() {
    let base = std::path::Path::new("/proj");
    assert_eq!(
        resolve_at_path("/etc/policy.md", base),
        Some(std::path::PathBuf::from("/etc/policy.md"))
    );
}

#[test]
fn resolves_home_via_env() {
    // SAFETY: tests run in single-threaded runtime; env mutation is OK
    // for this test since we restore via a guard.
    let prev = std::env::var("HOME").ok();
    unsafe {
        std::env::set_var("HOME", "/tmp/fakehome");
    }
    let resolved = resolve_at_path("~/notes.md", std::path::Path::new("/proj"));
    assert_eq!(
        resolved,
        Some(std::path::PathBuf::from("/tmp/fakehome/notes.md"))
    );
    unsafe {
        match prev {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
fn text_extension_allowlist() {
    assert!(is_text_extension(std::path::Path::new("a.md")));
    assert!(is_text_extension(std::path::Path::new("a.MD"))); // case-insensitive
    assert!(is_text_extension(std::path::Path::new("a.rs")));
    assert!(is_text_extension(std::path::Path::new("README"))); // no ext → allowed
    assert!(!is_text_extension(std::path::Path::new("img.png")));
    assert!(!is_text_extension(std::path::Path::new("doc.pdf")));
    assert!(!is_text_extension(std::path::Path::new("a.exe")));
}

#[test]
fn expand_imports_loads_parent_then_child() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.md");
    let child = dir.path().join("child.md");
    fs::write(&parent, "# Parent\n@./child.md\n").unwrap();
    fs::write(&child, "child content\n").unwrap();

    let mut processed = HashSet::new();
    let entries = expand_imports(
        &parent,
        "# Parent\n@./child.md\n",
        &mut processed,
        0,
        dir.path(),
        true,
    );
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, parent);
    assert_eq!(entries[1].0, child);
    assert_eq!(entries[1].1, "child content\n");
}

#[test]
fn expand_imports_breaks_cycles() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.md");
    let b = dir.path().join("b.md");
    fs::write(&a, "@./b.md\n").unwrap();
    fs::write(&b, "@./a.md\n").unwrap();

    let mut processed = HashSet::new();
    let entries = expand_imports(&a, "@./b.md\n", &mut processed, 0, dir.path(), true);
    // a, then b — b's @./a.md is rejected by processed set.
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].0, a);
    assert_eq!(entries[1].0, b);
}

#[test]
fn expand_imports_caps_at_max_depth() {
    let dir = tempdir().unwrap();
    // Build chain a → b → c → d → e → f → g (7 levels).
    let names: Vec<_> = (0..7).map(|i| format!("file{i}.md")).collect();
    let paths: Vec<_> = names.iter().map(|n| dir.path().join(n)).collect();
    for (i, path) in paths.iter().enumerate() {
        let body = if i + 1 < paths.len() {
            format!("@./{}\n", names[i + 1])
        } else {
            "tail\n".into()
        };
        fs::write(path, &body).unwrap();
    }

    let mut processed = HashSet::new();
    let entries = expand_imports(
        &paths[0],
        &fs::read_to_string(&paths[0]).unwrap(),
        &mut processed,
        0,
        dir.path(),
        true,
    );
    // Should load up to MAX_INCLUDE_DEPTH (5) levels of children + the
    // parent itself = 6 entries. The 7th (file6.md) is reached at depth
    // 6 which is past the cap, so file5's @import doesn't recurse.
    assert!(
        entries.len() <= (MAX_INCLUDE_DEPTH as usize) + 1,
        "expected ≤ {} entries (parent + MAX_INCLUDE_DEPTH children), got {}",
        MAX_INCLUDE_DEPTH + 1,
        entries.len()
    );
}

#[test]
fn expand_imports_skips_binary_extensions() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.md");
    let img = dir.path().join("logo.png");
    fs::write(&parent, "@./logo.png\n").unwrap();
    fs::write(&img, b"\x89PNG").unwrap();

    let mut processed = HashSet::new();
    let entries = expand_imports(
        &parent,
        "@./logo.png\n",
        &mut processed,
        0,
        dir.path(),
        true,
    );
    assert_eq!(entries.len(), 1, "binary file must not be loaded");
    assert_eq!(entries[0].0, parent);
}

#[test]
fn strip_html_comments_removes_block_comments() {
    assert_eq!(strip_html_comments("a <!-- c --> b"), "a  b");
    // Multi-line comment.
    assert_eq!(strip_html_comments("a\n<!--\nx\n-->\nb"), "a\n\nb");
}

#[test]
fn strip_html_comments_preserves_code_fences_and_unclosed() {
    // A comment inside a fenced code block is kept verbatim.
    let fenced = "```\n<!-- keep -->\n```";
    assert_eq!(strip_html_comments(fenced), fenced);
    // A dangling `<!--` with no `-->` is left intact (can't eat the file).
    assert_eq!(strip_html_comments("real <!-- oops"), "real <!-- oops");
}

#[test]
fn expand_imports_strips_html_comments_from_loaded_content() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("CLAUDE.md");
    let body = "Visible.\n<!-- secret authoring note -->\nMore.\n";
    fs::write(&parent, body).unwrap();

    let mut processed = HashSet::new();
    let entries = expand_imports(&parent, body, &mut processed, 0, dir.path(), false);
    assert!(!entries[0].1.contains("secret authoring note"));
    assert!(entries[0].1.contains("Visible."));
    assert!(entries[0].1.contains("More."));
}

#[test]
fn expand_imports_gates_external_imports() {
    let cwd = tempdir().unwrap();
    let outside = tempdir().unwrap();
    let parent = cwd.path().join("CLAUDE.md");
    let secret = outside.path().join("secret.md");
    fs::write(&secret, "SECRET\n").unwrap();
    let body = format!("@{}\n", secret.display());
    fs::write(&parent, &body).unwrap();

    // Project memory (allow_external=false) must NOT pull a file outside cwd.
    let mut processed = HashSet::new();
    let gated = expand_imports(&parent, &body, &mut processed, 0, cwd.path(), false);
    assert_eq!(
        gated.len(),
        1,
        "external @import must be skipped when gated"
    );

    // User-global memory (allow_external=true) may include it.
    let mut processed2 = HashSet::new();
    let allowed = expand_imports(&parent, &body, &mut processed2, 0, cwd.path(), true);
    assert_eq!(allowed.len(), 2, "external @import loads when allowed");
    assert!(allowed[1].1.contains("SECRET"));
}

#[test]
fn expand_imports_skips_missing_files_silently() {
    let dir = tempdir().unwrap();
    let parent = dir.path().join("parent.md");
    fs::write(&parent, "@./does_not_exist.md\n").unwrap();

    let mut processed = HashSet::new();
    let entries = expand_imports(
        &parent,
        "@./does_not_exist.md\n",
        &mut processed,
        0,
        dir.path(),
        true,
    );
    assert_eq!(entries.len(), 1, "missing imports must not error");
    assert_eq!(entries[0].0, parent);
}

#[test]
fn scan_at_paths_starts_at_word_boundary_only() {
    // Non-boundary `@`: e.g. `foo@bar.com` should not match
    let paths = scan_at_paths("user@example.com");
    assert!(paths.is_empty(), "in-word `@` must not produce a match");

    // After whitespace: matches.
    let paths = scan_at_paths("see @./file.md");
    assert_eq!(paths, vec!["./file.md"]);
}
