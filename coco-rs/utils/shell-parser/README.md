# coco-shell-parser

Shell command parsing, security analysis, command summarization, and argv-based safety detection.

## Capabilities

### Parsing (`parser.rs`)

Tree-sitter AST parsing with tokenizer fallback.

- `ShellParser::parse(source)` — parse a shell command string into `ParsedShell`
- `ParsedShell::try_extract_safe_commands()` — extract word-only command sequences (rejects substitutions, redirections, subshells)
- `ParsedShell::extract_commands()` — extract all commands (relaxed, for analysis)
- `extract_shell_script(argv)` — detect shell invocation from argv (bash/zsh/sh/PowerShell/cmd)
- `detect_shell_type(path)` — identify shell type from executable path

### Security Analysis (`security/`)

24 risk analyzers across two phases.

- `analyze(cmd)` — run all analyzers, returns `SecurityAnalysis` with risks
- **Deny phase** (16 analyzers): auto-denied injection patterns (single-quote bypass, shell metacharacters, IFS injection, etc.)
- **Ask phase** (8 analyzers): require user approval (network exfiltration, privilege escalation, file system tampering, code execution, etc.)
- `SecurityRisk` with `RiskKind`, `RiskLevel` (Low/Medium/High/Critical), `RiskPhase` (Deny/Ask)

### Command Summary (`summary/`)

Ported from `codex-rs/shell-command/src/parse_command.rs`. Classifies commands into human-readable categories.

- `parse_command(argv) -> Vec<CommandSummary>` — classify an argv command
- `CommandSummary::Read { cmd, name, path }` — file read (cat, head, tail, sed -n, bat, less)
- `CommandSummary::Search { cmd, query, path }` — search (grep, rg, ag, git grep, fd, find)
- `CommandSummary::ListFiles { cmd, path }` — directory listing (ls, tree, du, rg --files, find)
- `CommandSummary::Unknown { cmd }` — unrecognized command

Handles `bash -lc "..."` recursion, pipeline simplification (strips formatting helpers like `wc`, `head`, `tail`), and `cd` tracking for path resolution.

### Argv Safety Detection (`safety/`)

Ported from `codex-rs/shell-command/src/command_safety/`. Operates on argv arrays.

- `is_known_safe_command(argv)` — whitelist check with per-binary rules:
  - **git**: only read-only subcommands; blocks `-c` config override, `branch -d`
  - **find**: blocks `-exec`, `-delete`, `-fls`, `-fprint`
  - **rg**: blocks `--pre`, `--search-zip`
  - **base64**: blocks `--output`
  - **sed**: only `-n {N,M}p` pattern
  - **bash -lc** recursion: parses script, validates each sub-command individually
- `command_might_be_dangerous(argv)` — detects `rm -rf`, `sudo` patterns

## Current Integration

```
core/tools/bash.rs:check_permission()
  │
  ├─ is_read_only_command(command)        ← exec/shell (string whitelist, fast path)
  ├─ is_known_safe_command(shell_argv)    ← shell-parser/safety (argv, compound commands)
  ├─ parse_and_analyze(command)           ← shell-parser/security (24 analyzers)
  └─ check_compound_risks(commands)       ← core/tools (structural checks)
```

| API | Caller | Status |
|-----|--------|--------|
| `ParsedShell` + `parse_and_analyze` | `core/tools/bash.rs`, `exec/shell/readonly.rs` | Integrated |
| `security::analyze` | `core/tools/bash.rs`, `exec/shell/readonly.rs` | Integrated |
| `is_known_safe_command` | `core/tools/bash.rs:323` | Integrated |
| `command_might_be_dangerous` | — | Not yet integrated |
| `CommandSummary` + `parse_command` | — | Not yet integrated |

## Future Integration Work

### CommandSummary in TUI approval prompts

When the Bash tool requests user approval, the TUI currently shows the raw command string. `parse_command()` could provide structured summaries:

- "Reading `Cargo.toml`" instead of `cat Cargo.toml`
- "Searching for `TODO` in `src/`" instead of `rg -n TODO src/`
- "Listing files in `packages/`" instead of `ls -la packages/`

**Requires**: Add `CommandSummary` to `coco-protocol` approval parameters, consume in `app/tui` rendering.

### command_might_be_dangerous for plan mode

The plan mode safety check (`is_plan_mode_allowed`) could use `command_might_be_dangerous` for faster rejection of obviously destructive commands (`rm -rf`, `sudo`).

### CommandSummary for command logging

Structured command classification (Read/Search/ListFiles/Unknown) could enrich telemetry and session history with semantic command categories.
