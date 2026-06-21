use std::fs;

use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::collect_rule_files;
use super::expand_braces;
use super::filter_rules_matching;
use super::parse_paths_field;
use super::split_paths_in_string;

#[test]
fn split_paths_basic_comma() {
    assert_eq!(split_paths_in_string("a, b,c"), vec!["a", "b", "c"]);
}

#[test]
fn split_paths_respects_brace_nesting() {
    // `{a,b}` stays together; outer commas split.
    assert_eq!(
        split_paths_in_string("src/*.{ts,tsx}, lib/*.rs"),
        vec!["src/*.{ts,tsx}", "lib/*.rs"]
    );
}

#[test]
fn split_paths_handles_empty_segments() {
    assert_eq!(split_paths_in_string(",a,,b,"), vec!["a", "b"]);
}

#[test]
fn expand_braces_simple() {
    assert_eq!(
        expand_braces("src/*.{ts,tsx}"),
        vec!["src/*.ts", "src/*.tsx"]
    );
}

#[test]
fn expand_braces_nested() {
    let mut out = expand_braces("{a,b}/{c,d}");
    out.sort();
    assert_eq!(out, vec!["a/c", "a/d", "b/c", "b/d"]);
}

#[test]
fn expand_braces_no_braces() {
    assert_eq!(expand_braces("plain/path"), vec!["plain/path"]);
}

#[test]
fn parse_paths_field_string_form() {
    let v = coco_frontmatter::FrontmatterValue::String("src/**/*.ts, lib/**/*.rs".into());
    assert_eq!(
        parse_paths_field(&v),
        Some(vec!["src/**/*.ts".into(), "lib/**/*.rs".into()])
    );
}

#[test]
fn parse_paths_field_yaml_list_form() {
    let v = coco_frontmatter::FrontmatterValue::Sequence(vec![
        coco_frontmatter::FrontmatterValue::String("a".into()),
        coco_frontmatter::FrontmatterValue::String("b/*.md".into()),
    ]);
    assert_eq!(
        parse_paths_field(&v),
        Some(vec!["a".into(), "b/*.md".into()])
    );
}

#[test]
fn parse_paths_field_strips_trailing_double_star() {
    // gitignore: `src/**` is equivalent to `src` matching path + everything inside.
    let v = coco_frontmatter::FrontmatterValue::String("src/**".into());
    assert_eq!(parse_paths_field(&v), Some(vec!["src".into()]));
}

#[test]
fn parse_paths_field_only_double_star_returns_none() {
    let v = coco_frontmatter::FrontmatterValue::String("**".into());
    assert_eq!(parse_paths_field(&v), None);
}

#[test]
fn parse_paths_field_empty_returns_none() {
    let v = coco_frontmatter::FrontmatterValue::String("   ".into());
    assert_eq!(parse_paths_field(&v), None);
}

#[test]
fn collect_rule_files_unconditional_only() {
    let dir = tempdir().unwrap();
    let rules = dir.path().join(".coco").join("rules");
    fs::create_dir_all(&rules).unwrap();
    fs::write(rules.join("uncond.md"), "no frontmatter here\n").unwrap();
    fs::write(
        rules.join("cond.md"),
        "---\npaths: \"src/**/*.rs\"\n---\nbody\n",
    )
    .unwrap();

    let uncond = collect_rule_files(&rules, false);
    assert_eq!(uncond.len(), 1);
    assert_eq!(uncond[0].path.file_name().unwrap(), "uncond.md");
    assert_eq!(uncond[0].paths, None);

    let cond = collect_rule_files(&rules, true);
    assert_eq!(cond.len(), 1);
    assert_eq!(cond[0].path.file_name().unwrap(), "cond.md");
    assert_eq!(cond[0].paths.as_deref(), Some(&["src/**/*.rs".into()][..]));
}

#[test]
fn collect_rule_files_recurses_into_subdirs() {
    let dir = tempdir().unwrap();
    let rules = dir.path().join(".coco").join("rules");
    let subdir = rules.join("nested");
    fs::create_dir_all(&subdir).unwrap();
    fs::write(rules.join("a.md"), "x").unwrap();
    fs::write(subdir.join("b.md"), "y").unwrap();

    let entries = collect_rule_files(&rules, false);
    let names: Vec<_> = entries
        .iter()
        .map(|r| r.path.file_name().unwrap().to_str().unwrap())
        .collect();
    assert!(names.contains(&"a.md"));
    assert!(names.contains(&"b.md"));
}

#[test]
fn collect_rule_files_missing_dir_returns_empty() {
    let dir = tempdir().unwrap();
    let nonexistent = dir.path().join("nope");
    assert!(collect_rule_files(&nonexistent, false).is_empty());
}

#[test]
fn collect_rule_files_case_insensitive_md_extension() {
    let dir = tempdir().unwrap();
    let rules = dir.path().join("rules");
    fs::create_dir_all(&rules).unwrap();
    fs::write(rules.join("UPPER.MD"), "x").unwrap();
    fs::write(rules.join("Mixed.Md"), "y").unwrap();

    let entries = collect_rule_files(&rules, false);
    assert_eq!(
        entries.len(),
        2,
        "case-insensitive .md extension match expected"
    );
}

#[test]
fn filter_rules_matching_simple_glob() {
    let dir = tempdir().unwrap();
    let proj = dir.path().join("proj");
    let src = proj.join("src");
    fs::create_dir_all(&src).unwrap();
    let target = src.join("foo.rs");
    fs::write(&target, "").unwrap();

    let rule = super::RuleFile {
        path: proj.join(".coco").join("rules").join("r.md"),
        raw_content: "body".into(),
        content: "body".into(),
        paths: Some(vec!["src/**/*.rs".into()]),
    };
    let kept = filter_rules_matching(vec![rule], &target, &proj);
    assert_eq!(kept.len(), 1, "rule should match src/foo.rs");
}

#[test]
fn filter_rules_matching_non_matching_glob() {
    let dir = tempdir().unwrap();
    let proj = dir.path().join("proj");
    let src = proj.join("src");
    fs::create_dir_all(&src).unwrap();
    let target = src.join("foo.rs");
    fs::write(&target, "").unwrap();

    let rule = super::RuleFile {
        path: proj.join(".coco").join("rules").join("r.md"),
        raw_content: "body".into(),
        content: "body".into(),
        paths: Some(vec!["docs/**/*.md".into()]),
    };
    let kept = filter_rules_matching(vec![rule], &target, &proj);
    assert!(kept.is_empty(), "rule should NOT match src/foo.rs");
}

#[test]
fn filter_rules_matching_brace_pattern() {
    let dir = tempdir().unwrap();
    let proj = dir.path().join("proj");
    let src = proj.join("src");
    fs::create_dir_all(&src).unwrap();
    let target = src.join("foo.tsx");
    fs::write(&target, "").unwrap();

    // Pre-expanded into separate patterns (simulates parse_paths_field output).
    let rule = super::RuleFile {
        path: proj.join(".coco").join("rules").join("r.md"),
        raw_content: "body".into(),
        content: "body".into(),
        paths: Some(vec!["src/*.ts".into(), "src/*.tsx".into()]),
    };
    let kept = filter_rules_matching(vec![rule], &target, &proj);
    assert_eq!(kept.len(), 1, "tsx pattern should match");
}

#[test]
fn filter_rules_target_outside_base_returns_empty() {
    let dir = tempdir().unwrap();
    let proj = dir.path().join("proj");
    let elsewhere = dir.path().join("other");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    let target = elsewhere.join("file.rs");
    fs::write(&target, "").unwrap();

    let rule = super::RuleFile {
        path: proj.join(".coco").join("rules").join("r.md"),
        raw_content: "body".into(),
        content: "body".into(),
        paths: Some(vec!["**".into()]),
    };
    let kept = filter_rules_matching(vec![rule], &target, &proj);
    assert!(kept.is_empty(), "target outside base must be rejected");
}
