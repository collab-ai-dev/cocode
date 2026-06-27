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
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use tokio_util::sync::CancellationToken;

/// Default max results when glob_limits.max_results is None.
const DEFAULT_MAX_RESULTS: usize = 100;

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

    /// Result persistence threshold — 100_000 chars.
    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        coco_tool_runtime::ResultSizeBound::Chars(100_000)
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
        // `ctx.cwd_override`; otherwise we fall back to the process CWD.
        // Relative `path` arguments are resolved against this base.
        let cwd = ctx
            .cwd_override
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));

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

        // Read limit from ctx.glob_limits
        let max_results = ctx
            .glob_limits
            .max_results
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_MAX_RESULTS);

        let timeout_secs = ctx.tool_config.glob_timeout_seconds.max(1) as u64;

        // Move owned values into the blocking closure — no redundant clones.
        let cancel = ctx.cancel_token();
        let pattern_owned = input.pattern.clone();
        let read_ignore_patterns = ctx.tool_config.file_read_ignore_patterns.clone();
        let search_future = tokio::task::spawn_blocking(move || {
            run_glob_search(
                &pattern_owned,
                &search_path,
                &cwd,
                max_results,
                &cancel,
                &read_ignore_patterns,
            )
        });

        let (paths, truncated) =
            tokio::time::timeout(Duration::from_secs(timeout_secs), search_future)
                .await
                .map_err(|_| ToolError::Timeout {
                    timeout_ms: (timeout_secs * 1000) as i64,
                })?
                .map_err(|e| ToolError::ExecutionFailed {
                    message: format!("glob search task failed: {e}"),
                    display_data: None,
                    source: None,
                })?
                .map_err(|e| ToolError::ExecutionFailed {
                    message: e,
                    display_data: None,
                    source: None,
                })?;

        let output = if paths.is_empty() {
            "No files found".to_string()
        } else {
            let mut text = paths.join("\n");
            if truncated {
                text.push_str(
                    "\n(Results are truncated. Consider using a more specific path or pattern.)",
                );
            }
            text
        };

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

fn run_glob_search(
    pattern: &str,
    search_path: &Path,
    base_dir: &Path,
    max_results: usize,
    cancel: &CancellationToken,
    read_ignore_patterns: &[String],
) -> Result<(Vec<String>, bool), String> {
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

    // Check truncation before limiting
    let truncated = matches.len() > max_results;
    if truncated {
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

    Ok((paths, truncated))
}

#[cfg(test)]
#[path = "glob.test.rs"]
mod tests;
