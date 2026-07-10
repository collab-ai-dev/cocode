//! Glob tool — fast file-pattern search that shells out to
//! `rg --files --glob <pattern> --sort=modified`.
//!
//! # Safety & concurrency model
//!
//! - `is_read_only(_) = true` — no filesystem modifications.
//! - `is_concurrency_safe(_) = true` — no shared mutable state; two calls
//!   may execute in parallel via the `ToolExecutor`.
//! - `is_destructive` / `interrupt_behavior` / `requires_user_interaction`
//!   all use the trait defaults, which also does not
//!   override these.
//!
//! # Execution pipeline
//!
//! The walker is constructed from [`IgnoreService`] with `.gitignore`
//! disabled (matching `--no-ignore` behavior) and hidden files enabled
//! (matching `--hidden` behavior). File discovery, compiled-glob matching, and mtime
//! collection all run inside [`tokio::task::spawn_blocking`], wrapped in
//! [`tokio::time::timeout`] for a bounded 20-second budget (overridable via
//! the `COCO_GLOB_TIMEOUT_SECONDS` env var).
//!
//! # Sort order
//!
//! Files are sorted **ascending** by modification time (oldest first),
//! matching `--sort=modified` ordering. This is verified by
//! `rg --files --sort=modified` — see [`run_glob_search`].
//!
//! # Cancellation & worktree isolation
//!
//! `ctx.cancel_token()` is checked per directory entry during the walk, and
//! `ctx.cwd_override` is honored when set (for worktree-isolated subagents).

use coco_file_ignore::IgnoreConfig;
use coco_file_ignore::IgnoreService;
use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::SearchReadInfo;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;

use super::blocking_fs::BlockingFsTask;
use tokio_util::sync::CancellationToken;

/// Tool description shown to the model.
const GLOB_DESCRIPTION: &str = "\
- Fast file pattern matching tool that works with any codebase size
- Supports glob patterns like \"**/*.js\" or \"src/**/*.ts\"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns
- When you are doing an open ended search that may require multiple rounds of globbing and grepping, use the Agent tool instead";

/// Typed input for [`GlobTool`].
///
/// Doc comments propagate to the model-visible schema as field `description`s.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct GlobInput {
    /// The glob pattern to match files against
    pub pattern: String,
    /// The directory to search in. If not specified, the current
    /// working directory will be used. IMPORTANT: Omit this field to
    /// use the default directory. DO NOT enter "undefined" or "null" —
    /// simply omit it for the default behavior. Must be a valid
    /// directory path if provided.
    #[serde(default)]
    pub path: Option<String>,
}

/// Glob tool — fast file pattern matching.
/// Returns matching file paths sorted by modification time (oldest first,
/// matching `rg --files --sort=modified`).
pub struct GlobTool;

#[async_trait::async_trait]
impl Tool for GlobTool {
    type Input = GlobInput;
    coco_tool_runtime::impl_runtime_schema!(GlobInput);
    /// Output is the pre-joined model-visible text (filenames + optional
    /// truncation hint, or `"No files found"`). A typed `GlobOutput
    /// { filenames, num_files, truncated }` is a follow-up — see the
    /// `tool-result-rendering` design note. For now the renderer is a
    /// pass-through, matching the pre-typed behaviour.
    type Output = String;

    fn to_auto_classifier_input(&self, input: &GlobInput) -> Option<String> {
        Some(input.pattern.clone())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Glob)
    }

    fn name(&self) -> &str {
        ToolName::Glob.as_str()
    }

    fn search_hint(&self) -> Option<&str> {
        Some("find files by name pattern or wildcard")
    }
    fn description(&self, _input: &GlobInput, _options: &DescriptionOptions) -> String {
        GLOB_DESCRIPTION.into()
    }

    /// Model-facing tool description (schema-listing time). Returns the SAME
    /// `DESCRIPTION` as `async description()`.
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        GLOB_DESCRIPTION.into()
    }

    /// Glob never modifies state.
    fn is_read_only(&self, _input: &GlobInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }

    /// Safe to run in parallel with other concurrency-safe tools. Batches
    /// with Grep/Read/etc. via the `ToolExecutor`.
    fn is_concurrency_safe(&self, _input: &GlobInput) -> bool {
        true
    }

    /// Result persistence threshold — 100_000 bytes. Declarations are
    /// authoritative (no hidden clamp): path lists tolerate larger windows,
    /// so this deliberately exceeds the 50K default.
    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        coco_tool_runtime::ResultSizeBound::Bytes(100_000)
    }

    /// `Self::Output = String` — render emits the prebuilt text directly.
    fn render_for_model(&self, out: &String) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.clone(),
            provider_options: None,
        }]
    }

    fn get_activity_description(&self, input: &GlobInput) -> Option<String> {
        Some(format!("Searching for {pattern}", pattern = input.pattern))
    }

    fn is_search_or_read_command(&self, _input: &GlobInput) -> Option<SearchReadInfo> {
        Some(SearchReadInfo {
            is_search: true,
            ..SearchReadInfo::default()
        })
    }

    /// R6-T20: block globbing under a path that's in the ignore list.
    /// Individual results matching an ignore glob are also filtered
    /// inside `run_glob_search`.
    async fn check_permissions(
        &self,
        input: &GlobInput,
        ctx: &ToolUseContext,
    ) -> coco_types::ToolCheckResult {
        let Some(path) = input.path.as_deref() else {
            return coco_types::ToolCheckResult::Passthrough;
        };
        let matcher = crate::tools::read_permissions::file_read_ignore_matcher_from_patterns(
            &ctx.tool_config.file_read_ignore_patterns,
        );
        crate::tools::read_permissions::check_read_permission_with_matcher(
            Path::new(path),
            &matcher,
            ctx,
        )
    }

    fn validate_input(&self, input: &GlobInput, _ctx: &ToolUseContext) -> ValidationResult {
        // Schema-level validation already enforced `pattern` is a
        // present String; reject empty strings here as a semantic
        // gate (empty string means no pattern).
        if input.pattern.is_empty() {
            return ValidationResult::invalid("missing required field: pattern");
        }
        ValidationResult::Valid
    }

    async fn execute(
        &self,
        input: GlobInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<String>, ToolError> {
        // Resolve the working directory. Worktree-isolated agents set
        // `ctx.cwd_override`; otherwise use the session cwd anchor.
        // Relative `path` arguments are resolved against this base.
        let cwd = ctx.cwd_anchor().await.unwrap_or_else(|| PathBuf::from("/"));

        let search_path = match input.path.as_deref() {
            Some(p) => {
                let path = Path::new(p);
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    cwd.join(p)
                }
            }
            None => cwd.clone(),
        };

        if !search_path.exists() {
            return Err(ToolError::ExecutionFailed {
                message: format!("search path does not exist: {}", search_path.display()),
                display_data: None,
                source: None,
            });
        }

        // Result cap (config/env via `tool.search.glob_max_results`); `.max(1)`
        // guards test contexts that build `ToolConfig` without `finalize()`.
        let max_results = ctx.tool_config.search.glob_max_results.max(1) as usize;

        let timeout_secs = ctx.tool_config.glob_timeout_seconds.max(1) as u64;

        // Move owned values into the blocking closure — no redundant clones.
        let cancel = ctx.cancel_token();
        let pattern_owned = input.pattern.clone();
        let read_ignore_patterns = ctx.tool_config.file_read_ignore_patterns.clone();
        let search_task = BlockingFsTask::spawn("glob search", move || {
            run_glob_search(
                &pattern_owned,
                &search_path,
                &cwd,
                max_results,
                &cancel,
                &read_ignore_patterns,
            )
        });

        let (paths, hidden) =
            tokio::time::timeout(Duration::from_secs(timeout_secs), search_task.join())
                .await
                .map_err(|_| ToolError::Timeout {
                    timeout_ms: (timeout_secs * 1000) as i64,
                })??
                .map_err(|e| ToolError::ExecutionFailed {
                    message: e,
                    display_data: None,
                    source: None,
                })?;

        // Directory-grouping thresholds (config/env via `tool.search`); `.max(1)`
        // guards test contexts that build `ToolConfig` without `finalize()`.
        let min_paths = ctx.tool_config.search.glob_group_min_paths.max(1) as usize;
        let min_dirs = ctx.tool_config.search.glob_group_min_dirs.max(1) as usize;
        let output = format_glob_output(&paths, hidden, min_paths, min_dirs);

        Ok(ToolResult {
            data: output,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Core glob search (synchronous, runs inside spawn_blocking)
// ---------------------------------------------------------------------------

/// Returns `(paths, hidden)` where `paths` is at most `max_results` entries
/// (mtime-ascending) and `hidden` is how many matches were dropped by the cap
/// (0 = complete result). `hidden` drives the `+N more files` overflow marker.
fn run_glob_search(
    pattern: &str,
    search_path: &Path,
    base_dir: &Path,
    max_results: usize,
    cancel: &CancellationToken,
    read_ignore_patterns: &[String],
) -> Result<(Vec<String>, usize), String> {
    // The user pattern is compiled into an `ignore::overrides::Override` — the
    // exact matcher ripgrep's `--glob` uses — and applied as a per-file filter
    // (not as the walker's whitelist override). A slash-less pattern therefore
    // matches its basename at any depth (`Cargo.toml` finds every Cargo.toml,
    // matching `rg --files --glob Cargo.toml`), while `.agentignore` (pruned by
    // the walk below) still wins: a whitelist override would otherwise outrank
    // ignore files and let the model read agent-hidden files via `Glob "**/*"`.
    let pattern_matcher = crate::tools::file_filter::compile_glob_matcher(search_path, &[pattern])
        .map_err(|e| format!("invalid glob pattern: {e}"))?;

    // Glob discovery mirrors the TS reference's `--no-ignore --hidden` while
    // keeping `.agentignore` in force (see `IgnoreConfig::for_glob_discovery`).
    // File-read ignore patterns prune the walk as `!` negatives (no whitelist,
    // so `.agentignore` is preserved) — one traversal, no second filter pass.
    let ignore_service = IgnoreService::new(IgnoreConfig::for_glob_discovery());
    let mut walker_builder = ignore_service.create_walk_builder(search_path);
    let exclusions =
        crate::tools::file_filter::build_exclusion_override(search_path, &[], read_ignore_patterns)
            .map_err(|e| format!("invalid file-read ignore pattern: {e}"))?;
    walker_builder.overrides(exclusions);

    let mut matches: Vec<(PathBuf, SystemTime)> = Vec::new();

    for entry in walker_builder.build().flatten() {
        if cancel.is_cancelled() {
            break;
        }

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Per-file glob filter (rg `--glob` semantics). The walk already
        // pruned `.agentignore` / read-ignored paths.
        let rel = path.strip_prefix(search_path).unwrap_or(path);
        if !pattern_matcher.matched(rel, false).is_whitelist() {
            continue;
        }

        let mtime = path
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        matches.push((path.to_path_buf(), mtime));
    }

    // Sort ascending by modification time (oldest first), matching
    // `rg --files --sort=modified`. Do not flip to newest-first.
    matches.sort_by(|a, b| a.1.cmp(&b.1));

    // Cap before conversion; remember how many were dropped for the overflow
    // marker. The full set was collected, so the count is exact.
    let hidden = matches.len().saturating_sub(max_results);
    if hidden > 0 {
        matches.truncate(max_results);
    }

    // Convert to relative paths
    let paths: Vec<String> = matches
        .into_iter()
        .map(|(p, _)| {
            p.strip_prefix(base_dir)
                .unwrap_or(&p)
                .to_string_lossy()
                .to_string()
        })
        .collect();

    Ok((paths, hidden))
}

// ---------------------------------------------------------------------------
// Output formatting — flat below threshold, directory-grouped above it (§2.4).
//
//   src/
//     a.rs
//     b.rs
//   src/util/
//     c.rs
//
// The directory prints once per group; filenames are indented. Path
// reconstruction is a mechanical `header + name` join. Grouping only kicks in
// once repeated directory prefixes actually dominate the payload — the
// thresholds come from `tool.search.glob_group_min_{paths,dirs}` (config/env).
// ---------------------------------------------------------------------------

/// One directory's files, plus the rank of its newest member (position in the
/// mtime-ascending input) used to order groups so recency stays at the tail.
struct GlobDirGroup<'a> {
    /// Header text, e.g. `src/util/` or `./` for the root.
    dir: String,
    /// Basenames, in the input's mtime-ascending order.
    files: Vec<&'a str>,
    /// Highest input index among this group's files (newest member).
    newest_rank: usize,
}

/// Render the glob result: directory-grouped when repeated prefixes dominate
/// (≥ `min_paths` paths across ≥ `min_dirs` dirs), else flat. Appends a
/// `+N more files` marker when `hidden > 0`.
fn format_glob_output(
    paths: &[String],
    hidden: usize,
    min_paths: usize,
    min_dirs: usize,
) -> String {
    if paths.is_empty() {
        return "No files found".to_string();
    }
    // Cheap scalar gate first — only build the dir grouping (a HashMap + a
    // String per path) once the path count clears the threshold.
    let body = if paths.len() >= min_paths {
        let groups = group_glob_by_dir(paths);
        if groups.len() >= min_dirs {
            render_glob_grouped(&groups)
        } else {
            paths.join("\n")
        }
    } else {
        paths.join("\n")
    };
    if hidden > 0 {
        format!("{body}\n+{hidden} more files (use a more specific path or pattern)")
    } else {
        body
    }
}

/// Split a relative path into its directory header (`src/util/`, or `./` for a
/// root-level file) and basename.
fn split_glob_path(full: &str) -> (String, &str) {
    let path = Path::new(full);
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or(full);
    let dir_header = match path.parent().and_then(|d| d.to_str()) {
        Some("") | None => "./".to_string(),
        // Filesystem root: `parent()` is already `/`; don't append a second
        // separator (a `//` header would reconstruct to `//foo`).
        Some("/") => "/".to_string(),
        Some(d) => format!("{d}/"),
    };
    (dir_header, file_name)
}

/// Group the mtime-ascending path list by directory, then order the groups by
/// their newest member (ascending) so the globally-newest files sit in the last
/// group — recency stays local to the tail, preserving the mtime-ascending
/// contract at the group level.
fn group_glob_by_dir(paths: &[String]) -> Vec<GlobDirGroup<'_>> {
    let mut groups: Vec<GlobDirGroup> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (rank, full) in paths.iter().enumerate() {
        let (dir_header, file_name) = split_glob_path(full);
        // Clone the header only on the miss path (one alloc per directory), not
        // for every file landing in an already-seen directory.
        let gi = match index.get(&dir_header) {
            Some(&gi) => gi,
            None => {
                let gi = groups.len();
                index.insert(dir_header.clone(), gi);
                groups.push(GlobDirGroup {
                    dir: dir_header,
                    files: Vec::new(),
                    newest_rank: rank,
                });
                gi
            }
        };
        groups[gi].files.push(file_name);
        groups[gi].newest_rank = rank; // ascending input ⇒ last seen is newest
    }
    groups.sort_by_key(|g| g.newest_rank);
    groups
}

fn render_glob_grouped(groups: &[GlobDirGroup]) -> String {
    let mut out: Vec<String> = Vec::new();
    for g in groups {
        out.push(g.dir.clone());
        for f in &g.files {
            out.push(format!("  {f}"));
        }
    }
    out.join("\n")
}

#[cfg(test)]
#[path = "glob.test.rs"]
mod tests;
