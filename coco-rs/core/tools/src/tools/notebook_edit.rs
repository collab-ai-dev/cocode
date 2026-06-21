//! NotebookEditTool — Jupyter `.ipynb` cell editing.
//!
//! Wire shape:
//!   - notebook_path  : string (required, absolute .ipynb path)
//!   - cell_id        : string (required, cell UUID or "cell-N" index)
//!   - new_source     : string (content for replace/insert)
//!   - cell_type      : enum { code, markdown } (required for insert)
//!     **raw is NOT supported**
//!   - edit_mode      : enum { replace, insert, delete }
//!
//! Cell ID generation uses a 13-char alphanumeric base-36 string and
//! only applies when the notebook's nbformat is ≥ 4.5.
//!
//! File-history hook: `track_file_edit` runs before `tokio::fs::write`
//! so a pre-edit backup is captured for the rewind subsystem.

use async_trait::async_trait;
use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Short per-call UI label.
const NOTEBOOK_EDIT_SHORT_DESCRIPTION: &str =
    "Replace the contents of a specific cell in a Jupyter notebook.";

/// Long-form model-facing description.
const NOTEBOOK_EDIT_PROMPT: &str = "Completely replaces the contents of a specific cell in a Jupyter notebook (.ipynb file) with new source. Jupyter notebooks are interactive documents that combine code, text, and visualizations, commonly used for data analysis and scientific computing. The notebook_path parameter must be an absolute path, not a relative path. The cell_id identifies the target cell — either a cell ID/UUID, or \"cell-N\" for the N-th cell (0-indexed). Use edit_mode=insert to add a new cell at the position of cell_id (or at the top when cell_id is omitted). Use edit_mode=delete to delete the cell identified by cell_id.";

/// Cell type for the `insert` mode. `raw` is not supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotebookCellType {
    #[default]
    Code,
    Markdown,
}

impl NotebookCellType {
    fn as_str(self) -> &'static str {
        match self {
            NotebookCellType::Code => "code",
            NotebookCellType::Markdown => "markdown",
        }
    }
}

/// Edit operation to perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotebookEditMode {
    /// Replace the cell at `cell_id` with `new_source`.
    #[default]
    Replace,
    /// Insert a new cell at `cell_id`'s position (or position 0 when
    /// `cell_id` is empty).
    Insert,
    /// Delete the cell at `cell_id`.
    Delete,
}

impl NotebookEditMode {
    /// Lowercase wire name (matches the `snake_case` serde rename).
    fn as_str(self) -> &'static str {
        match self {
            NotebookEditMode::Replace => "replace",
            NotebookEditMode::Insert => "insert",
            NotebookEditMode::Delete => "delete",
        }
    }
}

/// Typed input for [`NotebookEditTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct NotebookEditInput {
    /// The absolute path to the Jupyter notebook file to edit (must be absolute, not relative)
    pub notebook_path: String,
    /// The ID of the cell to edit. When inserting a new cell, the new cell will be inserted after the cell with this ID, or at the beginning if not specified.
    #[serde(default)]
    pub cell_id: String,
    /// The new source for the cell
    pub new_source: String,
    /// The type of the cell (code or markdown). If not specified, it defaults to the current cell type. If using edit_mode=insert, this is required.
    #[serde(default)]
    pub cell_type: Option<NotebookCellType>,
    /// The type of edit to make (replace, insert, delete). Defaults to replace.
    #[serde(default)]
    pub edit_mode: NotebookEditMode,
}

/// Typed output for [`NotebookEditTool`]. Tagged union keyed by
/// `edit_mode` so each variant carries only the id field it owns:
/// `replace`/`delete` resolve the existing `cell_id`, `insert` emits a
/// fresh `new_cell_id` (or `null` when nbformat < 4.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "edit_mode", rename_all = "snake_case")]
pub enum NotebookEditOutput {
    Replace {
        #[serde(default)]
        message: String,
        #[serde(default)]
        notebook_path: String,
        #[serde(default)]
        cell_index: usize,
        /// Resolved cell id from the underlying notebook. `None` when
        /// the notebook uses nbformat < 4.5 (no per-cell ids).
        #[serde(default)]
        cell_id: Option<String>,
    },
    Insert {
        #[serde(default)]
        message: String,
        #[serde(default)]
        notebook_path: String,
        #[serde(default)]
        cell_index: usize,
        /// Freshly-generated cell id (nbformat ≥ 4.5) or `null` for
        /// older notebooks. Serialized as JSON `null` rather than omitted.
        new_cell_id: Option<String>,
    },
    Delete {
        #[serde(default)]
        message: String,
        #[serde(default)]
        notebook_path: String,
        #[serde(default)]
        cell_index: usize,
        #[serde(default)]
        cell_id: Option<String>,
    },
}

impl NotebookEditOutput {
    fn message(&self) -> &str {
        match self {
            NotebookEditOutput::Replace { message, .. }
            | NotebookEditOutput::Insert { message, .. }
            | NotebookEditOutput::Delete { message, .. } => message,
        }
    }
}

pub struct NotebookEditTool;

#[async_trait]
impl Tool for NotebookEditTool {
    type Input = NotebookEditInput;
    coco_tool_runtime::impl_runtime_schema!(NotebookEditInput);
    type Output = NotebookEditOutput;

    fn to_auto_classifier_input(&self, input: &NotebookEditInput) -> Option<String> {
        // `edit_mode` (replace / insert / delete) is security-relevant, so it
        // rides along with the path and payload.
        Some(format!(
            "{} {}: {}",
            input.notebook_path,
            input.edit_mode.as_str(),
            input.new_source
        ))
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::NotebookEdit)
    }
    fn name(&self) -> &str {
        ToolName::NotebookEdit.as_str()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(coco_types::Feature::NotebookEdit)
    }
    fn description(&self, _input: &NotebookEditInput, _options: &DescriptionOptions) -> String {
        NOTEBOOK_EDIT_SHORT_DESCRIPTION.into()
    }

    /// Model-facing tool description (schema-listing time).
    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        NOTEBOOK_EDIT_PROMPT.into()
    }

    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("edit Jupyter notebook cells (.ipynb)")
    }

    /// Reject non-`.ipynb` paths (errorCode 2 → redirect to FileEdit)
    /// and require a `cell_type` for insert (errorCode 5). UNC paths
    /// (`\\…` / `//…`) bypass the extension check.
    fn validate_input(
        &self,
        input: &NotebookEditInput,
        _ctx: &ToolUseContext,
    ) -> coco_tool_runtime::ValidationResult {
        use coco_tool_runtime::ValidationResult;
        if input.notebook_path.is_empty() {
            return ValidationResult::invalid("notebook_path parameter is required");
        }
        let is_unc =
            input.notebook_path.starts_with("\\\\") || input.notebook_path.starts_with("//");
        if !is_unc && !input.notebook_path.to_lowercase().ends_with(".ipynb") {
            return ValidationResult::invalid_with_code(
                "File must be a Jupyter notebook (.ipynb file). For editing other file types, use the FileEdit tool.",
                "2",
            );
        }
        if matches!(input.edit_mode, NotebookEditMode::Insert) && input.cell_type.is_none() {
            return ValidationResult::invalid_with_code(
                "Cell type is required when using edit_mode=insert.",
                "5",
            );
        }
        ValidationResult::Valid
    }

    async fn check_permissions(
        &self,
        input: &NotebookEditInput,
        ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        if input.notebook_path.is_empty() {
            return ToolCheckResult::Passthrough;
        }
        crate::tools::write_permissions::check_write_permission_for_path(
            &input.notebook_path,
            ctx,
            ToolName::NotebookEdit.as_str(),
            "edit a notebook",
        )
    }

    /// Render the edit envelope as the prebuilt `message` field so the
    /// model gets the human-readable summary directly. notebook_path /
    /// cell_index / cell_id are TUI/state concerns.
    fn render_for_model(&self, out: &NotebookEditOutput) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.message().to_string(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: NotebookEditInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<NotebookEditOutput>, ToolError> {
        let notebook_path = input.notebook_path.as_str();

        if notebook_path.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "notebook_path parameter is required".into(),
                error_code: None,
            });
        }

        let edit_mode = input.edit_mode;
        let cell_id = input.cell_id.as_str();

        // Enforce read-before-edit: without this guard the model can edit a
        // notebook it never saw (or
        // edit against a stale view after an external change), silently
        // clobbering data. Mirrors the same check in `FileEditTool` /
        // `FileWriteTool`. The check runs only when `file_read_state` is
        // populated — tests without a context still work.
        if let Some(frs) = &ctx.file_read_state
            && let Ok(abs_path) = std::fs::canonicalize(notebook_path)
        {
            let frs_read = frs.read().await;
            if frs_read.peek(&abs_path).is_none() {
                return Err(ToolError::ExecutionFailed {
                    message: format!(
                        "{notebook_path} has not been read yet. Read it first before editing it."
                    ),
                    display_data: None,
                    source: None,
                });
            }
            // mtime drift check — reject edits staged against a view that is
            // older than the current
            // disk mtime so we don't quietly overwrite external changes.
            if let Some(entry) = frs_read.peek(&abs_path) {
                if !entry.can_satisfy_edit_or_write() {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "{notebook_path} was only provided as partial injected context. \
                             Read it with the Read tool before editing it."
                        ),
                        display_data: None,
                        source: None,
                    });
                }

                if let Ok(disk_mtime) = coco_context::file_mtime_ms(&abs_path).await
                    && disk_mtime > entry.mtime_ms
                {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "{notebook_path} has been modified since it was last read. \
                             Read it again before editing."
                        ),
                        display_data: None,
                        source: None,
                    });
                }
            }
        }

        // Read the notebook file
        let content = tokio::fs::read_to_string(notebook_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to read notebook '{notebook_path}': {e}"),
                display_data: None,
                source: None,
            })?;

        let mut notebook: Value =
            serde_json::from_str(&content).map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to parse notebook JSON: {e}"),
                display_data: None,
                source: None,
            })?;

        // Read nbformat before mutating
        let nbformat = notebook
            .get("nbformat")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(4);
        let nbformat_minor = notebook
            .get("nbformat_minor")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);

        let cells = notebook
            .get_mut("cells")
            .and_then(|v| v.as_array_mut())
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "Notebook does not contain a 'cells' array".into(),
                display_data: None,
                source: None,
            })?;

        // Resolve cell index from cell_id. For insert with an empty
        // cell_id we default to position 0 so the model can create the
        // first cell without having to pass "0" explicitly. For insert
        // with a referenced cell, places the new cell AFTER it
        // (cellIndex += 1), so the new cell follows the one named — not
        // before it.
        let cell_index = if matches!(edit_mode, NotebookEditMode::Insert) && cell_id.is_empty() {
            0
        } else {
            let idx = resolve_cell_index(cells, cell_id)?;
            if matches!(edit_mode, NotebookEditMode::Insert) {
                idx + 1
            } else {
                idx
            }
        };

        // Replacing one-past-the-end is treated as an append-insert.
        // `cell_type` then defaults to code (the `validate_input` gate
        // only requires it for an *explicit* insert).
        let effective_mode =
            if matches!(edit_mode, NotebookEditMode::Replace) && cell_index == cells.len() {
                NotebookEditMode::Insert
            } else {
                edit_mode
            };

        // R5-T15: return the actual cell ID (string) rather than a bare
        // index. `new_cell_id` for insert and `cell_id` for
        // replace/delete so the model can reference cells by the ID it
        // wrote. `cell_index` is still returned for debuggability.
        let mut resolved_cell_id: Option<String> = None;
        let mut new_cell_id: Option<String> = None;

        // The id surfaced to the model is `cell_id` from input for replace/delete
        // and a freshly-generated 13-char base-36 string for insert — but
        // only when nbformat ≥ 4.5. Older notebooks have no cell-id
        // field, so renders the literal string `undefined`.
        let supports_ids = nbformat > 4 || (nbformat == 4 && nbformat_minor >= 5);
        let displayed_cell_id = |id: Option<&str>| -> String {
            match id {
                Some(s) if supports_ids && !s.is_empty() => s.to_string(),
                _ => "undefined".to_string(),
            }
        };

        let result_msg = match effective_mode {
            NotebookEditMode::Replace => {
                let new_source = input.new_source.as_str();

                if cell_index >= cells.len() {
                    return Err(ToolError::InvalidInput {
                        message: format!(
                            "cell index {cell_index} out of range (notebook has {} cells)",
                            cells.len()
                        ),
                        error_code: None,
                    });
                }

                // Capture the resolved cell's id BEFORE mutating the
                // source — the id field is not touched by replace, so
                // reading it here preserves the expected semantics.
                resolved_cell_id = cells[cell_index]
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                // Only code cells reset execution state. Read the *current*
                // type before any cell_type switch.
                let was_code = cells[cell_index].get("cell_type").and_then(|v| v.as_str())
                    == Some(NotebookCellType::Code.as_str());

                cells[cell_index]["source"] = Value::Array(source_to_lines(new_source));
                if was_code {
                    cells[cell_index]["execution_count"] = Value::Null;
                    cells[cell_index]["outputs"] = Value::Array(vec![]);
                }

                // Apply a cell_type switch on replace when the input differs
                // from the cell.
                if let Some(ct) = input.cell_type
                    && cells[cell_index].get("cell_type").and_then(|v| v.as_str())
                        != Some(ct.as_str())
                {
                    cells[cell_index]["cell_type"] = Value::String(ct.as_str().to_string());
                }

                let id = displayed_cell_id(Some(cell_id));
                format!("Updated cell {id} with {new_source}")
            }
            NotebookEditMode::Insert => {
                let new_source = input.new_source.as_str();
                let cell_type = input.cell_type.unwrap_or(NotebookCellType::Code);
                let cell_type_str = cell_type.as_str();

                let mut new_cell = serde_json::json!({
                    "cell_type": cell_type_str,
                    "source": source_to_lines(new_source),
                    "metadata": {},
                });

                if matches!(cell_type, NotebookCellType::Code) {
                    new_cell["execution_count"] = Value::Null;
                    new_cell["outputs"] = Value::Array(vec![]);
                }

                // Cell ID generation — nbformat ≥ 4.5 only.
                // Generates a 13-char base-36 alphanumeric using
                // rand::thread_rng.
                if supports_ids {
                    let generated = generate_cell_id();
                    new_cell["id"] = Value::String(generated.clone());
                    new_cell_id = Some(generated);
                }

                let insert_at = cell_index.min(cells.len());
                cells.insert(insert_at, new_cell);

                let id = displayed_cell_id(new_cell_id.as_deref());
                format!("Inserted cell {id} with {new_source}")
            }
            NotebookEditMode::Delete => {
                if cell_index >= cells.len() {
                    return Err(ToolError::InvalidInput {
                        message: format!(
                            "cell index {cell_index} out of range (notebook has {} cells)",
                            cells.len()
                        ),
                        error_code: None,
                    });
                }
                // Capture the cell's id before removing it.
                resolved_cell_id = cells[cell_index]
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                cells.remove(cell_index);
                let id = displayed_cell_id(Some(cell_id));
                format!("Deleted cell {id}")
            }
        };

        // Write back
        let updated =
            serde_json::to_string_pretty(&notebook).map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to serialize notebook: {e}"),
                display_data: None,
                source: None,
            })?;

        // Sandboxed write fence (memory-extraction / auto-dream
        // subagents). No-op when `ctx.allowed_write_roots` is empty.
        if let Some(err) = crate::check_write_root_fence(ctx, std::path::Path::new(notebook_path)) {
            return Err(ToolError::ExecutionFailed {
                message: err,
                display_data: None,
                source: None,
            });
        }

        // Capture pre-edit content for file history before mutating.
        // Mirrors Edit/Write/apply_patch ordering.
        crate::track_file_edit(ctx, std::path::Path::new(notebook_path)).await;

        tokio::fs::write(notebook_path, &updated)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to write notebook '{notebook_path}': {e}"),
                display_data: None,
                source: None,
            })?;

        // Notify the LSP server of the save so diagnostics refresh. Best-effort.
        ctx.lsp
            .notify_save(std::path::Path::new(notebook_path))
            .await;

        // Build the response. For insert: include `new_cell_id`
        // (or null when nbformat < 4.5). For replace/delete: include
        // `cell_id` from the resolved cell. Always include `cell_index`
        // for debuggability.
        let notebook_path_string = notebook_path.to_string();
        let data = match effective_mode {
            NotebookEditMode::Replace => NotebookEditOutput::Replace {
                message: result_msg,
                notebook_path: notebook_path_string,
                cell_index,
                cell_id: resolved_cell_id,
            },
            NotebookEditMode::Insert => NotebookEditOutput::Insert {
                message: result_msg,
                notebook_path: notebook_path_string,
                cell_index,
                // `new_cell_id` is emitted even when null (nbformat < 4.5).
                new_cell_id,
            },
            NotebookEditMode::Delete => NotebookEditOutput::Delete {
                message: result_msg,
                notebook_path: notebook_path_string,
                cell_index,
                cell_id: resolved_cell_id,
            },
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

/// Resolve a cell identifier to an index.
/// Supports: numeric string, "cell-N" format, or cell ID matching.
fn resolve_cell_index(cells: &[Value], cell_id: &str) -> Result<usize, ToolError> {
    if cell_id.is_empty() {
        return Err(ToolError::InvalidInput {
            message: "cell_id parameter is required".into(),
            error_code: None,
        });
    }

    // Try "cell-N" format
    if let Some(n) = cell_id.strip_prefix("cell-")
        && let Ok(idx) = n.parse::<usize>()
    {
        return Ok(idx);
    }

    // Try direct numeric
    if let Ok(idx) = cell_id.parse::<usize>() {
        return Ok(idx);
    }

    // Try matching cell ID field
    for (i, cell) in cells.iter().enumerate() {
        if cell.get("id").and_then(|v| v.as_str()) == Some(cell_id) {
            return Ok(i);
        }
    }

    Err(ToolError::InvalidInput {
        message: format!("Could not find cell with ID '{cell_id}'"),
        error_code: None,
    })
}

/// Convert source text to notebook line array format.
fn source_to_lines(source: &str) -> Vec<Value> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return vec![Value::String(String::new())];
    }
    lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            if i < lines.len() - 1 {
                Value::String(format!("{line}\n"))
            } else {
                Value::String((*line).to_string())
            }
        })
        .collect()
}

/// Generate a Jupyter cell ID.
///
/// Generates a 13-char lowercase alphanumeric (base-36) string,
/// matching the format produced by
/// `Math.random().toString(36).substring(2, 15)`.
pub(crate) fn generate_cell_id() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::rng();
    (0..13)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

#[cfg(test)]
#[path = "notebook_edit.test.rs"]
mod tests;
