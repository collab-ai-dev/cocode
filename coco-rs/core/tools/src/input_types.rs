use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Output mode for the Grep tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GrepOutputMode {
    /// Show matching lines with context.
    Content,
    /// Show only file paths containing matches.
    #[default]
    FilesWithMatches,
    /// Show match counts per file.
    Count,
}

/// Action for the LSP tool — `operation` discriminated union.
///
/// Wire format is camelCase so the model's tool call validates against the
/// same JSON shape across runtimes. Diagnostics are **not** an action —
/// they flow through the passive `system_reminder` pipeline
/// (`coco_lsp::DiagnosticsStore` + `app/query/reminder_adapters.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum LspAction {
    GoToDefinition,
    FindReferences,
    Hover,
    DocumentSymbol,
    WorkspaceSymbol,
    GoToImplementation,
    PrepareCallHierarchy,
    IncomingCalls,
    OutgoingCalls,
}

impl LspAction {
    /// LSP wire method for the first request. `IncomingCalls` /
    /// `OutgoingCalls` use a two-step lookup that begins with
    /// `prepareCallHierarchy` (the second method
    /// — `callHierarchy/{incomingCalls,outgoingCalls}` — is selected by
    /// the tool layer after `prepareCallHierarchy` returns the item).
    pub fn lsp_method(self) -> &'static str {
        match self {
            Self::GoToDefinition => "textDocument/definition",
            Self::FindReferences => "textDocument/references",
            Self::Hover => "textDocument/hover",
            Self::DocumentSymbol => "textDocument/documentSymbol",
            Self::WorkspaceSymbol => "workspace/symbol",
            Self::GoToImplementation => "textDocument/implementation",
            Self::PrepareCallHierarchy | Self::IncomingCalls | Self::OutgoingCalls => {
                "textDocument/prepareCallHierarchy"
            }
        }
    }

    /// Whether the action's input must include `{line, character}`.
    /// File-scoped (`DocumentSymbol`) and workspace-scoped
    /// (`WorkspaceSymbol`) actions don't.
    pub fn requires_position(self) -> bool {
        !matches!(self, Self::DocumentSymbol | Self::WorkspaceSymbol)
    }

    /// Camel-case wire string matching the `operation` field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GoToDefinition => "goToDefinition",
            Self::FindReferences => "findReferences",
            Self::Hover => "hover",
            Self::DocumentSymbol => "documentSymbol",
            Self::WorkspaceSymbol => "workspaceSymbol",
            Self::GoToImplementation => "goToImplementation",
            Self::PrepareCallHierarchy => "prepareCallHierarchy",
            Self::IncomingCalls => "incomingCalls",
            Self::OutgoingCalls => "outgoingCalls",
        }
    }
}
