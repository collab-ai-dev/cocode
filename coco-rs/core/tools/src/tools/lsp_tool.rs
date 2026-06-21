//! `LSPTool` — code-intelligence queries (definitions, references, hover,
//! symbols, implementations, call hierarchy).
//!
//! Gated behind `Feature::Lsp` AND `ctx.lsp.is_connected()` — when no
//! language server is configured for the current workspace, the tool is
//! hidden from the model's tool list entirely.
//!
//! Architecture:
//!   - **DTOs + formatters**: [`crate::tools::lsp`] (shared with
//!     other crates and snapshot tests).
//!   - **Wire dispatch**: `ctx.lsp` (an `LspHandleRef`) — the handle
//!     opens the file, routes by extension, and forwards the raw
//!     JSON-RPC request. Implementation lives in
//!     `app/cli/src/lsp_handle_adapter.rs`.

use std::path::Path;
use std::path::PathBuf;

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolUseContext;
use coco_types::Feature;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use crate::input_types::LspAction;
use crate::tools::lsp::CallHierarchyItem;
use crate::tools::lsp::DocumentSymbol;
use crate::tools::lsp::HoverResult;
use crate::tools::lsp::IncomingCall;
use crate::tools::lsp::LspLocation;
use crate::tools::lsp::LspOutput;
use crate::tools::lsp::OutgoingCall;
use crate::tools::lsp::SymbolInformation;
use crate::tools::lsp::count_unique_files;
use crate::tools::lsp::format_call_hierarchy;
use crate::tools::lsp::format_definition_result;
use crate::tools::lsp::format_document_symbols;
use crate::tools::lsp::format_hover_result;
use crate::tools::lsp::format_incoming_calls;
use crate::tools::lsp::format_outgoing_calls;
use crate::tools::lsp::format_references_result;
use crate::tools::lsp::format_workspace_symbols;
use crate::tools::lsp::path_to_file_uri;
use crate::tools::lsp::validate_lsp_file;

/// Model-facing tool description AND prompt, returned from both
/// `description()` and `prompt()`. Multi-line enumeration is what the model
/// expects — single-line summaries hurt operation-name retrieval on small
/// models.
const LSP_DESCRIPTION: &str = "Interact with Language Server Protocol (LSP) servers to get code intelligence features.

Supported operations:
- goToDefinition: Find where a symbol is defined
- findReferences: Find all references to a symbol
- hover: Get hover information (documentation, type info) for a symbol
- documentSymbol: Get all symbols (functions, classes, variables) in a document
- workspaceSymbol: Search for symbols across the entire workspace
- goToImplementation: Find implementations of an interface or abstract method
- prepareCallHierarchy: Get call hierarchy item at a position (functions/methods)
- incomingCalls: Find all functions/methods that call the function at a position
- outgoingCalls: Find all functions/methods called by the function at a position

All operations require:
- filePath: The file to operate on
- line: The line number (1-based, as shown in editors)
- character: The character offset (1-based, as shown in editors)

Note: LSP servers must be configured for the file type. If no server is available, an error will be returned.";

/// Typed tool input matching the LSP tool-facing `inputSchema`.
///
/// `line` / `character` are 1-based (user-facing) and REQUIRED for every
/// operation. They are converted to LSP's 0-based positions in
/// [`build_params`], which ignores them for the file-scoped
/// (`DocumentSymbol`) and workspace-scoped (`WorkspaceSymbol`) operations.
///
/// `filePath` is camelCase on the wire.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct LspInput {
    /// The LSP operation to perform
    pub operation: LspAction,
    /// The absolute or relative path to the file
    #[serde(rename = "filePath")]
    pub file_path: String,
    /// The line number (1-based, as shown in editors)
    #[schemars(range(min = 1))]
    pub line: i32,
    /// The character offset (1-based, as shown in editors)
    #[schemars(range(min = 1))]
    pub character: i32,
}

pub struct LspTool;

#[async_trait::async_trait]
impl Tool for LspTool {
    type Input = LspInput;
    coco_tool_runtime::impl_runtime_schema!(LspInput);
    type Output = crate::tools::lsp::LspOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Lsp)
    }

    fn name(&self) -> &str {
        ToolName::Lsp.as_str()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::Lsp) && ctx.lsp.is_connected()
    }

    fn description(&self, _input: &LspInput, _options: &DescriptionOptions) -> String {
        LSP_DESCRIPTION.into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        LSP_DESCRIPTION.to_string()
    }

    fn is_read_only(&self, _input: &LspInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }

    fn is_lsp(&self) -> bool {
        true
    }

    /// LSP queries are side-effect-free and safe to issue in parallel
    /// — the language server itself handles concurrent requests.
    fn is_concurrency_safe(&self, _input: &LspInput) -> bool {
        true
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("code intelligence (definitions, references, symbols, hover)")
    }

    async fn execute(
        &self,
        input: LspInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<LspOutput>, ToolError> {
        // `line` / `character` are schema-required for every operation;
        // `build_params` ignores them for the file- and workspace-scoped
        // operations that don't carry a position.
        //
        // Worktree-isolated subagents pass `cwd_override` so a
        // relative `filePath` resolves against the worktree, not the
        // process cwd. Falls back to `env::current_dir()` for normal
        // sessions. This is the single source of truth for both
        // path resolution and the gitignore-filter anchor.
        let cwd_buf: PathBuf = ctx
            .cwd_override
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));

        let raw_path = PathBuf::from(&input.file_path);
        let resolved_path = if raw_path.is_absolute() {
            raw_path
        } else {
            cwd_buf.join(&raw_path)
        };
        // The size gate is sourced from `LspConfig::max_file_size_bytes`
        // (settings.json `lsp.max_file_size_bytes` or
        // `COCO_LSP_MAX_FILE_SIZE_BYTES` env override). `i64 → u64`
        // narrowing is safe — the config resolver floors the value to
        // `0` (the "size gate disabled" sentinel) before it reaches here.
        let max_bytes = ctx.lsp_config.max_file_size_bytes.max(0) as u64;
        if let Err(message) = validate_lsp_file(&resolved_path, max_bytes) {
            return Err(ToolError::InvalidInput {
                message,
                error_code: None,
            });
        }
        // Canonicalize so `path_to_file_uri` + `LspServerManager.find_project_root`
        // see the real on-disk path (worktree symlink targets, etc.).
        let path = std::fs::canonicalize(&resolved_path).unwrap_or(resolved_path);

        let uri = path_to_file_uri(&path).ok_or_else(|| ToolError::InvalidInput {
            message: format!("could not build file:// URI for {}", path.display()),
            error_code: None,
        })?;
        let params = build_params(input.operation, &uri, input.line, input.character);

        let raw = dispatch(ctx, input.operation, &path, params).await?;

        let cwd = cwd_buf.to_str();
        let output = format_output(input.operation, &raw, &input.file_path, cwd)?;

        Ok(ToolResult {
            data: output,
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data: None,
        })
    }
}

/// Build the initial LSP request params for an operation. Positions are
/// converted 1-based → 0-based.
fn build_params(op: LspAction, uri: &str, line: i32, character: i32) -> Value {
    if matches!(op, LspAction::WorkspaceSymbol) {
        return json!({ "query": "" });
    }
    if matches!(op, LspAction::DocumentSymbol) {
        return json!({ "textDocument": { "uri": uri } });
    }
    let position = json!({
        "line": line - 1,
        "character": character - 1,
    });
    if matches!(op, LspAction::FindReferences) {
        return json!({
            "textDocument": { "uri": uri },
            "position": position,
            "context": { "includeDeclaration": true },
        });
    }
    json!({
        "textDocument": { "uri": uri },
        "position": position,
    })
}

/// Dispatch the request through `ctx.lsp`. For `IncomingCalls` /
/// `OutgoingCalls` we run a two-step pattern:
/// `prepareCallHierarchy` → pick the first item → second request.
async fn dispatch(
    ctx: &ToolUseContext,
    op: LspAction,
    path: &Path,
    params: Value,
) -> Result<Value, ToolError> {
    let raw = ctx
        .lsp
        .send_request(path, op.lsp_method(), params)
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("LSP request failed: {e}"),
            display_data: None,
            source: None,
        })?;

    let second_method = match op {
        LspAction::IncomingCalls => Some("callHierarchy/incomingCalls"),
        LspAction::OutgoingCalls => Some("callHierarchy/outgoingCalls"),
        _ => None,
    };

    let Some(method) = second_method else {
        return Ok(raw);
    };

    // Two-step call hierarchy: pick the first prepared item, then issue
    // the actual incomingCalls / outgoingCalls request.
    let items: Vec<CallHierarchyItem> = parse_or_default(&raw);
    let Some(first) = items.into_iter().next() else {
        return Ok(Value::Array(vec![]));
    };

    let item_value = serde_json::to_value(&first).map_err(|e| ToolError::ExecutionFailed {
        message: format!("failed to serialize call hierarchy item: {e}"),
        display_data: None,
        source: None,
    })?;
    ctx.lsp
        .send_request(path, method, json!({ "item": item_value }))
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("LSP request failed: {e}"),
            display_data: None,
            source: None,
        })
}

/// Convert raw JSON-RPC response + operation into the structured
/// [`LspOutput`] (markdown summary + counts).
fn format_output(
    op: LspAction,
    raw: &Value,
    file_path: &str,
    cwd: Option<&str>,
) -> Result<LspOutput, ToolError> {
    let mut out = LspOutput {
        operation: op.as_str().to_string(),
        result: String::new(),
        file_path: file_path.to_string(),
        result_count: None,
        file_count: None,
    };

    match op {
        LspAction::GoToDefinition | LspAction::GoToImplementation => {
            let locs = filter_locations_ignored(parse_locations(raw), cwd);
            out.file_count = Some(count_unique_files(&locs));
            out.result_count = Some(locs.len() as i32);
            out.result = format_definition_result(&locs, cwd);
        }
        LspAction::FindReferences => {
            let locs = filter_locations_ignored(parse_locations(raw), cwd);
            out.file_count = Some(count_unique_files(&locs));
            out.result_count = Some(locs.len() as i32);
            out.result = format_references_result(&locs, cwd);
        }
        LspAction::Hover => {
            let hover: Option<HoverResult> = if raw.is_null() {
                None
            } else {
                serde_json::from_value(raw.clone()).ok()
            };
            out.result = format_hover_result(hover.as_ref());
            out.result_count = Some(if hover.is_some() { 1 } else { 0 });
        }
        LspAction::DocumentSymbol => {
            let symbols: Vec<DocumentSymbol> = parse_or_default(raw);
            out.result_count = Some(symbols.len() as i32);
            out.result = format_document_symbols(&symbols);
        }
        LspAction::WorkspaceSymbol => {
            let symbols = filter_symbols_ignored(parse_or_default(raw), cwd);
            let uri_set: std::collections::HashSet<&str> =
                symbols.iter().map(|s| s.location.uri.as_str()).collect();
            out.file_count = Some(uri_set.len() as i32);
            out.result_count = Some(symbols.len() as i32);
            out.result = format_workspace_symbols(&symbols, cwd);
        }
        LspAction::PrepareCallHierarchy => {
            let items: Vec<CallHierarchyItem> = parse_or_default(raw);
            out.result_count = Some(items.len() as i32);
            out.result = format_call_hierarchy(&items, cwd);
        }
        LspAction::IncomingCalls => {
            let calls: Vec<IncomingCall> = parse_or_default(raw);
            let uri_set: std::collections::HashSet<&str> =
                calls.iter().map(|c| c.from.uri.as_str()).collect();
            out.file_count = Some(uri_set.len() as i32);
            out.result_count = Some(calls.len() as i32);
            out.result = format_incoming_calls(&calls, cwd);
        }
        LspAction::OutgoingCalls => {
            let calls: Vec<OutgoingCall> = parse_or_default(raw);
            let uri_set: std::collections::HashSet<&str> =
                calls.iter().map(|c| c.to.uri.as_str()).collect();
            out.file_count = Some(uri_set.len() as i32);
            out.result_count = Some(calls.len() as i32);
            out.result = format_outgoing_calls(&calls, cwd);
        }
    }

    Ok(out)
}

/// Tolerant JSON parse — returns the default value when the response is
/// `null` or shape-incompatible. LSP servers can legitimately reply with
/// `null` for "no result" (e.g. hover on whitespace), so this is not an
/// error.
fn parse_or_default<T: serde::de::DeserializeOwned + Default>(raw: &Value) -> T {
    if raw.is_null() {
        return T::default();
    }
    serde_json::from_value(raw.clone()).unwrap_or_default()
}

/// Drop locations that point at gitignored files. Uses the in-process
/// [`coco_file_ignore::PathChecker`] (the same one `GrepTool` / `GlobTool`
/// rely on) so we don't fork a subprocess per call.
///
/// `cwd` is the workspace anchor — `None` (no override available) skips
/// filtering entirely, since `PathChecker` needs a root to anchor
/// gitignore discovery. The filter is best-effort: any URI that can't
/// be parsed back to a path stays in the result list.
fn filter_locations_ignored(locs: Vec<LspLocation>, cwd: Option<&str>) -> Vec<LspLocation> {
    let Some(checker) = build_path_checker(cwd) else {
        return locs;
    };
    locs.into_iter()
        .filter(|loc| !is_uri_ignored(&checker, &loc.uri))
        .collect()
}

/// Same as [`filter_locations_ignored`] but for `workspace/symbol`
/// (carries `Location` inside each `SymbolInformation`).
fn filter_symbols_ignored(
    symbols: Vec<SymbolInformation>,
    cwd: Option<&str>,
) -> Vec<SymbolInformation> {
    let Some(checker) = build_path_checker(cwd) else {
        return symbols;
    };
    symbols
        .into_iter()
        .filter(|s| !is_uri_ignored(&checker, &s.location.uri))
        .collect()
}

fn build_path_checker(cwd: Option<&str>) -> Option<coco_file_ignore::PathChecker> {
    let cwd = cwd?;
    let root = std::path::Path::new(cwd);
    Some(coco_file_ignore::PathChecker::new(
        root,
        &coco_file_ignore::IgnoreConfig::default(),
    ))
}

fn is_uri_ignored(checker: &coco_file_ignore::PathChecker, uri: &str) -> bool {
    let path_str = crate::tools::lsp::uri_to_file_path(uri);
    if path_str.is_empty() {
        return false;
    }
    checker.is_ignored(std::path::Path::new(&path_str))
}

/// Parse the `textDocument/{definition,implementation,references}`
/// response into a flat `Vec<LspLocation>`. The LSP spec allows three
/// shapes:
///   - `null` (no result)
///   - a single `Location` object
///   - `Location[]` (array)
///   - `LocationLink[]` (different shape — `targetUri` / `targetRange`)
///
/// We accept all four uniformly so the formatter doesn't have to branch.
fn parse_locations(raw: &Value) -> Vec<LspLocation> {
    use crate::tools::lsp::LspRange;

    fn from_link(link: &Value) -> Option<LspLocation> {
        let uri = link.get("targetUri")?.as_str()?.to_string();
        let range: LspRange = serde_json::from_value(link.get("targetRange")?.clone()).ok()?;
        Some(LspLocation { uri, range })
    }

    match raw {
        Value::Null => Vec::new(),
        Value::Object(_) => {
            if let Ok(loc) = serde_json::from_value::<LspLocation>(raw.clone()) {
                vec![loc]
            } else if let Some(link) = from_link(raw) {
                vec![link]
            } else {
                Vec::new()
            }
        }
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                serde_json::from_value::<LspLocation>(item.clone())
                    .ok()
                    .or_else(|| from_link(item))
            })
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
#[path = "lsp_tool.test.rs"]
mod tests;
