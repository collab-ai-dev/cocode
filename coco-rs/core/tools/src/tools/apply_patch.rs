//! `apply_patch` — model-specific tool used by the gpt-5 family in lieu of
//! the `Edit` built-in. The model emits a unified-diff-style patch and the
//! runtime applies it. Visible only when
//! `ctx.tool_overrides.is_extra(ToolId::Builtin(ToolName::ApplyPatch))`.
//!
//! Backed by [`coco_apply_patch::apply_patch`] + [`coco_exec_server::LOCAL_FS`].

use std::collections::VecDeque;

use async_trait::async_trait;
use coco_apply_patch::Hunk as ApplyPatchHunk;
use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::error::ToolError;
use coco_types::ApplyPatchPreview;
use coco_types::ApplyPatchPreviewAction;
use coco_types::ApplyPatchPreviewRow;
use coco_types::ApplyPatchPreviewSign;
use coco_types::ToolCheckResult;
use coco_types::ToolDisplayData;
use coco_types::ToolId;
use coco_types::ToolName;
use coco_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

const APPLY_PATCH_PREVIEW_ROWS: usize = 200;
const APPLY_PATCH_PREVIEW_ROW_CHARS: usize = 512;

/// Typed input for [`ApplyPatchTool`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ApplyPatchInput {
    /// Patch body wrapped in `*** Begin Patch` / `*** End Patch`.
    pub patch: String,
}

/// Typed output — stdout / stderr emitted by `coco_apply_patch::apply_patch`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyPatchOutput {
    pub stdout: String,
    pub stderr: String,
}

/// Model-facing description for the freeform `apply_patch` tool. Mirrors codex
/// `create_apply_patch_freeform_tool`'s one-liner — the lark grammar
/// ([`APPLY_PATCH_LARK_GRAMMAR`]) constrains the body, so the description only
/// needs to tell the model this is a freeform (non-JSON) tool. There is no
/// upstream counterpart (gpt-5 / codex-family only).
const APPLY_PATCH_FREEFORM_DESCRIPTION: &str = "Use the `apply_patch` tool to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.";

/// The lark grammar the model's freeform output is constrained to — a verbatim
/// mirror of codex `apply_patch.lark`. The `coco_apply_patch` parser accepts
/// exactly this envelope (`*** Begin Patch` … `*** End Patch`).
const APPLY_PATCH_LARK_GRAMMAR: &str = include_str!("apply_patch.lark");

pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    type Input = ApplyPatchInput;
    coco_tool_runtime::impl_runtime_schema!(ApplyPatchInput);
    type Output = ApplyPatchOutput;

    fn to_auto_classifier_input(&self, input: &ApplyPatchInput) -> Option<String> {
        Some(input.patch.clone())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::ApplyPatch)
    }

    fn name(&self) -> &str {
        ToolName::ApplyPatch.as_str()
    }

    /// Layer-2 gate: only models that explicitly add `apply_patch` as
    /// an extra tool (e.g. gpt-5) see this tool. Other models would
    /// call it accidentally if it were registered universally.
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.tool_overrides
            .is_extra(&ToolId::Builtin(ToolName::ApplyPatch))
    }

    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        APPLY_PATCH_FREEFORM_DESCRIPTION.into()
    }

    /// `apply_patch` is the one built-in that is NOT a JSON function tool: it
    /// is the codex freeform/grammar custom tool (`ToolSpec::Freeform`), where
    /// the model emits the raw `*** Begin Patch …` envelope lark-constrained
    /// instead of a JSON object. The model's `apply_patch_tool_type` (threaded
    /// via `SchemaContext`) selects the shape; today the only variant is
    /// `Freeform`, so the match is exhaustive — a future variant would force a
    /// new arm here rather than silently defaulting.
    async fn tool_spec(
        &self,
        schema_ctx: &coco_tool_runtime::SchemaContext,
        prompt_opts: &coco_tool_runtime::PromptOptions,
    ) -> coco_tool_runtime::ToolSpec {
        match schema_ctx.apply_patch_tool_type {
            None | Some(coco_types::ApplyPatchToolType::Freeform) => {
                coco_tool_runtime::ToolSpec::Freeform(coco_tool_runtime::FreeformToolSpec {
                    name: ToolName::ApplyPatch.as_str().to_string(),
                    // Source the description from `prompt()` so that method
                    // stays the single owner of the const (and isn't dead).
                    description: self.prompt(prompt_opts).await,
                    format: coco_tool_runtime::GrammarFormat {
                        syntax: "lark".to_string(),
                        definition: APPLY_PATCH_LARK_GRAMMAR.to_string(),
                    },
                })
            }
        }
    }

    /// The freeform tool call delivers the patch as a bare string; wrap it into
    /// the `{ "patch": <raw> }` shape that [`ApplyPatchInput`] and the runtime
    /// validation schema expect, so validation + deserialization succeed.
    fn coerce_raw_string_input(&self, raw: &str) -> Option<serde_json::Value> {
        Some(serde_json::json!({ "patch": raw }))
    }

    fn description(&self, _input: &ApplyPatchInput, _options: &DescriptionOptions) -> String {
        "Apply a unified-diff-style patch to one or more files. The patch \
         body must follow the `*** Begin Patch` / `*** End Patch` envelope \
         emitted by gpt-5."
            .into()
    }

    fn is_read_only(&self, _input: &ApplyPatchInput) -> bool {
        false
    }

    async fn check_permissions(
        &self,
        input: &ApplyPatchInput,
        ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        let Ok(cwd) = apply_patch_cwd(ctx) else {
            return ToolCheckResult::Passthrough;
        };
        let Ok(parsed) = coco_apply_patch::parse_patch(&input.patch) else {
            return ToolCheckResult::Passthrough;
        };
        let path_effects = coco_apply_patch::collect_path_effects(&parsed.hunks, &cwd);
        if path_effects.permission_paths.is_empty() {
            return ToolCheckResult::Passthrough;
        }

        let cwd_str = cwd.as_path().to_string_lossy().to_string();
        let mut all_paths_to_check = Vec::new();
        for path in &path_effects.permission_paths {
            if let Some(message) = crate::check_write_root_fence(ctx, path.as_path()) {
                return ToolCheckResult::Deny { message };
            }
            let path_str = path.to_string_lossy();
            let paths_to_check =
                coco_permissions::filesystem::get_paths_for_permission_check(&path_str, &cwd_str);
            all_paths_to_check.extend(paths_to_check);
        }
        crate::tools::write_permissions::check_write_permission_for_paths(
            &all_paths_to_check,
            ctx,
            ToolName::ApplyPatch.as_str(),
            "apply a patch",
            cwd.as_path(),
        )
    }

    /// Render `{stdout, stderr}` by joining stdout + stderr with a
    /// newline (skip empty pieces). Same shape as a simplified Bash.
    fn render_for_model(&self, out: &ApplyPatchOutput) -> Vec<ToolResultContentPart> {
        let stdout = out.stdout.trim_end();
        let stderr = out.stderr.trim();
        let combined = [stdout, stderr]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<&str>>()
            .join("\n");
        vec![ToolResultContentPart::Text {
            text: combined,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: ApplyPatchInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<ApplyPatchOutput>, ToolError> {
        let patch = &input.patch;
        let preview = build_apply_patch_preview(patch);
        let display_data = preview.clone().map(ToolDisplayData::ApplyPatchPreview);

        let cwd_path = ctx
            .cwd_override
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| {
                execution_failed_with_preview(
                    "no working directory available for apply_patch",
                    display_data.clone(),
                )
            })?;
        let cwd = AbsolutePathBuf::from_absolute_path(&cwd_path).map_err(|e| {
            execution_failed_with_preview(
                format!("cwd `{}` is not absolute: {e}", cwd_path.display()),
                display_data.clone(),
            )
        })?;

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let fs: &dyn coco_exec_server::ExecutorFileSystem = coco_exec_server::LOCAL_FS.as_ref();

        let parsed = match coco_apply_patch::parse_patch(patch) {
            Ok(parsed) => Some(parsed),
            Err(_) => {
                coco_apply_patch::apply_patch(patch, &cwd, &mut stdout, &mut stderr, fs)
                    .await
                    .map_err(|e| {
                        apply_patch_error_with_preview(&stderr, e, display_data.clone())
                    })?;
                None
            }
        };
        let Some(parsed) = parsed else {
            return Ok(result_with_preview(stdout, stderr, display_data));
        };
        let path_effects = coco_apply_patch::collect_path_effects(&parsed.hunks, &cwd);

        // Execute-time guard: `canUseTool` Allow skips built-in permission
        // checks, so re-enforce the write fence immediately before mutation.
        for path in &path_effects.permission_paths {
            if let Some(message) = crate::check_write_root_fence(ctx, path.as_path()) {
                return Err(execution_failed_with_preview(message, display_data));
            }
        }

        enforce_team_memory_secret_guard(ctx, &parsed.hunks, &cwd, fs)
            .await
            .map_err(|message| execution_failed_with_preview(message, display_data.clone()))?;

        // Capture file-history snapshots before mutation, mirroring Edit/Write.
        for path in &path_effects.history_paths {
            crate::track_file_edit(ctx, path.as_path()).await;
        }

        coco_apply_patch::apply_hunks(&parsed.hunks, &cwd, &mut stdout, &mut stderr, fs)
            .await
            .map_err(|e| apply_patch_error_with_preview(&stderr, e, display_data.clone()))?;

        // Notify LSP of `didSave` per file touched so diagnostics refresh.
        // Best-effort, errors swallowed.
        for path in &path_effects.lsp_notify_paths {
            ctx.lsp.notify_save(path.as_path()).await;
        }

        Ok(result_with_preview(stdout, stderr, display_data))
    }
}

async fn enforce_team_memory_secret_guard(
    ctx: &ToolUseContext,
    hunks: &[ApplyPatchHunk],
    cwd: &AbsolutePathBuf,
    fs: &dyn coco_exec_server::ExecutorFileSystem,
) -> Result<(), String> {
    for hunk in hunks {
        match hunk {
            ApplyPatchHunk::AddFile { path, contents } => {
                let target = resolve_patch_path(path, cwd);
                if let Some(message) = crate::check_team_mem_secret(ctx, &target, contents) {
                    return Err(message);
                }
            }
            ApplyPatchHunk::DeleteFile { .. } => {}
            ApplyPatchHunk::UpdateFile {
                path,
                move_path,
                chunks,
            } => {
                let source = AbsolutePathBuf::resolve_path_against_base(path, cwd);
                let update = coco_apply_patch::unified_diff_from_chunks(&source, chunks, fs)
                    .await
                    .map_err(|e| e.to_string())?;
                let target = resolve_patch_path(move_path.as_deref().unwrap_or(path), cwd);
                if let Some(message) = crate::check_team_mem_secret(ctx, &target, update.content())
                {
                    return Err(message);
                }
            }
        }
    }
    Ok(())
}

fn apply_patch_error_with_preview(
    stderr: &[u8],
    error: coco_apply_patch::ApplyPatchError,
    display_data: Option<ToolDisplayData>,
) -> ToolError {
    // The patch may be invalid, but the bounded preview still helps the UI show
    // what the model attempted.
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(stderr),
        if stderr.is_empty() {
            error.to_string()
        } else {
            String::new()
        },
    );
    execution_failed_with_preview(message, display_data)
}

fn execution_failed_with_preview(
    message: impl Into<String>,
    display_data: Option<ToolDisplayData>,
) -> ToolError {
    if let Some(display_data) = display_data {
        ToolError::execution_failed_with_display_data(message, display_data)
    } else {
        ToolError::execution_failed(message)
    }
}

fn result_with_preview(
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    display_data: Option<ToolDisplayData>,
) -> ToolResult<ApplyPatchOutput> {
    let result = ToolResult::data(ApplyPatchOutput {
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
    });
    if let Some(display_data) = display_data {
        result.with_display_data(display_data)
    } else {
        result
    }
}

fn build_apply_patch_preview(patch: &str) -> Option<ApplyPatchPreview> {
    if patch.trim().is_empty() {
        return None;
    }

    let mut rows = BoundedPreviewRows::new(APPLY_PATCH_PREVIEW_ROWS);
    match coco_apply_patch::parse_patch(patch) {
        Ok(parsed) => {
            for hunk in parsed.hunks {
                push_hunk_preview(hunk, &mut rows);
            }
        }
        Err(_) => {
            for raw in patch.lines() {
                match raw.as_bytes().first() {
                    Some(b'+') => rows.push(ApplyPatchPreviewRow::Line {
                        sign: ApplyPatchPreviewSign::Added,
                        content: cap_preview_text(&raw[1..]),
                    }),
                    Some(b'-') => rows.push(ApplyPatchPreviewRow::Line {
                        sign: ApplyPatchPreviewSign::Removed,
                        content: cap_preview_text(&raw[1..]),
                    }),
                    _ => rows.push(ApplyPatchPreviewRow::Raw {
                        content: cap_preview_text(raw),
                    }),
                }
            }
        }
    }

    Some(rows.into_preview())
}

fn push_hunk_preview(hunk: ApplyPatchHunk, rows: &mut BoundedPreviewRows) {
    match hunk {
        ApplyPatchHunk::AddFile { path, contents } => {
            rows.push(ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Add,
                target: cap_preview_text(&path.display().to_string()),
            });
            for line in contents.lines() {
                rows.push(ApplyPatchPreviewRow::Line {
                    sign: ApplyPatchPreviewSign::Added,
                    content: cap_preview_text(line),
                });
            }
        }
        ApplyPatchHunk::DeleteFile { path } => {
            rows.push(ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Delete,
                target: cap_preview_text(&path.display().to_string()),
            });
        }
        ApplyPatchHunk::UpdateFile {
            path,
            move_path,
            chunks,
        } => {
            let target = if let Some(move_path) = move_path {
                format!("{} -> {}", path.display(), move_path.display())
            } else {
                path.display().to_string()
            };
            rows.push(ApplyPatchPreviewRow::Header {
                action: ApplyPatchPreviewAction::Update,
                target: cap_preview_text(&target),
            });
            for chunk in chunks {
                push_update_chunk_preview(&chunk.old_lines, &chunk.new_lines, rows);
            }
        }
    }
}

fn push_update_chunk_preview(
    old_lines: &[String],
    new_lines: &[String],
    rows: &mut BoundedPreviewRows,
) {
    let old = patch_lines_text(old_lines);
    let new = patch_lines_text(new_lines);
    if old == new {
        return;
    }

    let diff = similar::TextDiff::from_lines(&old, &new);
    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            similar::ChangeTag::Delete => ApplyPatchPreviewSign::Removed,
            similar::ChangeTag::Insert => ApplyPatchPreviewSign::Added,
            similar::ChangeTag::Equal => ApplyPatchPreviewSign::Context,
        };
        rows.push(ApplyPatchPreviewRow::Line {
            sign,
            content: cap_preview_text(change.value().trim_end_matches('\n')),
        });
    }
}

fn patch_lines_text(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

struct BoundedPreviewRows {
    limit: usize,
    head: Vec<ApplyPatchPreviewRow>,
    tail: VecDeque<ApplyPatchPreviewRow>,
    total: usize,
}

impl BoundedPreviewRows {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            head: Vec::with_capacity(limit / 2),
            tail: VecDeque::with_capacity(limit / 2),
            total: 0,
        }
    }

    fn push(&mut self, row: ApplyPatchPreviewRow) {
        if self.limit == 0 {
            self.total += 1;
            return;
        }

        let head_limit = self.limit.div_ceil(2);
        let tail_limit = self.limit / 2;
        if self.head.len() < head_limit {
            self.head.push(row);
        } else if tail_limit > 0 {
            if self.tail.len() == tail_limit {
                self.tail.pop_front();
            }
            self.tail.push_back(row);
        }
        self.total += 1;
    }

    fn into_preview(mut self) -> ApplyPatchPreview {
        let kept = self.head.len() + self.tail.len();
        let mut omitted = self.total.saturating_sub(kept);
        if omitted > 0 && kept + 1 > self.limit {
            let removed = self.tail.pop_front().is_some() || self.head.pop().is_some();
            if removed {
                omitted += 1;
            }
        }
        let mut rows = self.head;
        if omitted > 0 {
            rows.push(ApplyPatchPreviewRow::Omitted {
                rows: preview_rows_to_dto(omitted),
            });
        }
        rows.extend(self.tail);
        ApplyPatchPreview { rows }
    }
}

fn cap_preview_text(text: &str) -> String {
    text.chars().take(APPLY_PATCH_PREVIEW_ROW_CHARS).collect()
}

fn apply_patch_cwd(ctx: &ToolUseContext) -> Result<AbsolutePathBuf, String> {
    let cwd_path = ctx
        .cwd_override
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| "no working directory available for apply_patch".to_string())?;
    AbsolutePathBuf::from_absolute_path(&cwd_path)
        .map_err(|e| format!("cwd `{}` is not absolute: {e}", cwd_path.display()))
}

fn resolve_patch_path(path: &std::path::Path, cwd: &AbsolutePathBuf) -> std::path::PathBuf {
    AbsolutePathBuf::resolve_path_against_base(path, cwd)
        .as_path()
        .to_path_buf()
}

fn preview_rows_to_dto(rows: usize) -> i64 {
    i64::try_from(rows).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "apply_patch.test.rs"]
mod tests;
