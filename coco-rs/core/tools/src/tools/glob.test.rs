use crate::tools::glob::GlobTool;
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
fn test_glob_is_read_only() {
    assert!(<GlobTool as DynTool>::is_read_only(
        &GlobTool,
        &serde_json::json!({"pattern": "*"})
    ));
}

#[test]
fn test_glob_is_concurrency_safe() {
    assert!(<GlobTool as DynTool>::is_concurrency_safe(
        &GlobTool,
        &serde_json::json!({"pattern": "*"})
    ));
}

#[test]
fn test_glob_is_not_destructive() {
    assert!(!<GlobTool as DynTool>::is_destructive(
        &GlobTool,
        &serde_json::json!({"pattern": "*"})
    ));
}

#[test]
fn test_glob_is_search_command() {
    let info = <GlobTool as DynTool>::is_search_or_read_command(
        &GlobTool,
        &serde_json::json!({"pattern": "*"}),
    )
    .expect("Glob should report as search command");
    assert!(info.is_search);
}

// -----------------------------------------------------------------------
// Basic matching
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_glob_pattern_match() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn test() {}").unwrap();
    std::fs::write(dir.path().join("c.txt"), "text file").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*.rs",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("a.rs"), "should match a.rs: {t}");
    assert!(t.contains("b.rs"), "should match b.rs: {t}");
    assert!(!t.contains("c.txt"), "should not match c.txt: {t}");
}

#[tokio::test]
async fn test_glob_recursive_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("src");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(dir.path().join("root.rs"), "root").unwrap();
    std::fs::write(sub.join("nested.rs"), "nested").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "**/*.rs",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("root.rs"), "should find root file: {t}");
    assert!(t.contains("nested.rs"), "should find nested file: {t}");
}

#[tokio::test]
async fn test_glob_no_matches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*.xyz",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert_eq!(t, "No files found");
}

#[tokio::test]
async fn test_glob_invalid_pattern() {
    let dir = tempfile::tempdir().unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "[invalid",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await;

    assert!(result.is_err(), "should error on invalid glob pattern");
}

// -----------------------------------------------------------------------
// Behavioral tests
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_glob_hidden_files_included() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(".hidden"), "secret").unwrap();
    std::fs::write(dir.path().join("visible"), "public").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains(".hidden"),
        "should include hidden files (TS: --hidden): {t}"
    );
}

#[tokio::test]
async fn test_glob_no_gitignore_by_default() {
    let dir = tempfile::tempdir().unwrap();

    // Init minimal git repo so .gitignore takes effect
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(dir.path().join("keep.txt"), "keep").unwrap();
    std::fs::write(dir.path().join("debug.log"), "log").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("debug.log"),
        "should NOT respect .gitignore (TS: --no-ignore): {t}"
    );
}

#[tokio::test]
async fn test_glob_mtime_sorting_matches_ts() {
    // GlobTool sorts files ASCENDING by mtime (oldest first).
    // This test verifies that ordering.
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(dir.path().join("old.txt"), "old").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    std::fs::write(dir.path().join("new.txt"), "new").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*.txt",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    let new_pos = t.find("new.txt").expect("should find new.txt");
    let old_pos = t.find("old.txt").expect("should find old.txt");
    assert!(
        old_pos < new_pos,
        "oldest file should appear first (TS --sort=modified behavior): {t}"
    );
}

#[tokio::test]
async fn test_glob_truncation_message() {
    let dir = tempfile::tempdir().unwrap();

    for i in 0..5 {
        std::fs::write(
            dir.path().join(format!("file{i:03}.txt")),
            format!("content {i}"),
        )
        .unwrap();
    }

    let mut ctx = ToolUseContext::test_default();
    ctx.glob_limits.max_results = Some(3);

    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*.txt",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("Results are truncated"),
        "should have TS truncation message: {t}"
    );
}

// -----------------------------------------------------------------------
// rg --glob parity: slash-less patterns match the basename at any depth
// -----------------------------------------------------------------------

/// Regression for the "No files found" bug: a bare filename pattern like
/// `Cargo.toml` (no `/`, no wildcard) must match every `Cargo.toml` in the
/// tree, matching `rg --files --glob Cargo.toml`. The old globset full-path
/// matcher only matched a file literally named `Cargo.toml` at the root.
#[tokio::test]
async fn test_glob_bare_basename_matches_at_any_depth() {
    let dir = tempfile::tempdir().unwrap();
    let app = dir.path().join("app/cli");
    let core = dir.path().join("core/tools");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&core).unwrap();
    // NOTE: deliberately NO Cargo.toml at the root — only nested ones, which
    // is exactly the shape that produced "No files found".
    std::fs::write(app.join("Cargo.toml"), "[package]").unwrap();
    std::fs::write(core.join("Cargo.toml"), "[package]").unwrap();
    std::fs::write(dir.path().join("README.md"), "readme").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({"pattern": "Cargo.toml", "path": dir.path().to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(
        t.contains("app/cli/Cargo.toml"),
        "should find nested app: {t}"
    );
    assert!(
        t.contains("core/tools/Cargo.toml"),
        "should find nested core: {t}"
    );
    assert!(!t.contains("README.md"), "should not match README: {t}");
}

/// A path-segment glob (`subdir/*.rs`) is matched relative to the search
/// root, like ripgrep's `--glob`.
#[tokio::test]
async fn test_glob_path_segment_pattern() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("subdir");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("a.rs"), "").unwrap();
    std::fs::write(dir.path().join("root.rs"), "").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({"pattern": "subdir/*.rs", "path": dir.path().to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("subdir/a.rs"), "should match subdir file: {t}");
    assert!(!t.contains("root.rs"), "should not match root file: {t}");
}

/// Brace alternation (`*.{rs,txt}`) works like rg `--glob '*.{rs,txt}'`.
#[tokio::test]
async fn test_glob_brace_expansion() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.rs"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();
    std::fs::write(dir.path().join("c.md"), "").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({"pattern": "*.{rs,txt}", "path": dir.path().to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("a.rs"), "should match .rs: {t}");
    assert!(t.contains("b.txt"), "should match .txt: {t}");
    assert!(!t.contains("c.md"), "should not match .md: {t}");
}

// -----------------------------------------------------------------------
// .agentignore + read-ignore folded into the single walk
// -----------------------------------------------------------------------

/// `.agentignore` excludes files from Glob even though Glob runs in
/// `--no-ignore` mode (gitignore/.ignore off).
#[tokio::test]
async fn test_glob_agentignore_excludes_in_no_ignore_mode() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(dir.path().join("build.log"), "").unwrap(); // gitignored
    std::fs::write(dir.path().join("fixture.json"), "").unwrap(); // agentignored
    std::fs::write(dir.path().join("real.json"), "").unwrap();
    std::fs::write(dir.path().join(".agentignore"), "fixture.json\n").unwrap();

    let ctx = ToolUseContext::test_default();
    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({"pattern": "*", "path": dir.path().to_str().unwrap()}),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    assert!(t.contains("build.log"), "gitignore is off in Glob: {t}");
    assert!(t.contains("real.json"), "non-ignored file present: {t}");
    assert!(
        !t.contains("fixture.json"),
        ".agentignore must hide the fixture even in --no-ignore mode: {t}"
    );
}

/// file_read_ignore_patterns are folded into the walk as `!` negatives.
#[tokio::test]
async fn test_glob_read_ignore_patterns_exclude() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("app.rs"), "").unwrap();
    std::fs::write(dir.path().join("secret.env"), "").unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.tool_config.file_read_ignore_patterns = vec!["*.env".to_string()];

    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({"pattern": "*", "path": dir.path().to_str().unwrap()}),
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
// max_result_size_bound
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_glob_max_result_size_bound() {
    assert_eq!(
        <GlobTool as DynTool>::max_result_size_bound(&GlobTool,),
        coco_tool_runtime::ResultSizeBound::Chars(100_000),
    );
}

// -----------------------------------------------------------------------
// Reads glob_limits from context
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_glob_reads_glob_limits() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..10 {
        std::fs::write(
            dir.path().join(format!("f{i}.rs")),
            format!("fn f{i}() {{}}"),
        )
        .unwrap();
    }

    let mut ctx = ToolUseContext::test_default();
    ctx.glob_limits.max_results = Some(5);

    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*.rs",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .unwrap();

    let t = text(&result);
    let file_count = t.lines().filter(|l| l.ends_with(".rs")).count();
    assert_eq!(file_count, 5, "should limit to 5 results: {t}");
    assert!(
        t.contains("Results are truncated"),
        "should be truncated: {t}"
    );
}

// -----------------------------------------------------------------------
// Concurrency & cancellation
// -----------------------------------------------------------------------

/// Two Glob calls should execute in parallel without interference.
#[tokio::test]
async fn test_glob_parallel_execution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.rs"), "").unwrap();
    std::fs::write(dir.path().join("doc.md"), "").unwrap();

    let ctx = ToolUseContext::test_default();
    let path = dir.path().to_str().unwrap().to_string();

    let rs_fut =
        <GlobTool as DynTool>::execute(&GlobTool, json!({"pattern": "*.rs", "path": &path}), &ctx);
    let md_fut =
        <GlobTool as DynTool>::execute(&GlobTool, json!({"pattern": "*.md", "path": &path}), &ctx);
    let (rs_res, md_res) = tokio::join!(rs_fut, md_fut);

    let rs_text = text(rs_res.as_ref().unwrap());
    let md_text = text(md_res.as_ref().unwrap());

    assert!(rs_text.contains("main.rs"), "rs: {rs_text}");
    assert!(!rs_text.contains("doc.md"), "rs spilled: {rs_text}");
    assert!(md_text.contains("doc.md"), "md: {md_text}");
    assert!(!md_text.contains("main.rs"), "md spilled: {md_text}");
}

/// A pre-cancelled token should short-circuit glob walking.
#[tokio::test]
async fn test_glob_respects_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..20 {
        std::fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
    }

    let mut ctx = ToolUseContext::test_default();
    let cancel = tokio_util::sync::CancellationToken::new();
    ctx.abort = coco_tool_runtime::ToolAbortSignal::from_turn(
        coco_tool_runtime::TurnAbortSignal::from_token(cancel.clone()),
    );
    cancel.cancel();

    let result = <GlobTool as DynTool>::execute(
        &GlobTool,
        json!({
            "pattern": "*.txt",
            "path": dir.path().to_str().unwrap()
        }),
        &ctx,
    )
    .await
    .expect("cancelled glob should still return Ok");

    let t = text(&result);
    assert_eq!(
        t, "No files found",
        "pre-cancelled Glob should return empty result: {t}"
    );
}

/// `cwd_override` redirects the default search path.
#[tokio::test]
async fn test_glob_respects_cwd_override() {
    let outer = tempfile::tempdir().unwrap();
    let inner = tempfile::tempdir().unwrap();

    std::fs::write(outer.path().join("decoy.rs"), "").unwrap();
    std::fs::write(inner.path().join("real.rs"), "").unwrap();

    let mut ctx = ToolUseContext::test_default();
    ctx.cwd_override = Some(inner.path().to_path_buf());

    let result = <GlobTool as DynTool>::execute(&GlobTool, json!({"pattern": "*.rs"}), &ctx)
        .await
        .unwrap();

    let t = text(&result);
    assert!(t.contains("real.rs"), "should find override file: {t}");
    assert!(!t.contains("decoy.rs"), "must not leak to outer: {t}");
}

// ---------------------------------------------------------------------------
// render_for_model — emit bare string instead of JSON-stringified wrapper
// ---------------------------------------------------------------------------

#[test]
fn render_for_model_unwraps_data_string_into_text_part() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!("Found 2 files\n/abs/a.rs\n/abs/b.rs");
    let parts = <GlobTool as DynTool>::render_for_model(&GlobTool, &data);
    assert_eq!(parts.len(), 1);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    // Bare string — no escaped \n, no surrounding quotes.
    assert_eq!(text, "Found 2 files\n/abs/a.rs\n/abs/b.rs");
}

#[test]
fn render_for_model_no_files_branch() {
    use coco_tool_runtime::ToolResultContentPart;
    let data = json!("No files found");
    let parts = <GlobTool as DynTool>::render_for_model(&GlobTool, &data);
    let ToolResultContentPart::Text { text, .. } = &parts[0] else {
        panic!("expected Text part");
    };
    assert_eq!(text, "No files found");
}
