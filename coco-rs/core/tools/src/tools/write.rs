use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::path::Path;

/// Long-form tool description shown to the model.
const WRITE_TOOL_DESCRIPTION: &str = "Writes a file to the local filesystem.

Usage:
- This tool will overwrite the existing file if there is one at the provided path.
- If this is an existing file, you MUST use the `Read` tool first to read the file's contents. This tool will fail if you did not read the file first.
- Prefer the Edit tool for modifying existing files — it only sends the diff. Only use this tool to create new files or for complete rewrites.
- NEVER create documentation files (*.md) or README files unless explicitly requested by the User.
- Only use emojis if the user explicitly requests it. Avoid writing emojis to files unless asked.";

/// Typed input for [`WriteTool`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct WriteInput {
    /// The absolute path to the file to write (must be absolute, not relative)
    pub file_path: String,
    /// The content to write to the file
    pub content: String,
}

/// Typed output for [`WriteTool`] — tagged enum keyed by the operation
/// performed. `filePath` is camelCase on the wire
/// (`mapToolResultToToolResultBlockParam`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WriteOutput {
    /// New file created. `filePath` is the resolved absolute path.
    Create {
        #[serde(rename = "filePath")]
        file_path: String,
    },
    /// Existing file overwritten.
    Update {
        #[serde(rename = "filePath")]
        file_path: String,
    },
}

impl WriteOutput {
    fn file_path(&self) -> &str {
        match self {
            WriteOutput::Create { file_path } | WriteOutput::Update { file_path } => file_path,
        }
    }
}

/// Write tool — creates or overwrites a file.
/// Creates parent directories as needed.
pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
    type Input = WriteInput;
    coco_tool_runtime::impl_runtime_schema!(WriteInput);
    type Output = WriteOutput;

    fn to_auto_classifier_input(&self, input: &WriteInput) -> Option<String> {
        Some(format!("{}: {}", input.file_path, input.content))
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Write)
    }

    fn name(&self) -> &str {
        ToolName::Write.as_str()
    }

    /// Short per-call UI label: `'Write a file to the local filesystem.'`.
    fn search_hint(&self) -> Option<&str> {
        Some("create or overwrite files")
    }
    fn description(&self, _input: &WriteInput, _options: &DescriptionOptions) -> String {
        "Write a file to the local filesystem.".into()
    }

    /// Model-facing tool description (schema-listing time). Text held in
    /// [`WRITE_TOOL_DESCRIPTION`].
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        WRITE_TOOL_DESCRIPTION.into()
    }

    fn is_destructive(&self, _input: &WriteInput) -> bool {
        true
    }

    fn get_activity_description(&self, input: &WriteInput) -> Option<String> {
        Some(format!("Writing {path}", path = input.file_path))
    }

    fn get_path(&self, input: &WriteInput) -> Option<String> {
        Some(input.file_path.clone())
    }

    fn validate_input(&self, input: &WriteInput, _ctx: &ToolUseContext) -> ValidationResult {
        if input.file_path.is_empty() {
            return ValidationResult::invalid("missing required field: file_path");
        }
        // Note: `content: String` deserialized successfully, so it's
        // present; empty content is a legitimate "truncate" use case.
        let _ = &input.content;
        ValidationResult::Valid
    }

    async fn check_permissions(&self, input: &WriteInput, ctx: &ToolUseContext) -> ToolCheckResult {
        crate::tools::write_permissions::check_write_permission_for_path(
            &input.file_path,
            ctx,
            ToolName::Write.as_str(),
            "write to a file",
        )
    }

    /// Branch on the tagged enum to emit the confirmation message.
    fn render_for_model(&self, out: &WriteOutput) -> Vec<ToolResultContentPart> {
        let file_path = out.file_path();
        let text = match out {
            WriteOutput::Create { .. } => format!("File created successfully at: {file_path}"),
            WriteOutput::Update { .. } => {
                format!("The file {file_path} has been updated successfully.")
            }
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: WriteInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<WriteOutput>, ToolError> {
        let file_path = input.file_path.as_str();
        let content = input.content.as_str();

        let path = Path::new(file_path);

        // Sandbox pre-flight — deny inadmissible writes before any I/O,
        // so SDK consumers can intercept via the approval bridge.
        super::sandbox_preflight::preflight_path(ctx, path, /*write=*/ true).await?;

        let is_new = !path.exists();

        // Read-before-write enforcement + race detection.
        //
        // When overwriting an existing file, the model MUST have read it first
        // in this session. This prevents the "model hallucinates a replacement
        // for a file it never saw" class of bugs. The check has three layers:
        //
        //   1. The file must exist in `readFileState` (Read was called).
        //   2. The stored mtime must still match the on-disk mtime (no one
        //      edited the file since our read).
        //   3. If mtime matches but the stored content differs from the
        //      current disk content (e.g. mtime 1s precision lied), reject.
        //
        // New files bypass all of this — there's nothing to race with.
        if !is_new
            && let Some(frs) = &ctx.file_read_state
            && let Ok(abs_path) = std::fs::canonicalize(path)
        {
            let frs_read = frs.read().await;
            let Some(entry) = frs_read.peek(&abs_path) else {
                return Err(ToolError::ExecutionFailed {
                    message: format!(
                        "{file_path} must be read with the Read tool before it can be \
                         overwritten. This prevents accidental data loss from unseen files."
                    ),
                    display_data: None,
                    source: None,
                });
            };

            if !entry.can_satisfy_edit_or_write() {
                return Err(ToolError::ExecutionFailed {
                    message: format!(
                        "{file_path} was only provided as partial injected context. \
                         Read it with the Read tool before overwriting."
                    ),
                    display_data: None,
                    source: None,
                });
            }

            // Layer 2: mtime comparison.
            if let Ok(disk_mtime) = coco_context::file_mtime_ms(&abs_path).await
                && disk_mtime > entry.mtime_ms
            {
                return Err(ToolError::ExecutionFailed {
                    message: format!(
                        "{file_path} has been modified since it was last read \
                         (mtime changed). Read it again before overwriting."
                    ),
                    display_data: None,
                    source: None,
                });
            }

            // Layer 3: content hash fallback. mtime can be stable even when
            // content changed on filesystems with <1s precision or under
            // rapid edit loops, so we compare the stored content against
            // the current disk content when the stored entry is a full real
            // snapshot. For line-range reads we can't
            // compare meaningfully, so we skip this layer.
            if entry.is_full_real()
                && let Ok(raw) = std::fs::read(&abs_path)
            {
                let detected_enc = coco_file_encoding::detect_encoding(&raw);
                if let Ok(current) = detected_enc.decode(&raw)
                    && current != entry.content
                {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "{file_path} has been modified since it was last read \
                             (content changed). Read it again before overwriting."
                        ),
                        display_data: None,
                        source: None,
                    });
                }
            }
        }

        // Sandboxed write fence (memory-extraction / auto-dream
        // subagents). No-op when `ctx.allowed_write_roots` is empty.
        if let Some(err) = crate::check_write_root_fence(ctx, path) {
            return Err(ToolError::ExecutionFailed {
                message: err,
                display_data: None,
                source: None,
            });
        }

        // Team-memory secret guard: refuse to write content containing API
        // keys / tokens / credentials into the team memory directory, because
        // team memory is synced to all repo collaborators. Helper is a no-op
        // for paths outside the team-memory pattern.
        if let Some(err) = crate::check_team_mem_secret(ctx, path, content) {
            return Err(ToolError::ExecutionFailed {
                message: err,
                display_data: None,
                source: None,
            });
        }

        // Track file edit for checkpoint/rewind before modifying.
        crate::track_file_edit(ctx, path).await;

        // Detect encoding + read existing content for diff + preservation.
        // Always write 'LF' even when the original file used CRLF: Write is
        // a full content replacement — the model sent explicit line endings
        // in `content` and meant them. Do not rewrite them. Only encoding is
        // preserved, not line endings. For existing files, sniff the original
        // encoding so we can preserve it on overwrite.
        let detected_encoding: coco_file_encoding::Encoding = if !is_new {
            std::fs::read(file_path)
                .map(|raw| coco_file_encoding::detect_encoding(&raw))
                .unwrap_or(coco_file_encoding::Encoding::Utf8)
        } else {
            coco_file_encoding::Encoding::Utf8
        };

        // Ensure parent directory exists
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent).map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to create directory {}: {e}", parent.display()),
                display_data: None,
                source: None,
            })?;
        }

        // Encode the content using the detected encoding (UTF-8 default for
        // new files). `write_with_format` handles BOM prepending and line-
        // ending normalization. Always pass `LineEnding::Lf` — see the
        // comment above about the line-ending design decision.
        coco_file_encoding::write_with_format(
            path,
            content,
            detected_encoding,
            coco_file_encoding::LineEnding::Lf,
        )
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to write {file_path}: {e}"),
            display_data: None,
            source: None,
        })?;

        crate::record_file_edit(ctx, path, content.to_string()).await;
        // Skill auto-discovery + conditional-skill activation — when a write
        // touches a path under a nested `.coco/skills/` ancestor or matches
        // a `paths`-gated skill, the next batch boundary picks both up.
        crate::track_skill_triggers(ctx, path).await;
        // Clear delivered diagnostics + notify the language server to
        // re-index and emit fresh diagnostics after every successful write.
        // `notify_save` is best-effort — no LSP server / no language
        // binding / RPC failure all become silent no-ops.
        ctx.lsp.notify_save(path).await;

        // Return structured tagged envelope so render_for_model can branch on operation type.
        let data = if is_new {
            WriteOutput::Create {
                file_path: file_path.to_string(),
            }
        } else {
            WriteOutput::Update {
                file_path: file_path.to_string(),
            }
        };
        Ok(ToolResult {
            data,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

#[cfg(test)]
#[path = "write.test.rs"]
mod tests;
