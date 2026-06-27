//! LSP integration for tool context.

/// LSP diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// An LSP diagnostic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub file_path: String,
    pub line: i32,
    pub column: i32,
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
}

/// Symbol information from LSP.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub line: i32,
    pub column: i32,
}

/// Format diagnostics for display in tool results.
pub fn format_diagnostics(diagnostics: &[Diagnostic]) -> String {
    if diagnostics.is_empty() {
        return "No diagnostics.".to_string();
    }
    diagnostics
        .iter()
        .map(|d| {
            format!(
                "{}:{}:{}: {:?}: {}",
                d.file_path, d.line, d.column, d.severity, d.message
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Format symbols for display.
pub fn format_symbols(symbols: &[SymbolInfo]) -> String {
    if symbols.is_empty() {
        return "No symbols found.".to_string();
    }
    symbols
        .iter()
        .map(|s| format!("{} ({}) at {}:{}", s.name, s.kind, s.file_path, s.line))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "lsp_integration.test.rs"]
mod tests;
