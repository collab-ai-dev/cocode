# coco-shell-parser

Shell command parsing (tree-sitter + tokenizer fallback) and security analysis. Infrastructure layer for `exec/shell`, which layers read-only validation and destructive warnings on top.

## Security Analyzers

Risk types (`RiskKind`) span two phases: **Deny** (critical/high, auto-blocked) and **Ask** (medium, require approval). See `security/risks.rs` for the enum — don't cite counts, they grow.

## Key Types

- `ShellParser`, `ParsedShell`, `ShellType`, `detect_shell_type`, `extract_shell_script`
- `Tokenizer`, `Token`, `TokenKind`, `Span`
- `PipeSegment`, `Redirect`, `RedirectKind`
- `safety::{is_known_safe_command, command_might_be_dangerous}`
- `security::{analyze, SecurityAnalysis, RiskLevel}`
- `summary::{CommandSummary, parse_command}`
- Convenience: `parse_and_analyze`, `parse_and_analyze_with`, `is_safe_command`
