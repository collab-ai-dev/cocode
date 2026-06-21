//! PowerShell provider (pwsh).
//!
//! Key points:
//!
//! - **CWD tracking** via `(Get-Location).Path | Out-File -FilePath …
//!   -Encoding utf8 -NoNewline`. The path is single-quoted with the
//!   PowerShell `''` escape (not the bash `'"'"'` form).
//!
//! - **Exit-code capture** prefers `$LASTEXITCODE` when a native exe ran,
//!   falling back to `$?` for cmdlet-only pipelines. PS 5.1 sets `$?` to
//!   `$false` when a native command writes to stderr through a redirect
//!   even when exit was 0 — so `$LASTEXITCODE` is more reliable.
//!
//! - **Sandbox path** emits `-EncodedCommand <base64-utf16le>`. The
//!   sandbox runtime wraps the command via its own shell-quote pass
//!   which corrupts `!`/`$`/`?` if those appear in a quoted string;
//!   base64 is `[A-Za-z0-9+/=]` only — passes every quoter unchanged.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;

use crate::provider::BuildExecOpts;
use crate::provider::BuiltCommand;
use crate::provider::ShellProvider;
use crate::session_env::SessionEnvVars;
use crate::shell_types::Shell;
use crate::shell_types::ShellType;

/// Per-session PowerShell command assembler.
#[derive(Debug)]
pub struct PowerShellProvider {
    shell: Shell,
    session_env_vars: SessionEnvVars,
}

impl PowerShellProvider {
    pub fn new(shell: Shell, session_env_vars: SessionEnvVars) -> Self {
        Self {
            shell,
            session_env_vars,
        }
    }

    pub fn from_shell(shell: Shell) -> Self {
        Self::new(shell, SessionEnvVars::new())
    }

    /// PowerShell-style single-quote: `'` becomes `''`.
    fn ps_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "''"))
    }

    /// Base64-encode a string as UTF-16LE for `-EncodedCommand`.
    fn encode_utf16le_base64(cmd: &str) -> String {
        use base64::Engine;
        let mut bytes = Vec::with_capacity(cmd.len() * 2);
        for unit in cmd.encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// Build the cwd-tracking + exit-code-capture tail appended to the
    /// user command.
    fn ps_cwd_tracking(cwd_file_path: &Path) -> String {
        let escaped = Self::ps_quote(&cwd_file_path.display().to_string());
        format!(
            "\n; $_ec = if ($null -ne $LASTEXITCODE) {{ $LASTEXITCODE }} elseif ($?) {{ 0 }} else {{ 1 }}\n; (Get-Location).Path | Out-File -FilePath {escaped} -Encoding utf8 -NoNewline\n; exit $_ec"
        )
    }

    /// Argv that ships with every pwsh invocation.
    pub fn flags() -> [&'static str; 3] {
        ["-NoProfile", "-NonInteractive", "-Command"]
    }
}

#[async_trait]
impl ShellProvider for PowerShellProvider {
    fn shell_type(&self) -> &ShellType {
        self.shell.shell_type()
    }

    fn shell_path(&self) -> &Path {
        self.shell.shell_path()
    }

    async fn build_exec_command(&self, command: &str, opts: &BuildExecOpts) -> BuiltCommand {
        // Outside the sandbox tmpdir, include the PID so concurrent test
        // processes (each starting their `opts.id` counter at 1) don't
        // collide on the same path in the global temp dir.
        let cwd_file_path = match (&opts.use_sandbox, &opts.sandbox_tmp_dir) {
            (true, Some(dir)) => dir.join(format!("cwd-ps-{}", opts.id)),
            _ => {
                std::env::temp_dir().join(format!("coco-pwd-ps-{}-{}", std::process::id(), opts.id))
            }
        };
        let ps_command = format!("{command}{}", Self::ps_cwd_tracking(&cwd_file_path));

        let command_string = if opts.use_sandbox {
            // Sandbox-runtime wraps as `<binShell> -c '<cmd>'` and
            // applies its own shell-quote pass. Encoded base64 dodges
            // every quoting layer cleanly.
            let shell_quoted = Self::ps_quote(&self.shell.shell_path().display().to_string());
            let encoded = Self::encode_utf16le_base64(&ps_command);
            format!("{shell_quoted} -NoProfile -NonInteractive -EncodedCommand {encoded}")
        } else {
            ps_command
        };

        BuiltCommand {
            command_string,
            cwd_file_path,
        }
    }

    fn spawn_args(&self, command_string: &str) -> Vec<String> {
        let f = Self::flags();
        vec![
            f[0].to_string(),
            f[1].to_string(),
            f[2].to_string(),
            command_string.to_string(),
        ]
    }

    async fn env_overrides(&self, _command: &str, opts: &BuildExecOpts) -> HashMap<String, String> {
        let mut env = self.session_env_vars.snapshot();
        // Sandbox isolation wins over user `/env` overrides.
        if let Some(dir) = &opts.sandbox_tmp_dir {
            let dir_str = dir.display().to_string();
            env.insert("TMPDIR".to_string(), dir_str.clone());
            env.insert("COCO_TMPDIR".to_string(), dir_str);
        }
        env
    }
}

#[cfg(test)]
#[path = "powershell.test.rs"]
mod tests;
