//! Shell command execution in skill prompts.
//!
//! Executes shell commands embedded in skill / slash-command markdown
//! content before sending to the model.
//!
//! Both paths support the same two marker syntaxes:
//! - Fenced bang block: ```` ```! command ``` ```` — whole block replaced
//!   with the command's stdout.
//! - Inline bang span: `` !`command` `` — replaced with stdout, but only
//!   when the `!` is preceded by start-of-line or whitespace (uses a
//!   positive lookbehind; the `regex` crate has none, so the guard is a
//!   manual char scan).
//!
//! Two code paths live here:
//!
//! 1. [`execute_shell_in_prompt`] — the legacy, handle-free path. Runs each
//!    marker directly through `sh -c` with NO permission check. Retained only
//!    for unit tests and call sites that have no [`BashToolHandle`] wired (the
//!    substitution is best-effort: a failing command becomes empty output).
//! 2. [`execute_shell_in_prompt_with_tool`] — the production path. Routes EACH
//!    command through the injected [`BashToolHandle`], which performs the real
//!    per-command permission check + Bash execution. A denied or failing
//!    command ABORTS the whole expansion (`Err`) — mirroring
//!    `MalformedCommandError`, which throws out of `Promise.all` and aborts
//!    `getPromptForCommand` with no partial substitution.

use std::time::Duration;

use async_trait::async_trait;

/// Default timeout for shell commands in skill prompts.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Callback seam letting `coco-skills` (and, via re-export,
/// `coco-commands`) route in-prompt shell commands through the real
/// `Bash` tool with a per-command permission check — WITHOUT either
/// crate depending on `coco-tools` / the tool runtime.
///
/// The trait lives here (the lowest crate both consumers can see:
/// `coco-commands` already depends on `coco-skills`) so the
/// substitution functions can take `&dyn BashToolHandle` as a
/// parameter. The concrete implementation lives in `app/cli`
/// (`SessionBashToolHandle`), built once the per-tool `ToolUseContext`
/// exists; it is injected into the command/skill handlers at session
/// bootstrap.
///
/// The handle folds both the permission check and the Bash execution into
/// one call.
#[async_trait]
pub trait BashToolHandle: Send + Sync {
    /// Permission-check then execute `command` through the Bash tool.
    ///
    /// `allowed_tools` are the skill frontmatter `allowed-tools`
    /// entries, surfaced to the permission evaluator as
    /// `alwaysAllowRules.command`. Slash commands pass an empty slice —
    /// only configured rules apply.
    ///
    /// Returns the formatted stdout/stderr block to substitute on
    /// success, or an error message on permission-deny / execution
    /// failure (the caller aborts the whole expansion).
    async fn execute_with_permissions(
        &self,
        command: &str,
        allowed_tools: &[String],
    ) -> Result<String, String>;
}

/// No-op [`BashToolHandle`] for contexts without a wired Bash runtime
/// (unit tests, early bootstrap). Returns empty output and never denies.
pub struct NoOpBashToolHandle;

#[async_trait]
impl BashToolHandle for NoOpBashToolHandle {
    async fn execute_with_permissions(
        &self,
        _command: &str,
        _allowed_tools: &[String],
    ) -> Result<String, String> {
        Ok(String::new())
    }
}

/// Execute shell commands embedded in skill prompt content.
///
/// Replaces fenced ```` ```! … ``` ```` blocks and inline `` !`…` `` spans
/// with their command stdout.
///
/// Returns the content with shell commands replaced by their output.
///
/// # Security
/// - MCP-sourced skills skip shell execution entirely (`skip_shell`).
///   MCP skills are remote and untrusted — their markdown body must never run
///   inline shell.
/// - Commands inherit the current working directory.
/// - Timeout prevents hanging commands.
pub async fn execute_shell_in_prompt(content: &str, skip_shell: bool) -> String {
    if skip_shell {
        return content.to_string();
    }

    // Fenced bang blocks run first, then inline spans. Each pass works on
    // the output of the previous so a command's stdout is never re-scanned
    // for further patterns (sequential here is equivalent for non-overlapping
    // matches and avoids re-executing stdout that happens to contain a
    // pattern).
    let after_blocks = replace_block_shell(content).await;
    replace_inline_shell(&after_blocks).await
}

/// Replace fenced bang blocks ```` ```! \n body \n``` ```` with their stdout.
///
/// The block pattern `/```!\s*\n?([\s\S]*?)\n?```/g`: a triple-backtick
/// immediately followed by `!`, optional whitespace then optional newline,
/// a lazy body, an optional trailing newline, and the closing fence.
async fn replace_block_shell(content: &str) -> String {
    const OPEN: &str = "```!";
    const FENCE: &str = "```";

    let mut result = String::with_capacity(content.len());
    let mut rest = content;

    while let Some(open_at) = rest.find(OPEN) {
        // Emit everything up to the opening fence verbatim.
        result.push_str(&rest[..open_at]);
        let after_open = &rest[open_at + OPEN.len()..];

        // Find the closing fence. If none, the opener is literal text.
        let Some(close_rel) = after_open.find(FENCE) else {
            result.push_str(&rest[open_at..]);
            return result;
        };
        let body_raw = &after_open[..close_rel];
        let after_close = &after_open[close_rel + FENCE.len()..];

        // Strips leading `\s*\n?` after `!` and a single trailing `\n`
        // before the closing fence. `trim()` on the captured body is the
        // load-bearing equivalent — the command string fed to the shell is
        // `match[1]?.trim()`.
        let command = body_raw.trim();
        if command.is_empty() {
            // Empty command: skips execution and leaves the match in
            // place (the `if (command)` guard is false). Preserve the
            // original fenced block verbatim.
            result.push_str(OPEN);
            result.push_str(body_raw);
            result.push_str(FENCE);
        } else {
            let output = run_shell_command(command).await;
            result.push_str(&output);
        }
        rest = after_close;
    }

    result.push_str(rest);
    result
}

/// Execute the two marker syntaxes through an injected
/// [`BashToolHandle`], substituting each command's output.
///
/// Markers:
/// - **Block:** ```` ```!\n<cmd>\n``` ```` — a fenced code block opened
///   with `` ```! ``.
/// - **Inline:** `` !`<cmd>` `` — a `!` (preceded by start-of-text or
///   whitespace) immediately followed by a backtick-delimited command.
///
/// Each command is routed through `handle.execute_with_permissions`,
/// which performs the real permission check and Bash execution. On any
/// `Err` (permission denied OR command failure) the WHOLE expansion is
/// aborted with that message — mirroring `MalformedCommandError`,
/// which throws out of the per-marker `Promise.all` so the caller
/// performs NO partial substitution.
///
/// The caller is responsible for the MCP skip: MCP-sourced skills must
/// not call this at all.
pub async fn execute_shell_in_prompt_with_tool(
    content: &str,
    handle: &dyn BashToolHandle,
    allowed_tools: &[String],
) -> Result<String, String> {
    let markers = scan_shell_markers(content);
    if markers.is_empty() {
        return Ok(content.to_string());
    }

    // Substitutes per marker using absolute byte spans from the scan,
    // rebuilding the string in one pass (markers are non-overlapping and
    // sorted by start offset).
    let mut out = String::with_capacity(content.len());
    let mut last = 0usize;
    for marker in &markers {
        let output = handle
            .execute_with_permissions(&marker.command, allowed_tools)
            .await?;
        out.push_str(&content[last..marker.start]);
        out.push_str(&output);
        last = marker.end;
    }
    out.push_str(&content[last..]);
    Ok(out)
}

/// A shell marker located in prompt content: its full byte span (the
/// region to replace) and the extracted, trimmed command.
struct ShellMarker {
    start: usize,
    end: usize,
    command: String,
}

/// Scan `content` for block (```` ```! ````) and inline (`` !`cmd` ``)
/// markers, returning them sorted by start offset and de-overlapped
/// (a later marker whose start falls inside an already-accepted span is
/// dropped). Block and inline matches are concatenated; an inline
/// `` !`…` `` inside a block body is already consumed by the block marker
/// so it must not match again.
fn scan_shell_markers(content: &str) -> Vec<ShellMarker> {
    let mut markers: Vec<ShellMarker> = Vec::new();
    scan_block_markers(content, &mut markers);
    scan_inline_markers(content, &mut markers);
    markers.sort_by_key(|m| m.start);

    // De-overlap: keep the first (earliest), drop any whose start is
    // within a kept span.
    let mut accepted: Vec<ShellMarker> = Vec::with_capacity(markers.len());
    let mut covered_until = 0usize;
    for m in markers {
        if m.start < covered_until {
            continue;
        }
        covered_until = m.end;
        accepted.push(m);
    }
    accepted
}

/// Locate ```` ```! ```` fenced blocks.
/// Pattern: `/```!\s*\n?([\s\S]*?)\n?```/g`: the opening fence is `` ```! ``
/// followed by optional whitespace and an optional newline; the body is
/// captured lazily up to the next `` ``` ``, with a trailing newline
/// trimmed from the body. Empty / whitespace-only commands are skipped
/// (the `if (command)` guard after `.trim()`).
fn scan_block_markers(content: &str, out: &mut Vec<ShellMarker>) {
    const OPEN: &str = "```!";
    const CLOSE: &str = "```";
    let bytes = content.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = content[search..].find(OPEN) {
        let start = search + rel;
        // Body begins after the opener + any inline whitespace + one
        // optional newline (`\s*\n?`). `\s` includes newlines, so a
        // run of whitespace before the body is consumed; the lazy body
        // then trims one leading newline implicitly via `\n?`.
        let mut body_start = start + OPEN.len();
        while body_start < bytes.len() && matches!(bytes[body_start], b' ' | b'\t' | b'\r') {
            body_start += 1;
        }
        if body_start < bytes.len() && bytes[body_start] == b'\n' {
            body_start += 1;
        }
        match content[body_start..].find(CLOSE) {
            Some(close_rel) => {
                let body_end = body_start + close_rel;
                let end = body_end + CLOSE.len();
                // Trim one trailing newline from the body (`\n?` before
                // the closing fence) then trim for the `if (command)` gate.
                let raw_body = content[body_start..body_end].trim_end_matches('\n');
                let command = raw_body.trim();
                if !command.is_empty() {
                    out.push(ShellMarker {
                        start,
                        end,
                        command: command.to_string(),
                    });
                }
                search = end;
            }
            None => break, // Unterminated fence — stop scanning blocks.
        }
    }
}

/// Locate `` !`cmd` `` inline markers.
/// Pattern: `/(?<=^|\s)!`([^`]+)`/gm` (gated by `text.includes('!`')`): a `!`
/// preceded by start-of-text or an ASCII/Unicode whitespace char,
/// immediately followed by a backtick, a non-backtick command, and a
/// closing backtick. Empty / whitespace-only commands are skipped.
fn scan_inline_markers(content: &str, out: &mut Vec<ShellMarker>) {
    if !content.contains("!`") {
        return;
    }
    let mut search = 0usize;
    while let Some(rel) = content[search..].find("!`") {
        let bang = search + rel;
        // Lookbehind: `!` must be at start-of-text or preceded by
        // whitespace (`(?<=^|\s)`).
        let preceded_ok = bang == 0
            || content[..bang]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let cmd_start = bang + 2; // past "!`"
        if !preceded_ok {
            search = cmd_start;
            continue;
        }
        match content[cmd_start..].find('`') {
            Some(close_rel) => {
                if close_rel == 0 {
                    // Empty command (`[^`]+` requires ≥1 char) — skip.
                    search = cmd_start;
                    continue;
                }
                let cmd_end = cmd_start + close_rel;
                let end = cmd_end + 1; // past closing backtick
                let command = content[cmd_start..cmd_end].trim();
                if !command.is_empty() {
                    out.push(ShellMarker {
                        start: bang,
                        end,
                        command: command.to_string(),
                    });
                }
                search = end;
            }
            None => break, // No closing backtick — stop scanning inline.
        }
    }
}

/// Replace inline `` !`command` `` spans with their stdout (handle-free path).
///
/// The inline pattern `/(?<=^|\s)!`([^`]+)`/gm`: a `!` preceded by
/// start-of-line or whitespace, then a backtick-delimited command. The Rust
/// `regex` crate has no lookbehind, so the word-boundary guard is a manual
/// char scan — rejects `` foo!`x` `` while accepting `` !`x` `` at line start
/// and after whitespace.
async fn replace_inline_shell(content: &str) -> String {
    // Cheap gate: 93% of skills have no inline bang span (gates the
    // expensive scan on `text.includes('!`')`).
    if !content.contains("!`") {
        return content.to_string();
    }

    let mut result = String::with_capacity(content.len());
    let mut idx = 0;

    while idx < content.len() {
        // Locate the next `!` followed immediately by a backtick.
        let Some(bang_rel) = content[idx..].find("!`") else {
            result.push_str(&content[idx..]);
            break;
        };
        let bang_at = idx + bang_rel;

        // Word-boundary guard: the char before `!` must be start-of-line
        // or whitespace. Lookbehind is `(?<=^|\s)`. `next_back()` is
        // `None` at start-of-line (`bang_at == 0`), which is a valid
        // boundary.
        let preceded_ok = content[..bang_at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);

        // The command runs from just after the backtick to the next
        // backtick. `[^`]+` requires a non-empty body.
        let body_start = bang_at + 2; // skip "!`"
        let close_rel = content[body_start..].find('`');

        match (preceded_ok, close_rel) {
            (true, Some(close_rel)) if close_rel > 0 => {
                let command = &content[body_start..body_start + close_rel];
                // Emit text before the span, then the command output.
                result.push_str(&content[idx..bang_at]);
                let output = run_shell_command(command).await;
                result.push_str(&output);
                // Resume after the closing backtick.
                idx = body_start + close_rel + 1;
            }
            _ => {
                // Not a valid span (bad boundary, no closing backtick, or
                // empty body). Emit up to and including the `!`, then resume
                // scanning just after it so the same position isn't
                // re-matched. `!` is ASCII, so `bang_at + 1` stays on a char
                // boundary.
                let consume_to = bang_at + 1;
                result.push_str(&content[idx..consume_to]);
                idx = consume_to;
            }
        }
    }

    result
}

/// Run a shell command and return its trimmed stdout. On non-zero exit,
/// spawn failure, or timeout this returns an empty string (the prompt
/// simply drops the failed substitution — see followups for full
/// permission/error wiring).
async fn run_shell_command(command: &str) -> String {
    let output = tokio::time::timeout(
        DEFAULT_TIMEOUT,
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output(),
    )
    .await;

    match output {
        Ok(Ok(out)) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        Ok(Ok(out)) => {
            tracing::warn!(
                "shell command in skill prompt failed (exit {}): {command}",
                out.status.code().unwrap_or(-1)
            );
            String::new()
        }
        Ok(Err(e)) => {
            tracing::warn!("failed to spawn shell command in skill prompt: {e}");
            String::new()
        }
        Err(_) => {
            tracing::warn!("shell command in skill prompt timed out: {command}");
            String::new()
        }
    }
}

#[cfg(test)]
#[path = "shell_exec.test.rs"]
mod tests;
