use crate::tools::grep::GrepTool;
use coco_tool_runtime::DynTool;
use coco_tool_runtime::ToolUseContext;
use serde_json::json;

fn text(result: &coco_messages::ToolResult<serde_json::Value>) -> &str {
    result.data.as_str().unwrap()
}

// -----------------------------------------------------------------------
// Tool trait contract (safety / concurrency flags)
// -----------------------------------------------------------------------

#[test]
fn test_grep_is_read_only() {
    assert!(<GrepTool as DynTool>::is_read_only(
        &GrepTool,
        &serde_json::json!({"pattern": "x"})
    ));
}

#[test]
fn test_grep_is_concurrency_safe() {
    assert!(<GrepTool as DynTool>::is_concurrency_safe(
        &GrepTool,
        &serde_json::json!({"pattern": "x"})
    ));
}

#[test]
fn test_grep_is_not_destructive() {
    assert!(!<GrepTool as DynTool>::is_destructive(
        &GrepTool,
        &serde_json::json!({"pattern": "x"})
    ));
}

#[test]
fn test_grep_is_search_command() {
    let info = <GrepTool as DynTool>::is_search_or_read_command(
        &GrepTool,
        &serde_json::json!({"pattern": "x"}),
    )
    .expect("Grep should report as search command");
    assert!(info.is_search);
}

// -----------------------------------------------------------------------
// Basic modes
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_files_with_matches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn hello() {}\nfn world() {}").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn goodbye() {}").unwrap();
    std::fs::write(dir.path().join("c.txt"), "no match here").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "hello",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "files_with_matches"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("a.rs"), "should contain a.rs, got: {t}");
    assert!(!t.contains("b.rs"), "should not contain b.rs");
    assert!(
        t.contains("Found 1 file"),
        "should have 'Found 1 file' header"
    );
}

#[tokio::test]
async fn test_grep_content_mode() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test.rs"), "line1\nfn hello()\nline3").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "hello",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("fn hello()"), "should contain match: {t}");
    assert!(t.contains(":2:"), "should have line number 2: {t}");
}

#[tokio::test]
async fn test_grep_count_mode() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("multi.rs"), "fn a()\nfn b()\nfn c()").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "fn",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "count"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains(":3"), "should have count 3: {t}");
    assert!(
        t.contains("Found 3 total occurrences across 1 file."),
        "should have summary line: {t}"
    );
}

// -----------------------------------------------------------------------
// Search options
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mixed.txt"), "Hello\nhello\nHELLO").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "hello",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "count",
            "-i": true
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains(":3"), "should match all 3 lines: {t}");
}

#[tokio::test]
async fn test_grep_no_matches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty.txt"), "nothing here").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "zzzzz",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    // Default output_mode is files_with_matches, so empty result = "No files found"
    assert_eq!(t, "No files found", "should report no files found: {t}");
}

#[tokio::test]
async fn test_grep_context_lines() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ctx.txt"),
        "line1\nline2\nMATCH\nline4\nline5",
    )
    .unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content",
            "-C": 1
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("line2"), "should have before-context: {t}");
    assert!(t.contains("MATCH"), "should have match: {t}");
    assert!(t.contains("line4"), "should have after-context: {t}");
}

#[tokio::test]
async fn test_grep_single_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("single.txt");
    std::fs::write(&file, "alpha\nbeta\ngamma").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "beta",
            "path": file.to_str().unwrap(),
            "output_mode": "content"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("beta"), "should contain beta: {t}");
}

// -----------------------------------------------------------------------
// Binary detection
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_binary_skipped() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(dir.path().join("text.rs"), "fn search_me() {}").unwrap();

    // Binary file with null bytes
    std::fs::write(
        dir.path().join("binary.bin"),
        b"fn search_me() {}\x00\x00binary",
    )
    .unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "search_me",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("text.rs"), "should find text file: {t}");
    assert!(!t.contains("binary.bin"), "should skip binary file: {t}");
}

// -----------------------------------------------------------------------
// VCS directory exclusion
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_vcs_dirs_excluded() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(dir.path().join("main.rs"), "fn hello() {}").unwrap();

    let git_dir = dir.path().join(".git");
    std::fs::create_dir_all(&git_dir).unwrap();
    std::fs::write(git_dir.join("config"), "fn hello() {}").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "hello",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("main.rs"), "should find main.rs: {t}");
    assert!(!t.contains(".git"), "should exclude .git directory: {t}");
}

// -----------------------------------------------------------------------
// Context precedence: context > -C > -B/-A
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_context_precedence() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("prec.txt"), "a\nb\nc\nMATCH\ne\nf\ng").unwrap();

    let ctx = ToolUseContext::test_default();

    // `context` param should take precedence over `-C`
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content",
            "context": 1,
            "-C": 3
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    // context=1 should show 1 line before/after, NOT 3
    assert!(t.contains("c"), "should have 1 before-context line: {t}");
    assert!(t.contains("e"), "should have 1 after-context line: {t}");
    // With context=1, 'a' should NOT appear (it's 3 lines before)
    assert!(!t.contains("-1-a"), "should not have 3-line context: {t}");
}

// -----------------------------------------------------------------------
// head_limit = 0 means unlimited
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_head_limit_zero_unlimited() {
    let dir = tempfile::tempdir().unwrap();

    // Create enough files to exceed default limit of 250
    for i in 0..10 {
        std::fs::write(
            dir.path().join(format!("file{i:03}.txt")),
            format!("match_target line {i}"),
        )
        .unwrap();
    }

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "match_target",
            "path": dir.path().to_str().unwrap(),
            "head_limit": 0
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("Found 10 files"),
        "should find all 10 files: {t}"
    );
    assert!(!t.contains("pagination"), "should not truncate: {t}");
}

// -----------------------------------------------------------------------
// Multiline matching
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_multiline() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("multi.txt"), "fn hello() {\n    world\n}").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "hello.*world",
            "multiline": true,
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("hello") && t.contains("world"),
        "should match across lines: {t}"
    );
}

// -----------------------------------------------------------------------
// mtime sorting for files_with_matches
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_mtime_sorting() {
    let dir = tempfile::tempdir().unwrap();

    // Create files with distinct mtimes
    std::fs::write(dir.path().join("old.txt"), "match_here").unwrap();
    // Small sleep to ensure different mtime
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(dir.path().join("new.txt"), "match_here").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "match_here",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "files_with_matches"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    // Newest first
    let new_pos = t.find("new.txt").expect("should find new.txt");
    let old_pos = t.find("old.txt").expect("should find old.txt");
    assert!(
        new_pos < old_pos,
        "new.txt should appear before old.txt (mtime desc): {t}"
    );
}

// -----------------------------------------------------------------------
// max_result_size_bound
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_max_result_size_bound() {
    assert_eq!(
        <GrepTool as DynTool>::max_result_size_bound(&GrepTool,),
        coco_tool_runtime::ResultSizeBound::Chars(20_000),
    );
}

// -----------------------------------------------------------------------
// Type filter
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("code.rs"), "fn target() {}").unwrap();
    std::fs::write(dir.path().join("code.py"), "def target(): pass").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "type": "rust"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("code.rs"), "should match rust file: {t}");
    assert!(!t.contains("code.py"), "should not match python file: {t}");
}

// -----------------------------------------------------------------------
// Glob filter
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_glob_filter() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn target() {}").unwrap();
    std::fs::write(dir.path().join("b.txt"), "target here too").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "glob": "*.rs"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("a.rs"), "should match .rs file: {t}");
    assert!(!t.contains("b.txt"), "should not match .txt file: {t}");
}

/// A path-segment glob (`subdir/*.rs`) must filter by path relative to the
/// search root — rg `--glob` semantics. The old implementation routed the
/// `glob` param through `TypesBuilder`, which only understands basename type
/// globs and silently mishandled path-containing patterns.
#[tokio::test]
async fn test_grep_glob_path_segment_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("a.rs"), "fn target() {}").unwrap();
    std::fs::write(dir.path().join("root.rs"), "fn target() {}").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "glob": "subdir/*.rs"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("subdir/a.rs"), "should match nested file: {t}");
    assert!(!t.contains("root.rs"), "should not match root file: {t}");
}

/// A slash-less glob matches the basename at any depth, like `rg --glob`.
#[tokio::test]
async fn test_grep_glob_bare_basename() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("nested");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("Cargo.toml"), "name = \"target\"").unwrap();
    std::fs::write(dir.path().join("other.txt"), "target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "glob": "Cargo.toml"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("nested/Cargo.toml"), "should match nested: {t}");
    assert!(!t.contains("other.txt"), "should not match other: {t}");
}

/// `.agentignore` excludes files from grep too (shared IgnoreService wiring).
#[tokio::test]
async fn test_grep_agentignore_excludes() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("keep.rs"), "fn target() {}").unwrap();
    std::fs::write(dir.path().join("fixture.rs"), "fn target() {}").unwrap();
    std::fs::write(dir.path().join(".agentignore"), "fixture.rs\n").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({"pattern": "target", "path": dir.path().to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("keep.rs"), "non-ignored file present: {t}");
    assert!(
        !t.contains("fixture.rs"),
        ".agentignore should hide it from grep: {t}"
    );
}

/// file_read_ignore_patterns are folded into the grep walk as `!` negatives.
#[tokio::test]
async fn test_grep_read_ignore_patterns_exclude() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("app.rs"), "let token = 1;").unwrap();
    std::fs::write(dir.path().join("secret.env"), "token = abc").unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.tool_config.file_read_ignore_patterns = vec!["*.env".to_string()];

    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({"pattern": "token", "path": dir.path().to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("app.rs"), "normal file present: {t}");
    assert!(
        !t.contains("secret.env"),
        "read-ignore pattern should exclude: {t}"
    );
}

// -----------------------------------------------------------------------
// Offset
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_grep_offset() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), format!("target_{i}")).unwrap();
    }

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "files_with_matches",
            "offset": 3
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("Found 2 files"),
        "should show 2 files after skipping 3: {t}"
    );
}

// -----------------------------------------------------------------------
// Context break separators
// -----------------------------------------------------------------------

// -----------------------------------------------------------------------
// Output format verification
// -----------------------------------------------------------------------

/// files_with_matches: no pagination info in header when not truncated,
/// no trailing footer.
#[tokio::test]
async fn test_grep_files_header_no_pagination_when_not_truncated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "files_with_matches"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        !t.contains("limit:") && !t.contains("offset:"),
        "should not include pagination info when not truncated: {t}"
    );
    assert!(
        !t.contains("[Showing results"),
        "should not have trailing pagination block: {t}"
    );
}

/// files_with_matches: "limit: X" is appended on the SAME header line when
/// truncated.
#[tokio::test]
async fn test_grep_files_header_has_pagination_when_truncated() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "target").unwrap();
    }

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "files_with_matches",
            "head_limit": 2
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    // First line should be header with limit info
    let first_line = t.lines().next().unwrap();
    assert!(
        first_line.contains("Found 2 files") && first_line.contains("limit: 2"),
        "header should have 'Found 2 files limit: 2': {t}"
    );
}

/// content mode: no footer block when not truncated and offset == 0.
#[tokio::test]
async fn test_grep_content_no_footer_when_not_truncated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        !t.contains("[Showing results"),
        "should not have footer when not truncated: {t}"
    );
}

/// content mode: footer uses "limit: X" format when truncated.
#[tokio::test]
async fn test_grep_content_footer_format() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), format!("target_{i}")).unwrap();
    }

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content",
            "head_limit": 2
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("[Showing results with pagination = limit: 2]"),
        "should have expected footer format: {t}"
    );
}

/// count mode: summary uses "N total occurrence(s)" phrasing.
#[tokio::test]
async fn test_grep_count_summary_format() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x x x").unwrap();
    std::fs::write(dir.path().join("b.txt"), "x x").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "x",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "count"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("total occurrences across 2 files."),
        "should have plural summary: {t}"
    );
}

/// count mode singular: "1 total occurrence across 1 file."
#[tokio::test]
async fn test_grep_count_summary_singular() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("one.txt"), "singleton").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "singleton",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "count"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("Found 1 total occurrence across 1 file."),
        "should have singular summary: {t}"
    );
}

/// offset > 0 adds "offset: N" to pagination footer even without truncation.
#[tokio::test]
async fn test_grep_offset_adds_pagination_info() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..5 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "target").unwrap();
    }

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content",
            "offset": 2
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("[Showing results with pagination = offset: 2]"),
        "should have offset in footer: {t}"
    );
}

#[tokio::test]
async fn test_grep_context_breaks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("breaks.txt"),
        "match1\na\nb\nc\nd\ne\nf\nmatch2",
    )
    .unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "match",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content",
            "-A": 1
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("match1"), "should have first match: {t}");
    assert!(t.contains("match2"), "should have second match: {t}");
    assert!(t.contains("--"), "should have context break separator: {t}");
}

// -----------------------------------------------------------------------
// Concurrency & cancellation (end-to-end verification of safety model)
// -----------------------------------------------------------------------

/// Two Grep calls should execute in parallel without interference. Each call
/// owns its own state, so running them concurrently via `tokio::join!` must
/// produce the same results as running them sequentially.
#[tokio::test]
async fn test_grep_parallel_execution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "alpha_target").unwrap();
    std::fs::write(dir.path().join("beta.txt"), "beta_target").unwrap();

    let ctx = ToolUseContext::test_default();
    let path = dir.path().to_str().unwrap().to_string();

    // Fire two grep calls simultaneously searching for different patterns.
    let alpha_fut = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({"pattern": "alpha_target", "path": &path}),
        &ctx,
    );
    let beta_fut = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({"pattern": "beta_target", "path": &path}),
        &ctx,
    );
    let (alpha_res, beta_res) = tokio::join!(alpha_fut, beta_fut);

    let alpha_text = text(alpha_res.as_ref().unwrap());
    let beta_text = text(beta_res.as_ref().unwrap());

    assert!(alpha_text.contains("alpha.txt"), "alpha: {alpha_text}");
    assert!(
        !alpha_text.contains("beta.txt"),
        "alpha spilled: {alpha_text}"
    );
    assert!(beta_text.contains("beta.txt"), "beta: {beta_text}");
    assert!(
        !beta_text.contains("alpha.txt"),
        "beta spilled: {beta_text}"
    );
}

/// A pre-cancelled token should short-circuit execution. The tool must not
/// hang or return arbitrary results — it returns whatever matches were
/// collected before the cancel check fired (here: typically zero).
#[tokio::test]
async fn test_grep_respects_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "target").unwrap();
    }

    let mut ctx = ToolUseContext::test_default();
    let cancel = tokio_util::sync::CancellationToken::new();
    ctx.abort = coco_tool_runtime::ToolAbortSignal::from_turn(
        coco_tool_runtime::TurnAbortSignal::from_token(cancel.clone()),
    );
    cancel.cancel(); // fire before execute

    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .expect("cancelled grep still returns Ok with partial/empty results");

    let t = text(&result);
    // The walker should see the cancel on the first iteration and bail out
    // with zero matches, producing "No files found".
    assert_eq!(
        t, "No files found",
        "pre-cancelled Grep should return empty result: {t}"
    );
}

// -----------------------------------------------------------------------
// Empty-result format (count/content edge cases)
// -----------------------------------------------------------------------

/// Count mode with 0 matches must still emit the summary line:
/// `"No matches found\n\nFound 0 total occurrences across 0 files."`
#[tokio::test]
async fn test_grep_count_empty_includes_summary() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "no target here").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "zzzzz",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "count"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert_eq!(
        t, "No matches found\n\nFound 0 total occurrences across 0 files.",
        "count empty should include summary: {t}"
    );
}

/// Content mode with 0 matches returns bare `"No matches found"` (no footer).
#[tokio::test]
async fn test_grep_content_empty_no_footer() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "no target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "zzzzz",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert_eq!(t, "No matches found", "content empty: {t}");
}

/// Content mode with 0 matches + offset > 0 includes the pagination block.
#[tokio::test]
async fn test_grep_content_empty_with_offset_keeps_pagination() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "no target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "zzzzz",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content",
            "offset": 5
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert_eq!(
        t, "No matches found\n\n[Showing results with pagination = offset: 5]",
        "content empty with offset: {t}"
    );
}

// -----------------------------------------------------------------------
// Glob filter splitting (whitespace / comma / brace)
// -----------------------------------------------------------------------

/// `glob: "*.js *.ts"` — whitespace-separated patterns. Both extensions
/// should be matched.
#[tokio::test]
async fn test_grep_glob_filter_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.js"), "target").unwrap();
    std::fs::write(dir.path().join("b.ts"), "target").unwrap();
    std::fs::write(dir.path().join("c.py"), "target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "glob": "*.js *.ts"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("a.js"), "should match .js: {t}");
    assert!(t.contains("b.ts"), "should match .ts: {t}");
    assert!(!t.contains("c.py"), "should NOT match .py: {t}");
}

/// `glob: "*.js,*.ts"` — comma-separated patterns.
#[tokio::test]
async fn test_grep_glob_filter_comma() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.js"), "target").unwrap();
    std::fs::write(dir.path().join("b.ts"), "target").unwrap();
    std::fs::write(dir.path().join("c.py"), "target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "glob": "*.js,*.ts"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("a.js"), "should match .js: {t}");
    assert!(t.contains("b.ts"), "should match .ts: {t}");
    assert!(!t.contains("c.py"), "should NOT match .py: {t}");
}

/// `glob: "*.{js,ts}"` — brace-expansion pattern kept intact.
#[tokio::test]
async fn test_grep_glob_filter_braces() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.js"), "target").unwrap();
    std::fs::write(dir.path().join("b.ts"), "target").unwrap();
    std::fs::write(dir.path().join("c.py"), "target").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "target",
            "path": dir.path().to_str().unwrap(),
            "glob": "*.{js,ts}"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("a.js"), "should match .js: {t}");
    assert!(t.contains("b.ts"), "should match .ts: {t}");
    assert!(!t.contains("c.py"), "should NOT match .py: {t}");
}

/// `-n: false` suppresses line numbers in content-mode output.
/// A model passing `-n: false` gets `path:content` instead of
/// `path:linenum:content`.
#[tokio::test]
async fn test_grep_line_numbers_suppressed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "line1\nMATCH\nline3").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content",
            "-n": false
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("MATCH"), "should contain match: {t}");
    // Line number segment must NOT be present. Without `-n`, lines are
    // formatted as `path:content` — not `path:2:content`.
    assert!(
        !t.contains(":2:"),
        "line number segment must be suppressed: {t}"
    );
    // The path separator should still be present.
    assert!(t.contains(":MATCH"), "path:content format expected: {t}");
}

/// Default is `-n: true` — line numbers appear when omitted.
#[tokio::test]
async fn test_grep_line_numbers_default_on() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f.txt"), "line1\nMATCH\nline3").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GrepTool as DynTool>::execute(
        &GrepTool,
        json!({
            "pattern": "MATCH",
            "path": dir.path().to_str().unwrap(),
            "output_mode": "content"
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains(":2:"),
        "line number segment expected by default: {t}"
    );
}

/// `cwd_override` should redirect relative-path searches to the override.
#[tokio::test]
async fn test_grep_respects_cwd_override() {
    let outer = tempfile::tempdir().unwrap();
    let inner = tempfile::tempdir().unwrap();

    // Decoy file in the process CWD / outer — must NOT be found.
    std::fs::write(outer.path().join("decoy.txt"), "target").unwrap();
    // Real file under the override — must be found.
    std::fs::write(inner.path().join("real.txt"), "target").unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(inner.path().to_path_buf());

    // No explicit `path` → tool defaults to cwd_override.
    let result = <GrepTool as DynTool>::execute(&GrepTool, json!({"pattern": "target"}), &ctx)
        .await
        .unwrap();

    let t = text(&result);
    assert!(t.contains("real.txt"), "should find override file: {t}");
    assert!(!t.contains("decoy.txt"), "must not leak to outer: {t}");
}

// ---------------------------------------------------------------------------
// render_for_model — emit bare string instead of JSON-stringified wrapper
// ---------------------------------------------------------------------------

#[test]
fn render_for_model_unwraps_files_with_matches() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!("Found 2 files\n/abs/a.rs\n/abs/b.rs");
    let parts = <GrepTool as DynTool>::render_for_model(&GrepTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "Found 2 files\n/abs/a.rs\n/abs/b.rs");
}

#[test]
fn render_for_model_unwraps_content_mode() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!("/abs/a.rs:1:fn main() {}\n/abs/b.rs:5:fn helper() {}");
    let parts = <GrepTool as DynTool>::render_for_model(&GrepTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "/abs/a.rs:1:fn main() {}\n/abs/b.rs:5:fn helper() {}");
}

#[test]
fn render_for_model_no_matches_branch() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!("No matches found");
    let parts = <GrepTool as DynTool>::render_for_model(&GrepTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "No matches found");
}
