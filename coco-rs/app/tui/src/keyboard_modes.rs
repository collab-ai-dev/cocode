//! Terminal keyboard-enhancement (kitty protocol) setup and teardown.
//!
//! Without the enhancement push, most terminals send a plain CR for
//! Shift+Enter / Ctrl+Enter — crossterm reports an unmodified Enter and the
//! default `chat:insertNewline` binding can never fire. Pushing
//! `DISAMBIGUATE_ESCAPE_CODES | REPORT_EVENT_TYPES | REPORT_ALTERNATE_KEYS`
//! makes modified keys distinguishable on kitty-capable terminals; terminals
//! without support ignore the sequence. The run loop already filters
//! `KeyEventKind::Press`, so `REPORT_EVENT_TYPES` cannot double-fire keys.
//!
//! Teardown has two strengths (ported from codex-rs `keyboard_modes.rs`):
//! a stack-respecting pop for suspend/resume, and a hard `CSI < u` reset on
//! process exit so the parent shell never inherits enhanced reporting if a
//! terminal missed a pop.

use std::fmt;
use std::io::stdout;

use coco_config::env::EnvKey;
use crossterm::Command;
use crossterm::event::KeyboardEnhancementFlags;
use crossterm::event::PopKeyboardEnhancementFlags;
use crossterm::event::PushKeyboardEnhancementFlags;
use crossterm::execute;

fn keyboard_enhancement_disabled() -> bool {
    let override_env = coco_config::env::env_truthy_opt(EnvKey::CocoTuiKeyboardEnhancementDisable);
    let is_wsl = running_in_wsl();
    let is_vscode_terminal = is_wsl && running_in_vscode_terminal();
    keyboard_enhancement_disabled_for(override_env, is_wsl, is_vscode_terminal)
}

fn keyboard_enhancement_disabled_for(
    override_env: Option<bool>,
    is_wsl: bool,
    is_vscode_terminal: bool,
) -> bool {
    if let Some(disabled) = override_env {
        return disabled;
    }
    // VS Code running a WSL shell can hide TERM_PROGRAM from the Linux
    // process environment, so `running_in_vscode_terminal` also probes the
    // Windows-side environment through WSL interop.
    is_wsl && is_vscode_terminal
}

fn running_in_wsl() -> bool {
    #[cfg(target_os = "linux")]
    {
        // `/proc/version` contains "microsoft" on WSL1/2 (same heuristic as
        // tui-ui's clipboard WSL fallback).
        std::fs::read_to_string("/proc/version")
            .map(|s| s.to_lowercase().contains("microsoft"))
            .unwrap_or(false)
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn running_in_vscode_terminal() -> bool {
    vscode_terminal_detected(
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        windows_term_program().as_deref(),
    )
}

fn vscode_terminal_detected(
    linux_term_program: Option<&str>,
    windows_term_program: Option<&str>,
) -> bool {
    term_program_is_vscode(linux_term_program) || term_program_is_vscode(windows_term_program)
}

fn term_program_is_vscode(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("vscode"))
}

fn windows_term_program() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        static WINDOWS_TERM_PROGRAM: std::sync::OnceLock<Option<String>> =
            std::sync::OnceLock::new();
        WINDOWS_TERM_PROGRAM
            .get_or_init(read_windows_term_program)
            .clone()
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_windows_term_program() -> Option<String> {
    if !running_in_wsl() {
        return None;
    }
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/s", "/c", "set TERM_PROGRAM"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.trim_end_matches('\r')
                .strip_prefix("TERM_PROGRAM=")
                .map(str::to_string)
        })
        .filter(|value| !value.trim().is_empty())
}

/// Push the kitty keyboard-enhancement flags (and, under a csi-u-confirmed
/// tmux, xterm modifyOtherKeys mode 2). Errors are ignored: terminals without
/// support simply discard the sequences.
pub(crate) fn enable_keyboard_enhancement() {
    if keyboard_enhancement_disabled() {
        return;
    }

    let _ = execute!(
        stdout(),
        DisableModifyOtherKeys,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    );

    if tmux_should_enable_modify_other_keys() {
        let _ = execute!(stdout(), EnableModifyOtherKeys);
    }
}

fn running_in_tmux_session() -> bool {
    tmux_session_detected(
        std::env::var("TMUX").ok().as_deref(),
        std::env::var("TMUX_PANE").ok().as_deref(),
    )
}

fn tmux_session_detected(tmux: Option<&str>, tmux_pane: Option<&str>) -> bool {
    tmux.is_some() || tmux_pane.is_some()
}

fn tmux_should_enable_modify_other_keys() -> bool {
    tmux_should_enable_modify_other_keys_for(
        running_in_tmux_session(),
        read_tmux_extended_keys_format().as_deref(),
    )
}

fn tmux_should_enable_modify_other_keys_for(
    running_in_tmux_session: bool,
    extended_keys_format: Option<&str>,
) -> bool {
    // Only request mode 2 when tmux confirms csi-u formatting. Older tmux
    // versions do not expose this option and may emit xterm-style sequences,
    // which crossterm does not parse consistently for modified keys.
    running_in_tmux_session && matches!(extended_keys_format, Some("csi-u"))
}

fn read_tmux_extended_keys_format() -> Option<String> {
    for args in [
        ["display-message", "-p", "#{extended-keys-format}"],
        ["show-options", "-gqv", "extended-keys-format"],
    ] {
        let output = std::process::Command::new("tmux")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;

        if !output.status.success() {
            continue;
        }

        if let Some(value) = String::from_utf8(output.stdout)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }

    None
}

/// Stack-respecting teardown for suspend/external-process windows — the
/// matching `enable_keyboard_enhancement` re-arms on resume.
pub(crate) fn restore_keyboard_enhancement_stack() {
    let _ = execute!(
        stdout(),
        PopKeyboardEnhancementFlags,
        DisableModifyOtherKeys
    );
}

/// Hard teardown for process exit/panic: pop, then `CSI < u` (clears every
/// pushed level) so the parent shell cannot inherit enhanced reporting.
pub(crate) fn reset_keyboard_reporting_after_exit() {
    let _ = execute!(
        stdout(),
        PopKeyboardEnhancementFlags,
        ResetKeyboardEnhancementFlags,
        DisableModifyOtherKeys
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResetKeyboardEnhancementFlags;

impl Command for ResetKeyboardEnhancementFlags {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[<u")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "keyboard enhancement reset is not implemented for the legacy Windows API",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EnableModifyOtherKeys;

impl Command for EnableModifyOtherKeys {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[>4;2m")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "modifyOtherKeys enable is not implemented for the legacy Windows API",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DisableModifyOtherKeys;

impl Command for DisableModifyOtherKeys {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[>4;0m")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "modifyOtherKeys reset is not implemented for the legacy Windows API",
        ))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "keyboard_modes.test.rs"]
mod tests;
