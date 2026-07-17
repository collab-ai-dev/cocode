# coco-shell — Crate Plan (HYBRID: cocode-rs base + TS enhancements)

TS source: `utils/bash/` (12K LOC), `utils/Shell.ts`, `utils/shell/`, `tools/BashTool/bashPermissions.ts` (700+), `tools/BashTool/bashSecurity.ts`, `tools/BashTool/readOnlyValidation.ts` (68K), `tools/BashTool/commandSemantics.ts`, `tools/BashTool/destructiveCommandWarning.ts` (103), `tools/BashTool/shouldUseSandbox.ts` (154), `tools/BashTool/modeValidation.ts` (116), `tools/BashTool/sedEditParser.ts` (200+)

**Strategy**: HYBRID — build on cocode-rs `utils/shell-parser` (24 analyzers, native Rust parsing) as the base, then add TS-specific enhancements:
- **KEEP from cocode-rs**: shell-parser crate (command parsing, 24 risk type analyzers, AST analysis) — Rust-native, no WASM dependency
- **ADD from TS**: read-only validation (40 safe commands + 200+ flag configs), destructive command warnings (18 patterns), two-phase wrapper stripping (HackerOne #3543050), 7-phase permission pipeline, command semantics, heredoc extraction, CWD tracking, 3449-input test corpus
- **Rationale**: cocode-rs shell-parser has stronger Rust-native parsing infrastructure; TS has 4x more security validation coverage. Merge both.

## Dependencies

```
coco-shell depends on:
  - coco-types       (PermissionMode; `SandboxMode` is consumed indirectly through `coco-sandbox`)
  - utils/shell-parser (KEEP: 24 risk type analyzers, AST parsing — extend with TS's 23 SecurityCheckId validators, read-only allowlist, destructive patterns)
  - coco-sandbox     (`SandboxState`, `SandboxBypass` — `ExecOptions.sandbox` is an `Arc<SandboxState>`; the executor calls `state.try_wrap_command(...)` before each spawn. The previous local `coco_shell::sandbox` mirror module has been deleted.)
  - tokio, tokio-util (CancellationToken, process spawning)

coco-shell does NOT depend on:
  - coco-tool     (no Tool trait awareness)
  - coco-config   (no settings — receives options via ExecOptions parameter)
  - coco-permissions (no permission evaluation — caller handles that)
  - any app/ crate
```

## Data Definitions

### Parsed Command (from `utils/bash/ParsedCommand.ts`)

```rust
pub trait ParsedCommand {
    fn original(&self) -> &str;
    /// Returns pipe segments as borrowed slices (no heap allocation on hot path).
    fn pipe_segments(&self) -> Vec<&str>;
    fn without_output_redirections(&self) -> String;
    fn redirections(&self) -> &[OutputRedirection];
}

pub struct OutputRedirection {
    pub target: String,
    pub operator: RedirectOp,  // Append, Overwrite
    pub fd: Option<i32>,       // 1 = stdout, 2 = stderr, None = default
}

// Two implementations (from TS):
// 1. TreeSitterParsedCommand — accurate, quote-aware
// 2. RegexParsedCommand — fallback when tree-sitter unavailable
```

### Bash Security (from `bashSecurity.ts` — 20+ check categories)

```rust
/// Security check IDs. Uses newtype for extensibility without recompilation.
/// Each check is a compile-time constant — no magic numbers in code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SecurityCheckId(i32);

impl SecurityCheckId {
    // IDs match TS bashSecurity.ts BASH_SECURITY_CHECK_IDS exactly
    pub const INCOMPLETE_COMMANDS: Self = Self(1);          // tab-start, flag-start, operator-start
    pub const JQ_SYSTEM_FUNCTION: Self = Self(2);
    pub const JQ_FILE_ARGUMENTS: Self = Self(3);            // -f, --from-file, --rawfile, --slurpfile, -L
    pub const OBFUSCATED_FLAGS: Self = Self(4);             // ANSI-C $'..', locale $"..", empty-quote-dash
    pub const SHELL_METACHARACTERS: Self = Self(5);         // ; | & in args including -name/-path/-regex
    pub const DANGEROUS_VARIABLES: Self = Self(6);          // variables adjacent to redirections/pipes
    pub const NEWLINES: Self = Self(7);                     // unquoted LF + CR (shell-quote/bash IFS differential)
    pub const DANGEROUS_PATTERNS_CMD_SUBSTITUTION: Self = Self(8); // $(), backticks, <(), >(), =() zsh, ${}
    pub const DANGEROUS_PATTERNS_INPUT_REDIR: Self = Self(9);
    pub const DANGEROUS_PATTERNS_OUTPUT_REDIR: Self = Self(10);
    pub const IFS_INJECTION: Self = Self(11);               // $IFS, ${...IFS...}
    pub const GIT_COMMIT_SUBSTITUTION: Self = Self(12);     // cmd substitution in git commit -m "..."
    pub const PROC_ENVIRON_ACCESS: Self = Self(13);         // /proc/*/environ
    pub const MALFORMED_TOKEN_INJECTION: Self = Self(14);   // unbalanced tokens + cmd separators (HackerOne)
    pub const BACKSLASH_ESCAPED_WHITESPACE: Self = Self(15); // `\ `, `\<NL>` mid-word
    pub const BRACE_EXPANSION: Self = Self(16);             // comma/sequence expansion + mismatched-brace attack
    pub const CONTROL_CHARACTERS: Self = Self(17);          // 0x00-0x08, 0x0B-0x0C, 0x0E-0x1F, 0x7F
    pub const UNICODE_WHITESPACE: Self = Self(18);          // NBSP, zero-width, BOM, line/paragraph separators
    pub const MID_WORD_HASH: Self = Self(19);               // shell-quote/bash comment-start differential
    pub const ZSH_DANGEROUS_COMMANDS: Self = Self(20);      // zmodload, emulate, sysopen, zpty, ztcp, etc.
    pub const BACKSLASH_ESCAPED_OPERATORS: Self = Self(21);  // \;, \|, \&, \<, \> with AST operator nodes
    pub const COMMENT_QUOTE_DESYNC: Self = Self(22);        // quote chars inside # comments desyncing trackers
    pub const QUOTED_NEWLINE: Self = Self(23);              // newline inside quotes where next line starts with #
}

pub static ZSH_DANGEROUS_COMMANDS: LazyLock<HashSet<&str>> = ...; // zmodload, emulate, sysopen, sysread, syswrite, zpty, ztcp, zsocket, mapfile, zf_rm, zf_mv, zf_ln, zf_chmod

/// IMPORTANT: Validator ordering is security-critical.
/// Pipeline has two phases:
/// 1. Early-allow validators: short-circuit to passthrough for known-safe commands
///    (validateEmpty, validateSafeCommandSubstitution for safe heredoc, validateGitCommit)
/// 2. Split into misparsing vs non-misparsing validators:
///    - Misparsing results (isBashSecurityCheckForMisparsing=true) are prioritized
///    - Non-misparsing results are deferred to prevent early-exit from suppressing
///      a later misparsing-flagged result
pub fn check_security(command: &str) -> Vec<SecurityCheck>;
pub fn strip_safe_redirections(command: &str) -> String;  // remove 2>&1, >/dev/null
pub fn has_unescaped_char(content: &str, ch: char) -> bool;
pub fn extract_quoted_content(command: &str) -> QuotedContent;
```

### Heredoc Extraction (from `utils/bash/heredoc.ts`, 734 LOC)

```rust
/// Extracts heredoc content from shell commands, replacing with random-salt placeholders.
/// This is critical for security: heredoc content must be removed before security
/// validators analyze the command, since heredoc bodies are data not code.
///
/// Key features:
/// - 6 delimiter variants: <<WORD, <<'WORD', <<"WORD", <<-WORD, <<-'WORD', <<\WORD
/// - Quoted-only mode: unquoted heredocs stay visible to security validators
///   (their content undergoes variable expansion, so they ARE code-like)
/// - Incremental O(n) quote/comment state tracker (previous impl was O(n²))
/// - PST_EOFTOKEN early-close detection (matches bash makes_cmd.c:606 behavior)
/// - Arithmetic-context ((x = 1 << 2)) bail-out (prevents misparsing as heredoc)
/// - Backslash-escaped \<< rejection
/// - Multi-heredoc collision detection (same content-start-index)
/// - Nesting filter (heredoc inside heredoc content)
/// - Random-salt placeholders to prevent injection via placeholder collision
pub fn extract_heredocs(command: &str, quoted_only: bool) -> (String, Vec<HeredocContent>);

pub struct HeredocContent {
    pub delimiter: String,
    pub content: String,
    pub is_quoted: bool,  // quoted delimiters suppress variable expansion
}
```

### Pure-TS Bash Parser (from `utils/bash/bashParser.ts`)

```rust
/// Pure Rust reimplementation of tree-sitter-bash parser producing compatible ASTs.
/// Used as primary parser; tree-sitter WASM is fallback.
/// Constraints:
/// - 50ms wall-clock parse timeout (prevents DoS on adversarial input)
/// - 50,000 AST node budget (prevents OOM on deeply nested constructs)
/// - Produces tree-sitter-compatible ASTs for downstream analysis
///
/// Handles full tokenizer: WORD, DQUOTE, SQUOTE, DOLLAR, BACKTICK, etc.
/// and all shell keywords (if/then/else/fi, for/while/until, case/esac, function).
pub fn parse_bash(command: &str) -> Result<AstNode, ParseError>;
```

### Bash Permissions (from `bashPermissions.ts`, 700+ LOC)

```rust
pub fn get_simple_command_prefix(command: &str) -> Option<String>;
// "git commit -m 'fix'" -> "git commit"
// Skips safe env vars (NODE_ENV, RUST_LOG, etc.)

pub fn get_first_word_prefix(command: &str) -> Option<String>;
// UI fallback, excludes BARE_SHELL_PREFIXES

pub static SAFE_ENV_VARS: LazyLock<HashSet<&str>> = ...; // NODE_ENV, RUST_LOG, PYTHON_ENV, GO_ENV...
pub static BARE_SHELL_PREFIXES: LazyLock<HashSet<&str>> = ...; // sh, bash, zsh, env, xargs, sudo, nohup...

pub const MAX_SUBCOMMANDS_FOR_SECURITY_CHECK: usize = 50;

/// Full bash permission analysis pipeline:
/// 1. parseForSecurityFromAst() — tree-sitter bash AST parsing
/// 2. checkSemantics() — command semantics validation
/// 3. checkCommandOperatorPermissions() — pipe/redirect analysis
/// 4. checkPathConstraints() — file path whitelist/blacklist
/// 5. checkSedConstraints() — sed in-place edit validation
/// 6. shouldUseSandbox() — sandbox decision
/// 7. checkPermissionMode() — mode-specific auto-allow
///
/// Compound command splitting: handles cmd1 && cmd2 | cmd3 branching
/// Env var stripping: FOO=bar cmd → analyze cmd
/// Wrapper stripping: TWO-PHASE architecture (HackerOne #3543050):
///   Phase 1: strip env vars only (+ comment lines)
///   Phase 2: strip wrapper commands only (NOT env vars, because wrappers
///            use execvp so VAR=val AFTER wrapper is the actual command)
///   Wrappers: timeout (full GNU flags), time, nice (3 forms), stdbuf, nohup
/// Complexity guard: 50-command limit to prevent event loop starvation
/// Fixed-point candidate expansion: both shouldUseSandbox and deny-rule
/// matching interleave stripAllLeadingEnvVars + stripSafeWrappers iteratively
```

### Read-Only Validation (from `readOnlyValidation.ts`, 68K)

```rust
pub struct CommandConfig {
    pub safe_flags: HashMap<String, FlagArgType>,
    pub regex: Option<Regex>,
    /// Extra validation for commands needing context-aware checks.
    /// Uses enum dispatch instead of function pointers (avoids fn pointer anti-pattern).
    pub extra_check: Option<ExtraValidation>,
    pub respects_double_dash: bool,
}

pub enum ExtraValidation {
    /// git pickaxe flags (-S, -G, -O) must take an argument
    GitPickaxeArgRequired,
    /// docker: reject write subcommands (rm, rmi, exec, etc.)
    DockerReadOnly,
    /// gh: reject mutating API methods (POST, PUT, DELETE)
    GhReadOnly,
}

/// 6 flag argument types for strict validation:
pub enum FlagArgType {
    None,        // no argument (e.g., --color, -n)
    StringArg,   // any string (e.g., --relative=path). Rejects leading '-' (except git --sort)
    NumberArg,   // integer only (e.g., --context=3). Rejects non-digits
    CharArg,     // single character (delimiter)
    LiteralCurly, // literal "{}" only (xargs replace-string)
    LiteralEof,  // literal "EOF" only (xargs eof-string)
}

/// Security: combined flags like -nr rejected if any bundled flag requires argument
/// (parser differential prevention). --flag=value format enforced for inline values.
/// git -S/-G/-O explicitly StringArg (not None) to prevent pickaxe flag injection.
/// xargs -i/-e removed (optional-argument getopt bug); use -I {}/-E EOF instead.
/// fd --exec excluded (subprocess execution risk).

/// Allowlisted safe commands with flag validation:
/// - git (show, log, diff, status, branch, blame, help, rev-list, reflog, shortlog — 200+ safe flags)
/// - file, sort, man, netstat, ps, xargs, head, tail, wc
/// - ripgrep, docker (read-only ops), pyright
/// - gh (pr view, issue view, workflow run — ant-only, network-safe)
///
/// SECURITY: Pickaxe search flags (-S, -G, -O) must take arguments
/// to prevent "git diff -S -- --output=/tmp/pwned" bypass.
/// UNC path detection: containsVulnerableUncPath() for Windows credential leak prevention.
pub static COMMAND_ALLOWLIST: LazyLock<HashMap<&str, CommandConfig>> = ...;

pub fn is_read_only_command(command: &str) -> bool;
```

## Destructive Command Warning (from `destructiveCommandWarning.ts`, 103 LOC)

```rust
/// Pattern-based destructive command detection (~20 patterns).
/// Returns a human-readable warning string if command is destructive.
/// Called by: BashTool in coco-tools (during tool execution, before exec).
///
/// Covered patterns:
///   git reset --hard       → "may discard uncommitted changes"
///   git push --force/-f    → "may overwrite remote history"
///   git clean -f (no -n)   → "may permanently delete untracked files"
///   git checkout .          → "may discard all working tree changes"
///   git --no-verify         → "may skip safety hooks"
///   git commit --amend      → "may rewrite the last commit"
///   rm -rf                  → "may recursively force-remove files"
///   DROP/TRUNCATE TABLE     → "may drop or truncate database"
///   kubectl delete          → "may delete Kubernetes resources"
///   terraform destroy       → "may destroy Terraform infrastructure"
///   docker rm/rmi --force   → "may remove containers/images"
///   ... (~10 more patterns)
pub fn get_destructive_command_warning(command: &str) -> Option<String>;
```

## Sandbox Decision (from `shouldUseSandbox.ts`, 154 LOC)

```rust
/// Complex sandbox decision logic:
/// 1. Check SandboxManager.is_sandboxing_enabled()
/// 2. Check dangerously_disable_sandbox override + policy
/// 3. Check contains_excluded_command(command):
///    - Dynamic GrowthBook: tengu_sandbox_disabled_commands
///    - User config: settings.sandbox.excluded_commands
///    - Wildcard + prefix matching on command candidates
/// 4. Return true if sandboxing applies
pub fn should_use_sandbox(command: Option<&str>, disable_override: bool) -> bool;

/// Check if command matches any excluded patterns
fn contains_excluded_command(command: &str) -> bool;
```

## Mode Validation (from `modeValidation.ts`, 116 LOC)

```rust
/// Permission mode-specific auto-allow logic.
///
/// acceptEdits mode: auto-allow filesystem commands.
/// NOTE: TS auto-allows ALL of these without flag restriction:
///   mkdir, touch, rm, rmdir, mv, cp, sed
/// No per-flag validation is applied in current TS implementation.
const ACCEPT_EDITS_ALLOWED_COMMANDS: &[&str] = &[
    "mkdir", "touch", "rm", "rmdir", "mv", "cp", "sed"
];

/// Check if permission mode auto-allows this command.
/// - acceptEdits + safe filesystem command → allow
/// - acceptEdits + flag-validated command + safe flags → allow
/// - acceptEdits + flag-validated command + dangerous flags → passthrough (ask user)
/// - bypassPermissions / dontAsk → passthrough (handled elsewhere)
/// - default / auto / plan → passthrough
pub fn check_permission_mode(
    command: &str,
    mode: &PermissionMode,
) -> PermissionCheckResult;

pub enum PermissionCheckResult {
    Allow,
    Passthrough,  // No mode-specific handling; let caller decide
}

pub fn get_auto_allowed_commands(mode: &PermissionMode) -> &[&str];
```

## Sed Edit Parser (from `sedEditParser.ts`, 200+ LOC)

```rust
/// Parses sed in-place edit commands to extract structured edit info
/// for file-edit-style rendering in the TUI.
///
/// Handles: shell quote handling, -i/-E/-r/-e flags, escaped chars
/// Converts BRE (Basic RE) to Rust regex via null-byte sentinels
/// Constraint: only supports '/' delimiter; rejects globs, multiple files
pub fn parse_sed_edit(command: &str) -> Option<SedEditInfo>;

pub struct SedEditInfo {
    pub file_path: String,
    pub search_pattern: String,
    pub replacement: String,
    pub flags: SedFlags,  // g, i, m, p, 1-9
}
```

## CWD Tracking (from `Shell.ts`)

```rust
/// CWD tracking via hidden temp file + pwd -P markers.
/// After each command: parse markers to detect directory changes.
/// Deleted CWD recovery: detect and recover when current dir is deleted.
/// NFC normalization: normalize Unicode paths (macOS HFS+).
/// Hook integration: CwdChanged hook event on directory changes.
pub struct CwdTracker {
    pub current_dir: PathBuf,
    marker_path: PathBuf,
}
```

### Command Semantics (from `commandSemantics.ts`)

```rust
/// Exit-code interpretation for known commands.
/// Maps non-zero exit codes to semantic meanings (not errors).
/// E.g., grep exit=1 means "no matches" (not failure),
/// diff exit=1 means "files differ", test exit=1 means "false condition".
pub fn interpret_command_result(command: &str, exit_code: i32) -> CommandResultInterpretation;

pub enum CommandResultInterpretation {
    /// Non-zero exit is a genuine error
    Error,
    /// Non-zero exit has a known semantic meaning (not a real error)
    Expected { description: String },
}

/// Extract the base command from a pipeline (last segment).
pub fn heuristically_extract_base_command(command: &str) -> &str;
```

## Core Logic

### Shell Execution (from `utils/Shell.ts`)

```rust
pub struct ShellExecutor {
    shell_path: PathBuf,  // detected via findSuitableShell()
    cwd: PathBuf,
}

pub struct ExecOptions {
    pub timeout: Duration,              // default: 30 minutes (DEFAULT_TIMEOUT = 30 * 60 * 1000ms in TS)
    pub prevent_cwd_changes: bool,
    pub should_use_sandbox: bool,
    pub should_auto_background: bool,
    pub extra_env: HashMap<String, String>,  // additional env vars for this command
    pub cwd_override: Option<PathBuf>,       // override working directory
}

pub struct ShellResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub new_cwd: Option<PathBuf>,
    pub timed_out: bool,
}

impl ShellExecutor {
    /// Detect suitable shell: CLAUDE_CODE_SHELL env > SHELL env > search /bin,/usr/bin
    pub async fn new(cwd: PathBuf) -> Result<Self, ShellError>;
    pub async fn exec(&self, command: &str, options: ExecOptions, cancel: CancellationToken) -> Result<ShellResult, ShellError>;
}
```

### Shell Providers

Two shell provider implementations:

```rust
/// Bash provider (default on macOS/Linux)
pub struct BashProvider;

/// PowerShell provider (Windows, from powershellProvider.ts + powershellDetection.ts)
pub struct PowerShellProvider;

pub trait ShellProvider {
    fn wrap_command(&self, command: &str, options: &ExecOptions) -> String;
    fn parse_cwd_marker(&self, output: &str) -> Option<PathBuf>;
}
```

### Shell Integration (from ShellSnapshot.ts)

Embeds tools into the shell session via argv0 dispatch:
- `createArgv0ShellFunction()` — shell function that dispatches based on binary name
- `createRipgrepShellIntegration()` — embedded ripgrep
- `createFindGrepShellIntegration()` — embedded bfs/ugrep
