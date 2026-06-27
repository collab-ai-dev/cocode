# coco-shell

Shell execution with security analysis, destructive-command warnings, sandbox decisions, read-only / mode validation, heredoc + sed-in-place handling, CWD tracking, snapshot capture, hook-emitted session env, and `/env` vars. Builds on `coco-shell-parser` (tree-sitter + tokenizer) for the AST and 23 security check IDs; layers read-only validation, destructive warnings, mode validation, and provider-based command assembly on top.

## Layering

Two layers, plus the parser crate underneath:

- **Provider layer** (`provider::ShellProvider` + impls) — owns every per-shell quirk: snapshot sourcing, session-env injection, extglob disabling, `eval`-quoting (bash) / base64-encoded `-EncodedCommand` (pwsh), `pwd -P` / `(Get-Location).Path` cwd tracking, sandbox `TMPDIR` / `TMPPREFIX` overrides, `COCO_SHELL_PREFIX` wrapping. Adding a new shell = new `ShellProvider` impl, no executor changes.
- **Executor layer** (`ShellExecutor`) — thin spawn / wait / cancel-token / timeout / sandbox-wrap loop on top of the provider's `BuiltCommand`. Holds the per-command `tempfile::TempDir` for sandbox isolation; reads back the cwd file after exec and applies the post-command bare-repo scrub.
- **Parser + 24 security analyzers** live in `coco-shell-parser` (infrastructure). This crate layers read-only validation, destructive warnings, mode validation, sed edit detection, accept-edits auto-allow, and sandbox decision on top.

## Key Types

- **Executor**: `ShellExecutor`, `ShellProgress`, `CommandResult`, `ExecOptions`
- **Provider**: `ShellProvider` (async trait), `BashProvider`, `PowerShellProvider`, `BuildExecOpts`, `BuiltCommand`
- **Session env**: `SessionEnvReader` reads hook scripts under `<coco_home>/session-env/<session_id>/{event}-hook-{idx}.sh` (regex-matched, sorted by event priority then index); `SessionEnvVars` is the in-memory `/env` snapshot
- **Quoting & path rewrites**: `shell_quoting::quote` (bash single-quote with `'"'"'` escape), `pipe_rearrange::rearrange_pipe_command` (appends `< /dev/null` outside the eval-quote when safe so the first pipeline process inherits stdin from `/dev/null`), `shell_prefix::format_shell_prefix_command` (`COCO_SHELL_PREFIX` wrap), `windows_paths::posix_to_windows_path`
- **AST**: `BashNode`, `SimpleCommand`, `parse_command`, `extract_simple_commands`
- **Bash permissions**: `get_command_prefix`, `is_dangerous_bare_prefix`, `split_compound_command`, `strip_all_env_vars`, `strip_safe_wrappers`
- **Security**: `SecurityCheck`, `SecurityCheckId` (23 IDs), `SecuritySeverity`, `SafetyResult`, `check_security`
- **Destructive / read-only / mode**: destructive warnings, read-only rule matcher, `is_auto_allowed_in_accept_edits`, path validation
- **Heredoc**: `HeredocContent`, `extract_heredocs`
- **Sed in-place**: `SedEditInfo`, `parse_sed_edit_command`, `is_sed_in_place_edit`
- **Semantics**: `CommandResultInterpretation`
- **Sandbox decision**: `ExecOptions.sandbox: Option<Arc<coco_sandbox::SandboxState>>` — when `Some`, the executor allocates a per-command `tempfile::TempDir` and calls `state.try_wrap_command_with_binds(...)` before `tokio::process::Command::spawn`. The bind-mounted tmpdir is also handed to the provider as `BuildExecOpts.sandbox_tmp_dir` so the inner shell writes its cwd file inside the sandbox and the provider can inject `TMPDIR` / `TMPPREFIX`. Decision logic (excluded commands, bypass) lives on `SandboxState::command_snapshot`.
- **Snapshot**: `ShellSnapshot::start_snapshotting` (background `bash -i` capture into `<coco_home>/snapshots/`), `cleanup_stale_snapshots` (mtime-based reaper), `SnapshotConfig`
