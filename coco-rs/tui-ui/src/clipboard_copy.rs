//! Clipboard copy backend for the TUI's `/copy` command.
//!
//! This module decides *how* to get text onto the user's clipboard based on the
//! current environment. The selection order is:
//!
//! 1. **SSH session** (`SSH_TTY` / `SSH_CONNECTION` set): use OSC 52 exclusively,
//!    because the native clipboard belongs to the remote machine.
//! 2. **Local session**: try `arboard` (native clipboard) first. On WSL, fall back
//!    to the Windows clipboard through PowerShell if `arboard` fails. Finally, fall
//!    back to OSC 52 if no native/WSL clipboard path succeeds.
//!
//! On Linux, X11 and some Wayland compositors require the process that wrote the
//! clipboard to keep its handle open. [`ClipboardLease`] wraps the
//! `arboard::Clipboard` so callers can store it for the lifetime of the TUI. On
//! other platforms the lease is always `None`.
//!
//! The module is intentionally narrow: text copy only, user-facing error strings,
//! no reusable clipboard abstraction. Image paste lives in `paste.rs`.
//!
//! Port of `codex-rs/tui/src/clipboard_copy.rs`.

use base64::Engine;
use std::io::Write;

/// Maximum raw bytes we will base64-encode into an OSC 52 sequence.
/// Large payloads are rejected before encoding to avoid overwhelming the terminal.
const OSC52_MAX_RAW_BYTES: usize = 100_000;

#[cfg(target_os = "macos")]
static STDERR_SUPPRESSION_MUTEX: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

/// Copy text to the system clipboard.
///
/// Over SSH, uses OSC 52 so the text reaches the *local* terminal emulator's
/// clipboard rather than a remote X11/Wayland clipboard that the user cannot
/// access. On a local session, tries `arboard` (native clipboard) first and
/// falls back to WSL PowerShell, then OSC 52, if needed.
///
/// OSC 52 is supported by kitty, WezTerm, iTerm2, Ghostty, and others.
pub fn copy_to_clipboard(text: &str) -> Result<Option<ClipboardLease>, String> {
    copy_to_clipboard_with(
        text,
        is_ssh_session(),
        is_wsl_session(),
        osc52_copy,
        arboard_copy,
        wsl_clipboard_copy,
    )
}

/// Keeps a platform clipboard owner alive when the backend requires one.
///
/// On Linux/X11 and some Wayland compositors, clipboard contents are served by
/// the owning process. Dropping the `arboard::Clipboard` before the user pastes
/// causes the content to vanish. Store this lease on the widget that triggered
/// the copy so the handle lives as long as the TUI does. On non-Linux native
/// paths and OSC 52 paths the lease is `None` — those backends do not require
/// process-lifetime ownership.
pub struct ClipboardLease {
    #[cfg(target_os = "linux")]
    _clipboard: Option<arboard::Clipboard>,
}

impl std::fmt::Debug for ClipboardLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardLease").finish_non_exhaustive()
    }
}

impl ClipboardLease {
    #[cfg(target_os = "linux")]
    fn native_linux(clipboard: arboard::Clipboard) -> Self {
        Self {
            _clipboard: Some(clipboard),
        }
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn test() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            _clipboard: None,
        }
    }
}

/// Core copy logic with injected backends, enabling deterministic unit tests
/// without touching real clipboards or terminal I/O.
fn copy_to_clipboard_with(
    text: &str,
    ssh_session: bool,
    wsl_session: bool,
    osc52_copy_fn: impl Fn(&str) -> Result<(), String>,
    arboard_copy_fn: impl Fn(&str) -> Result<Option<ClipboardLease>, String>,
    wsl_copy_fn: impl Fn(&str) -> Result<(), String>,
) -> Result<Option<ClipboardLease>, String> {
    if ssh_session {
        // Over SSH the native clipboard writes to the remote machine which is
        // useless. Use OSC 52, which travels through the SSH tunnel to the
        // local terminal emulator.
        return osc52_copy_fn(text).map(|()| None).map_err(|osc_err| {
            tracing::warn!("OSC 52 clipboard copy failed over SSH: {osc_err}");
            format!("OSC 52 clipboard copy failed over SSH: {osc_err}")
        });
    }

    match arboard_copy_fn(text) {
        Ok(lease) => Ok(lease),
        Err(native_err) => {
            if wsl_session {
                tracing::warn!(
                    "native clipboard copy failed: {native_err}, falling back to WSL PowerShell"
                );
                match wsl_copy_fn(text) {
                    Ok(()) => return Ok(None),
                    Err(wsl_err) => {
                        tracing::warn!(
                            "WSL PowerShell clipboard copy failed: {wsl_err}, falling back to OSC 52"
                        );
                        return osc52_copy_fn(text).map(|()| None).map_err(|osc_err| {
                            format!(
                                "native clipboard: {native_err}; WSL fallback: {wsl_err}; OSC 52 fallback: {osc_err}"
                            )
                        });
                    }
                }
            }
            tracing::warn!("native clipboard copy failed: {native_err}, falling back to OSC 52");
            osc52_copy_fn(text).map(|()| None).map_err(|osc_err| {
                format!("native clipboard: {native_err}; OSC 52 fallback: {osc_err}")
            })
        }
    }
}

/// Detect whether the current process is running inside an SSH session.
fn is_ssh_session() -> bool {
    std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some()
}

#[cfg(target_os = "linux")]
fn is_wsl_session() -> bool {
    is_probably_wsl()
}

#[cfg(not(target_os = "linux"))]
fn is_wsl_session() -> bool {
    false
}

/// Heuristic WSL detection — `/proc/version` contains "microsoft" on WSL1/2.
#[cfg(target_os = "linux")]
fn is_probably_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|s| s.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

/// Run arboard with stderr suppressed.
///
/// On macOS, `arboard::Clipboard::new()` initializes `NSPasteboard` which
/// triggers `os_log` / `NSLog` output on stderr. Because the TUI owns the
/// terminal, that stray output corrupts the display. We temporarily redirect
/// fd 2 to `/dev/null` around the call to keep the screen clean.
#[cfg(all(not(target_os = "android"), not(target_os = "linux")))]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    #[cfg(target_os = "macos")]
    let _stderr_lock = STDERR_SUPPRESSION_MUTEX
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .map_err(|_| "stderr suppression lock poisoned".to_string())?;
    let _guard = SuppressStderr::new();
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("failed to set clipboard text: {e}"))?;
    Ok(None)
}

/// Run arboard with stderr suppressed.
///
/// On Linux/X11 and some Wayland setups, clipboard contents are served by the
/// process that last wrote them. Keep the `Clipboard` alive so the copied text
/// remains pasteable while the TUI is running.
#[cfg(target_os = "linux")]
fn arboard_copy(text: &str) -> Result<Option<ClipboardLease>, String> {
    let _guard = SuppressStderr::new();
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("clipboard unavailable: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| format!("failed to set clipboard text: {e}"))?;
    Ok(Some(ClipboardLease::native_linux(clipboard)))
}

#[cfg(target_os = "android")]
fn arboard_copy(_text: &str) -> Result<Option<ClipboardLease>, String> {
    Err("native clipboard unavailable on Android".to_string())
}

/// Copy text into the Windows clipboard from a WSL process.
#[cfg(target_os = "linux")]
fn wsl_clipboard_copy(text: &str) -> Result<(), String> {
    let mut child = std::process::Command::new("powershell.exe")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .args([
            "-NoProfile",
            "-Command",
            "[Console]::InputEncoding = [System.Text.Encoding]::UTF8; $ErrorActionPreference = 'Stop'; $text = [Console]::In.ReadToEnd(); Set-Clipboard -Value $text",
        ])
        .spawn()
        .map_err(|e| format!("failed to spawn powershell.exe: {e}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("failed to open powershell.exe stdin".to_string());
    };

    if let Err(err) = stdin.write_all(text.as_bytes()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("failed to write to powershell.exe: {err}"));
    }

    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|e| format!("failed to wait for powershell.exe: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            let status = output.status;
            Err(format!("powershell.exe exited with status {status}"))
        } else {
            Err(format!("powershell.exe failed: {stderr}"))
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn wsl_clipboard_copy(_text: &str) -> Result<(), String> {
    Err("WSL clipboard fallback unavailable on this platform".to_string())
}

/// RAII guard that redirects stderr (fd 2) to `/dev/null` on creation and
/// restores the original fd on drop.
#[cfg(target_os = "macos")]
struct SuppressStderr {
    saved_fd: Option<libc::c_int>,
}

#[cfg(target_os = "macos")]
impl SuppressStderr {
    fn new() -> Self {
        // SAFETY: all libc calls are plain fd ops with documented semantics;
        // failures degrade to a no-op guard so stderr is unchanged.
        unsafe {
            let saved = libc::dup(2);
            if saved < 0 {
                return Self { saved_fd: None };
            }
            let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
            if devnull < 0 {
                libc::close(saved);
                return Self { saved_fd: None };
            }
            if libc::dup2(devnull, 2) < 0 {
                libc::close(saved);
                libc::close(devnull);
                return Self { saved_fd: None };
            }
            libc::close(devnull);
            Self {
                saved_fd: Some(saved),
            }
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for SuppressStderr {
    fn drop(&mut self) {
        if let Some(saved) = self.saved_fd {
            // SAFETY: `saved` was returned by `libc::dup` and hasn't been
            // closed elsewhere.
            unsafe {
                libc::dup2(saved, 2);
                libc::close(saved);
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
struct SuppressStderr;

#[cfg(not(target_os = "macos"))]
impl SuppressStderr {
    fn new() -> Self {
        Self
    }
}

/// Write text to the clipboard via the OSC 52 terminal escape sequence.
fn osc52_copy(text: &str) -> Result<(), String> {
    let sequence = osc52_sequence(text, std::env::var_os("TMUX").is_some())?;
    #[cfg(unix)]
    {
        match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            Ok(tty) => match write_osc52_to_writer(tty, &sequence) {
                Ok(()) => return Ok(()),
                Err(err) => tracing::debug!(
                    "failed to write OSC 52 to /dev/tty: {err}; falling back to stdout"
                ),
            },
            Err(err) => {
                tracing::debug!("failed to open /dev/tty for OSC 52: {err}; falling back to stdout")
            }
        }
    }

    write_osc52_to_writer(std::io::stdout().lock(), &sequence)
}

fn write_osc52_to_writer(mut writer: impl Write, sequence: &str) -> Result<(), String> {
    writer
        .write_all(sequence.as_bytes())
        .map_err(|e| format!("failed to write OSC 52: {e}"))?;
    writer
        .flush()
        .map_err(|e| format!("failed to flush OSC 52: {e}"))
}

fn osc52_sequence(text: &str, tmux: bool) -> Result<String, String> {
    let raw_bytes = text.len();
    if raw_bytes > OSC52_MAX_RAW_BYTES {
        return Err(format!(
            "OSC 52 payload too large ({raw_bytes} bytes; max {OSC52_MAX_RAW_BYTES})"
        ));
    }

    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    if tmux {
        Ok(format!("\x1bPtmux;\x1b\x1b]52;c;{encoded}\x07\x1b\\"))
    } else {
        Ok(format!("\x1b]52;c;{encoded}\x07"))
    }
}

#[cfg(test)]
#[path = "clipboard_copy.test.rs"]
mod tests;
