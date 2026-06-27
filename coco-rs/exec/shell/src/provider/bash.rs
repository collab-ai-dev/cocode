//! Bash / zsh / sh provider.
//!
//! What this assembles, in order, for every command:
//!
//! 1. (optional) `source <snapshot> 2>/dev/null || true` — restore the
//!    user's interactive environment (aliases, functions, exports) that
//!    we captured once at session start. The `|| true` guards a TOCTOU
//!    where the file vanishes between [`Self::build_exec_command`]'s
//!    `path().exists()` check and the spawned shell's `source`.
//!
//! 2. (optional) session-env hook script — concatenated output of
//!    `SessionStart` / `Setup` / `CwdChanged` / `FileChanged` hooks.
//!    See [`crate::session_env::SessionEnvReader`].
//!
//! 3. `shopt -u extglob 2>/dev/null || true` (bash) or
//!    `setopt NO_EXTENDED_GLOB 2>/dev/null || true` (zsh) — defense in
//!    depth against extended-glob expansion bypassing our security checks
//!    by exploiting filenames that expand at runtime.
//!
//! 4. `eval <quoted user command>` — single-quoted via the canonical
//!    `'"'"'` escape so internal quotes / `!` / jq filters survive. The
//!    redirect `< /dev/null` is appended *outside* the eval (when safe)
//!    so the first process in a pipeline inherits `/dev/null` for stdin
//!    rather than the open spawn pipe. See [`crate::pipe_rearrange`].
//!
//! 5. `pwd -P >| <cwd_file>` — physical CWD written to a temp file.
//!    The executor reads + unlinks it after the child exits.
//!
//! When `COCO_SHELL_PREFIX` is set, the whole assembled string is wrapped
//! via [`crate::shell_prefix::format_shell_prefix_command`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use crate::pipe_rearrange::rearrange_pipe_command;
use crate::provider::BuildExecOpts;
use crate::provider::BuiltCommand;
use crate::provider::ShellProvider;
use crate::session_env::SessionEnvReader;
use crate::session_env::SessionEnvVars;
use crate::shell_prefix::format_shell_prefix_command;
use crate::shell_quoting::quote;
use crate::shell_quoting::rewrite_windows_null_redirect;
use crate::shell_types::Shell;
use crate::shell_types::ShellType;
use crate::snapshot::ShellSnapshot;

/// Per-session bash/zsh/sh command assembler.
///
/// Typically constructed once at session bootstrap, wrapped in `Arc`,
/// and threaded through `ToolUseContext` to every tool call.
#[derive(Debug)]
pub struct BashProvider {
    /// Session-scoped shell handle. Holds the watch receiver fed by
    /// [`crate::snapshot::ShellSnapshot::start_snapshotting`].
    shell: Shell,
    /// Reads hook-emitted env scripts from
    /// `<coco_home>/session-env/<session_id>/`. `None` if the session
    /// has no coco_home (tests / SDK).
    session_env_reader: Option<Arc<SessionEnvReader>>,
    /// Env vars set via `/env`. Always present; usually empty.
    session_env_vars: SessionEnvVars,
    /// Resolved `COCO_SHELL_PREFIX` value (settings → env). `None` for
    /// no-prefix case.
    shell_prefix: Option<String>,
}

impl BashProvider {
    /// Construct a provider for the given session.
    ///
    /// `session_env_reader` is `None` for legacy / test callers without
    /// a coco_home directory.
    pub fn new(
        shell: Shell,
        session_env_reader: Option<Arc<SessionEnvReader>>,
        session_env_vars: SessionEnvVars,
        shell_prefix: Option<String>,
    ) -> Self {
        Self {
            shell,
            session_env_reader,
            session_env_vars,
            shell_prefix,
        }
    }

    /// Bare construction without session-env wiring — convenience for
    /// the legacy executor path and tests.
    pub fn from_shell(shell: Shell) -> Self {
        Self::new(shell, None, SessionEnvVars::new(), None)
    }

    /// Borrow the underlying [`Shell`].
    pub fn shell(&self) -> &Shell {
        &self.shell
    }

    /// Resolve the currently-published snapshot, if any. Snapshot must both
    /// be `Some` AND its file still exist on disk (TOCTOU guard); otherwise
    /// we treat it as missing and fall through to the login-shell path.
    fn resolved_snapshot(&self) -> Option<Arc<ShellSnapshot>> {
        let snap = self.shell.shell_snapshot()?;
        if snap.path().exists() {
            Some(snap)
        } else {
            None
        }
    }

    /// `shopt -u extglob` (bash) / `setopt NO_EXTENDED_GLOB` (zsh) /
    /// dual-form fallback (unknown shell, also used when a shell prefix
    /// may swap the actual executing shell behind our back).
    fn extglob_disable_snippet(&self) -> &'static str {
        if self.shell_prefix.is_some() {
            // The wrapper may run a different shell than `shellPath` —
            // emit a form that covers both bash and zsh, silently
            // ignored elsewhere.
            return "{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true";
        }
        match self.shell.shell_type() {
            ShellType::Bash => "shopt -u extglob 2>/dev/null || true",
            ShellType::Zsh => "setopt NO_EXTENDED_GLOB 2>/dev/null || true",
            _ => "{ shopt -u extglob || setopt NO_EXTENDED_GLOB; } >/dev/null 2>&1 || true",
        }
    }
}

#[async_trait]
impl ShellProvider for BashProvider {
    fn shell_type(&self) -> &ShellType {
        self.shell.shell_type()
    }

    fn shell_path(&self) -> &Path {
        self.shell.shell_path()
    }

    async fn build_exec_command(&self, command: &str, opts: &BuildExecOpts) -> BuiltCommand {
        // CWD-tracking file. When sandboxed, must live inside the sandbox
        // tmpdir (everything outside is read-only). Outside the sandbox we
        // include the PID so concurrent test processes (each starting their
        // monotonic `opts.id` counter at 1) don't collide on the same path
        // in `/tmp`.
        let cwd_file_path = match (&opts.use_sandbox, &opts.sandbox_tmp_dir) {
            (true, Some(dir)) => dir.join(format!("cwd-{}", opts.id)),
            _ => std::env::temp_dir().join(format!("coco-{}-{}-cwd", std::process::id(), opts.id)),
        };

        // Defensive rewrite: `2>nul` → `2>/dev/null`.
        let command = rewrite_windows_null_redirect(command);
        // Single-quote the user command for `eval`, appending `< /dev/null`
        // outside the quote when no heredoc / existing redirect is present.
        let quoted_for_eval = rearrange_pipe_command(&command);

        let mut parts: Vec<String> = Vec::with_capacity(6);

        // (1) Source snapshot.
        if let Some(snap) = self.resolved_snapshot() {
            let path_str = snap.path().display().to_string();
            parts.push(format!("source {} 2>/dev/null || true", quote(&[path_str])));
        }

        // (2) Source session-env hook script.
        if let Some(reader) = &self.session_env_reader
            && let Some(script) = reader.script()
        {
            parts.push(script);
        }

        // (3) Disable extglob.
        parts.push(self.extglob_disable_snippet().to_string());

        // (4) eval the user command (already single-quoted with optional
        //     trailing `< /dev/null`).
        parts.push(format!("eval {quoted_for_eval}"));

        // (5) Track CWD via file write.
        parts.push(format!(
            "pwd -P >| {}",
            quote(&[cwd_file_path.display().to_string()])
        ));

        let mut command_string = parts.join(" && ");

        // (6) Optional shell-prefix wrap.
        if let Some(prefix) = &self.shell_prefix
            && !prefix.is_empty()
        {
            command_string = format_shell_prefix_command(prefix, &command_string);
        }

        BuiltCommand {
            command_string,
            cwd_file_path,
        }
    }

    fn spawn_args(&self, command_string: &str) -> Vec<String> {
        // Skip login shell when the snapshot is providing user
        // environment. The snapshot is the whole point of the snapshot
        // pipeline — avoiding 200-500ms login-shell startup per command.
        let snapshot_ready = self.resolved_snapshot().is_some();
        if snapshot_ready {
            vec!["-c".to_string(), command_string.to_string()]
        } else {
            // Fall back to login shell so the user's environment is
            // still available (just slower).
            vec![
                "-c".to_string(),
                "-l".to_string(),
                command_string.to_string(),
            ]
        }
    }

    async fn env_overrides(&self, _command: &str, opts: &BuildExecOpts) -> HashMap<String, String> {
        let mut env = self.session_env_vars.snapshot();

        // Sandbox tmpdir overrides — applied AFTER /env so the user
        // can't override sandbox isolation via `/env TMPDIR=…`.
        if let Some(dir) = &opts.sandbox_tmp_dir {
            let dir_str = dir.display().to_string();
            env.insert("TMPDIR".to_string(), dir_str.clone());
            env.insert("COCO_TMPDIR".to_string(), dir_str);
            // zsh uses TMPPREFIX (default /tmp/zsh) for heredoc temp
            // files, not TMPDIR. Safe to set unconditionally — non-zsh
            // shells ignore it.
            let mut tmpprefix = dir.clone();
            tmpprefix.push("zsh");
            env.insert("TMPPREFIX".to_string(), tmpprefix.display().to_string());
        }

        env
    }
}

#[cfg(test)]
#[path = "bash.test.rs"]
mod tests;
