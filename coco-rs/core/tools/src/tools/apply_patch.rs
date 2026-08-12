//! `apply_patch` — model-specific tool used by the gpt-5 family in lieu of
//! the `Edit` built-in. The model emits a unified-diff-style patch and the
//! runtime applies it. Visible only when
//! `ctx.tool_overrides.is_extra(ToolId::Builtin(ToolName::ApplyPatch))`.
//!
//! Parsing and diagnostics match [`coco_apply_patch::apply_patch`]. Valid
//! patches go through [`coco_apply_patch::PreparedPatch`] so policy checks and
//! writes use one immutable, canonicalized plan.

use std::collections::VecDeque;

use async_trait::async_trait;
use coco_apply_patch::Hunk as ApplyPatchHunk;
use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PreparedToolState;
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
use coco_utils_path_uri::PathUri;
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
const APPLY_PATCH_FREEFORM_DESCRIPTION: &str = "The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON.";

/// The lark grammar the model's freeform output is constrained to — a verbatim
/// mirror of codex `apply_patch.lark`. The `coco_apply_patch` parser accepts
/// exactly this envelope (`*** Begin Patch` … `*** End Patch`).
const APPLY_PATCH_LARK_GRAMMAR: &str = include_str!("apply_patch.lark");

#[derive(Clone)]
pub struct ApplyPatchTool {
    fs: std::sync::Arc<dyn coco_exec_server::ExecutorFileSystem>,
    sandbox: Option<coco_exec_server::FileSystemSandboxContext>,
}

enum PreparedApplyPatch {
    DeniedByWriteFence { message: String },
    Paths(Box<coco_apply_patch::PreparedPatchPaths>),
}

impl ApplyPatchTool {
    pub fn new(
        fs: std::sync::Arc<dyn coco_exec_server::ExecutorFileSystem>,
        sandbox: Option<coco_exec_server::FileSystemSandboxContext>,
    ) -> Self {
        Self { fs, sandbox }
    }
}

impl Default for ApplyPatchTool {
    fn default() -> Self {
        Self::new(coco_exec_server::LOCAL_FS.clone(), None)
    }
}

impl std::fmt::Debug for ApplyPatchTool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ApplyPatchTool")
            .field("has_sandbox_context", &self.sandbox.is_some())
            .finish_non_exhaustive()
    }
}

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

    fn max_result_size_bound(&self) -> coco_tool_runtime::ResultSizeBound {
        coco_tool_runtime::ResultSizeBound::Bytes(3_000)
    }

    async fn prepare(
        &self,
        input: &ApplyPatchInput,
        ctx: &ToolUseContext,
    ) -> Result<Option<PreparedToolState>, ToolError> {
        let Ok(parsed) = coco_apply_patch::parse_patch(&input.patch) else {
            return Ok(None);
        };
        if parsed.environment_id.is_some() {
            return Err(execution_failed_with_preview(
                "apply_patch environment selection is unavailable for this session",
                build_apply_patch_preview(&input.patch).map(ToolDisplayData::ApplyPatchPreview),
            ));
        }
        let cwd = apply_patch_cwd(ctx)
            .await
            .map_err(ToolError::execution_failed)?;
        let logical_effects = coco_apply_patch::collect_path_effects(&parsed.hunks, &cwd)
            .map_err(|error| ToolError::execution_failed(error.to_string()))?;
        if let Some(message) = first_write_fence_denial(&logical_effects, ctx) {
            return Ok(Some(std::sync::Arc::new(
                PreparedApplyPatch::DeniedByWriteFence { message },
            )));
        }
        let prepared = coco_apply_patch::prepare_hunk_paths(
            &parsed.hunks,
            &cwd,
            coco_apply_patch::ApplyPatchFileUpdateMode::PreserveLineEndings,
            self.fs.clone(),
            self.sandbox.clone(),
        )
        .await
        .map_err(|error| {
            execution_failed_with_preview(
                error.to_string(),
                build_apply_patch_preview(&input.patch).map(ToolDisplayData::ApplyPatchPreview),
            )
        })?;
        Ok(Some(std::sync::Arc::new(PreparedApplyPatch::Paths(
            Box::new(prepared),
        ))))
    }

    async fn check_prepared_permissions(
        &self,
        input: &ApplyPatchInput,
        prepared: Option<&PreparedToolState>,
        ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        let Some(prepared) = prepared_apply_patch_state(prepared) else {
            return self.check_permissions(input, ctx).await;
        };
        match prepared {
            PreparedApplyPatch::DeniedByWriteFence { message } => ToolCheckResult::Deny {
                message: message.clone(),
            },
            PreparedApplyPatch::Paths(paths) => {
                check_canonical_path_permissions(paths.path_effects(), ctx).await
            }
        }
    }

    async fn check_permissions(
        &self,
        input: &ApplyPatchInput,
        ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        let Ok(cwd) = apply_patch_cwd(ctx).await else {
            return ToolCheckResult::Passthrough;
        };
        let Ok(parsed) = coco_apply_patch::parse_patch(&input.patch) else {
            return allow_correctness_check();
        };
        let logical_effects = match coco_apply_patch::collect_path_effects(&parsed.hunks, &cwd) {
            Ok(path_effects) => path_effects,
            Err(_) => return allow_correctness_check(),
        };
        if let Some(message) = first_write_fence_denial(&logical_effects, ctx) {
            return ToolCheckResult::Deny { message };
        }
        let path_effects = match coco_apply_patch::validate_hunk_paths(
            &parsed.hunks,
            &cwd,
            self.fs.as_ref(),
            self.sandbox.as_ref(),
        )
        .await
        {
            Ok(path_effects) => path_effects,
            Err(
                coco_apply_patch::ApplyPatchError::ParseError(_)
                | coco_apply_patch::ApplyPatchError::PathUri(_),
            ) => return allow_correctness_check(),
            Err(_) => return ToolCheckResult::Passthrough,
        };
        check_canonical_path_permissions(&path_effects, ctx).await
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
        let prepared = self.prepare(&input, ctx).await?;
        self.execute_prepared(input, prepared, ctx).await
    }

    async fn execute_prepared(
        &self,
        input: ApplyPatchInput,
        prepared_state: Option<PreparedToolState>,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<ApplyPatchOutput>, ToolError> {
        let patch = &input.patch;
        let preview = build_apply_patch_preview(patch);
        let display_data = preview.clone().map(ToolDisplayData::ApplyPatchPreview);

        let cwd_path = ctx.cwd_anchor().await.ok_or_else(|| {
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
        let cwd = PathUri::from_abs_path(&cwd);

        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let fs = self.fs.as_ref();
        let sandbox = self.sandbox.as_ref();

        let parsed = match coco_apply_patch::parse_patch(patch) {
            Ok(parsed) => Some(parsed),
            Err(_) => {
                coco_apply_patch::apply_patch(patch, &cwd, &mut stdout, &mut stderr, fs, sandbox)
                    .await
                    .map_err(|e| {
                        apply_patch_error_with_preview(&stderr, e, display_data.clone())
                    })?;
                None
            }
        };
        let Some(_) = parsed else {
            return Ok(result_with_preview(stdout, stderr, display_data));
        };
        let prepared_paths = match prepared_apply_patch_state(prepared_state.as_ref()) {
            Some(PreparedApplyPatch::Paths(prepared)) => prepared,
            Some(PreparedApplyPatch::DeniedByWriteFence { message }) => {
                return Err(execution_failed_with_preview(message.clone(), display_data));
            }
            None => {
                return Err(execution_failed_with_preview(
                    "apply_patch execution is missing its prepared plan; retry the tool call",
                    display_data,
                ));
            }
        };
        let path_effects = prepared_paths.path_effects().clone();

        // Execute-time guard: `canUseTool` Allow skips built-in permission
        // checks, so re-enforce the write fence immediately before mutation.
        for path in path_effects.paths() {
            if crate::check_write_root_fence(ctx, &path.to_path_buf()).is_some() {
                return Err(execution_failed_with_preview(
                    canonical_write_fence_denial(),
                    display_data,
                ));
            }
        }

        // Content snapshots and patch-context matching happen only after the
        // canonical path plan has passed permission resolution.
        let prepared = coco_apply_patch::prepare_hunks_from_paths(prepared_paths)
            .await
            .map_err(|error| {
                execution_failed_with_preview(error.to_string(), display_data.clone())
            })?;

        enforce_team_memory_secret_guard(ctx, &prepared)
            .map_err(|message| execution_failed_with_preview(message, display_data.clone()))?;

        // Capture file-history snapshots before mutation, mirroring Edit/Write.
        for path in path_effects.paths() {
            crate::track_file_edit(ctx, &path.to_path_buf()).await;
        }

        let committed = match coco_apply_patch::commit_prepared_patch(&prepared).await {
            Ok(committed) => committed,
            Err(error) => {
                record_applied_delta(ctx, error.delta(), &path_effects).await;
                return Err(apply_patch_error_with_preview(
                    &stderr,
                    error,
                    display_data.clone(),
                ));
            }
        };
        coco_apply_patch::print_summary(committed.affected_paths(), &mut stdout).map_err(
            |error| execution_failed_with_preview(error.to_string(), display_data.clone()),
        )?;

        for (path, contents) in committed.written_files() {
            crate::record_file_edit(ctx, &path.to_path_buf(), contents.to_string()).await;
        }
        for path in committed.deleted_files() {
            crate::record_file_delete(ctx, &path.to_path_buf()).await;
        }

        // Match Write/Edit post-commit integration for every source and
        // destination: activate path-gated skills and refresh diagnostics.
        for path in path_effects.logical_paths() {
            let path = path.to_path_buf();
            crate::track_skill_triggers(ctx, &path).await;
        }
        for path in path_effects.paths() {
            let path = path.to_path_buf();
            ctx.lsp.notify_save(&path).await;
        }

        Ok(result_with_preview(stdout, stderr, display_data))
    }
}

fn enforce_team_memory_secret_guard(
    ctx: &ToolUseContext,
    prepared: &coco_apply_patch::PreparedPatch,
) -> Result<(), String> {
    for (target, contents) in prepared.proposed_writes() {
        if let Some(message) = crate::check_team_mem_secret(ctx, &target.to_path_buf(), contents) {
            return Err(message);
        }
    }
    Ok(())
}

fn prepared_apply_patch_state(state: Option<&PreparedToolState>) -> Option<&PreparedApplyPatch> {
    state?.as_ref().downcast_ref::<PreparedApplyPatch>()
}

fn first_write_fence_denial(
    path_effects: &coco_apply_patch::ApplyPatchPathEffects,
    ctx: &ToolUseContext,
) -> Option<String> {
    path_effects
        .logical_paths()
        .iter()
        .find_map(|path| crate::check_write_root_fence(ctx, &path.to_path_buf()))
}

fn canonical_write_fence_denial() -> &'static str {
    "Refusing to apply patch: a target resolves outside this agent's allowed write roots."
}

async fn check_canonical_path_permissions(
    path_effects: &coco_apply_patch::ApplyPatchPathEffects,
    ctx: &ToolUseContext,
) -> ToolCheckResult {
    if path_effects.paths().is_empty() {
        return ToolCheckResult::Passthrough;
    }
    let Ok(cwd) = apply_patch_cwd(ctx).await else {
        return ToolCheckResult::Passthrough;
    };
    let cwd_path = cwd.to_path_buf();
    let cwd_str = cwd_path.to_string_lossy().to_string();
    let mut all_paths_to_check = Vec::new();
    for path in path_effects.paths() {
        let path = path.to_path_buf();
        if crate::check_write_root_fence(ctx, &path).is_some() {
            return ToolCheckResult::Deny {
                message: canonical_write_fence_denial().to_string(),
            };
        }
        let path_str = path.to_string_lossy();
        all_paths_to_check.extend(
            coco_permissions::filesystem::get_paths_for_permission_check(&path_str, &cwd_str),
        );
    }
    crate::tools::write_permissions::check_write_permission_for_paths(
        &all_paths_to_check,
        ctx,
        ToolName::ApplyPatch.as_str(),
        "apply a patch",
        &cwd_path,
    )
}

async fn record_applied_delta(
    ctx: &ToolUseContext,
    delta: &coco_apply_patch::AppliedPatchDelta,
    path_effects: &coco_apply_patch::ApplyPatchPathEffects,
) {
    for change in delta.changes() {
        match &change.change {
            coco_apply_patch::AppliedPatchFileChange::Add { content, .. } => {
                crate::record_file_edit(ctx, &change.path.to_path_buf(), content.clone()).await;
                ctx.lsp.notify_save(&change.path.to_path_buf()).await;
            }
            coco_apply_patch::AppliedPatchFileChange::Delete { .. } => {
                crate::record_file_delete(ctx, &change.path.to_path_buf()).await;
                ctx.lsp.notify_save(&change.path.to_path_buf()).await;
            }
            coco_apply_patch::AppliedPatchFileChange::Update {
                move_path,
                new_content,
                ..
            } => {
                let target = move_path.as_ref().unwrap_or(&change.path);
                crate::record_file_edit(ctx, &target.to_path_buf(), new_content.clone()).await;
                ctx.lsp.notify_save(&target.to_path_buf()).await;
                if move_path.is_some() {
                    crate::record_file_delete(ctx, &change.path.to_path_buf()).await;
                    ctx.lsp.notify_save(&change.path.to_path_buf()).await;
                }
            }
        }
    }

    if !delta.is_exact() {
        // A checked write can still fail after truncation (for example ENOSPC).
        // Drop cached contents for every authorized target and force LSP refresh.
        for path in path_effects.paths() {
            crate::record_file_delete(ctx, &path.to_path_buf()).await;
            ctx.lsp.notify_save(&path.to_path_buf()).await;
        }
    }
    for path in path_effects.logical_paths() {
        crate::track_skill_triggers(ctx, &path.to_path_buf()).await;
    }
}

fn allow_correctness_check() -> ToolCheckResult {
    ToolCheckResult::Allow {
        updated_input: None,
        feedback: None,
    }
}

fn apply_patch_error_with_preview(
    stderr: &[u8],
    error: impl std::fmt::Display,
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

async fn apply_patch_cwd(ctx: &ToolUseContext) -> Result<PathUri, String> {
    let cwd_path = ctx
        .cwd_anchor()
        .await
        .ok_or_else(|| "no working directory available for apply_patch".to_string())?;
    AbsolutePathBuf::from_absolute_path(&cwd_path)
        .map(|cwd| PathUri::from_abs_path(&cwd))
        .map_err(|e| format!("cwd `{}` is not absolute: {e}", cwd_path.display()))
}

fn preview_rows_to_dto(rows: usize) -> i64 {
    i64::try_from(rows).unwrap_or(i64::MAX)
}

#[cfg(test)]
#[path = "apply_patch.test.rs"]
mod tests;
