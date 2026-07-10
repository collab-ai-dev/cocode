//! The `rtk rewrite "<cmd>"` subprocess call: spawn, timeout, exit-code map,
//! and the rr-rtk binary-name fixup.
//!
//! Exit-code contract (design §1.4). coco runs its own permission engine
//! before the rewrite, so the verdict half of rtk's protocol is irrelevant:
//! **exit 0 or 3 with non-empty stdout ⇒ use the rewrite; anything else ⇒
//! passthrough.**
//!
//! | Exit | Meaning                              | Outcome            |
//! |------|--------------------------------------|--------------------|
//! | 0    | rewrite, host may auto-allow         | `Rewritten`        |
//! | 3    | rewrite, host should still prompt    | `Rewritten`        |
//! | 1    | no rtk equivalent                    | `NoEquivalent`     |
//! | 2    | a host deny rule matched             | `HostDeny`         |
//! | else | unknown                              | `NoEquivalent`     |

use std::time::Duration;

use tokio::process::Command;

#[cfg(test)]
#[path = "rewrite.test.rs"]
mod tests;

use super::PassthroughReason;
use super::RewriteOutcome;
use super::detect::RtkBinary;
use super::detect::RtkFlavor;

/// Run `rtk rewrite <command>` and map the result to a [`RewriteOutcome`].
///
/// The command is passed argv-style (`Command::new(path).arg("rewrite")
/// .arg(command)`) — never through a shell — so the model's command string is
/// not re-interpreted on the way in.
pub(super) async fn run_rewrite(
    binary: &RtkBinary,
    command: &str,
    timeout_ms: i64,
) -> RewriteOutcome {
    let mut cmd = Command::new(&binary.path);
    cmd.arg("rewrite")
        .arg(command)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        // On timeout the `output()` future is dropped, dropping the child;
        // kill_on_drop reaps it so a hung rewriter can't leak a process.
        .kill_on_drop(true);

    let timeout = Duration::from_millis(timeout_ms.max(1) as u64);
    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => return RewriteOutcome::Passthrough(PassthroughReason::SpawnError),
        Err(_) => return RewriteOutcome::Passthrough(PassthroughReason::Timeout),
    };

    map_exit_result(output.status.code(), &output.stdout, binary.flavor)
}

/// Map an `rtk rewrite` exit code + stdout to an outcome. Pure — the spawn /
/// timeout / spawn-error paths live in [`run_rewrite`]; this is the classifier
/// they share and the one the tests exercise directly.
fn map_exit_result(code: Option<i32>, stdout: &[u8], flavor: RtkFlavor) -> RewriteOutcome {
    match code {
        Some(0) | Some(3) => {
            let rewritten = String::from_utf8_lossy(stdout);
            let rewritten = rewritten.trim();
            if rewritten.is_empty() {
                return RewriteOutcome::Passthrough(PassthroughReason::NoEquivalent);
            }
            map_flavor(flavor, rewritten)
        }
        Some(1) => RewriteOutcome::Passthrough(PassthroughReason::NoEquivalent),
        Some(2) => RewriteOutcome::Passthrough(PassthroughReason::HostDeny),
        // Unknown exit code / killed by signal — treat as "no usable rewrite".
        _ => RewriteOutcome::Passthrough(PassthroughReason::NoEquivalent),
    }
}

/// A `Rtk`-flavored binary emits executable `rtk …` prefixes verbatim. An
/// `RrRtk` binary emits the same literal `rtk ` prefixes but is installed as
/// `rr-rtk`, so the rewrite must be fixed up before it can execute (§4.5).
fn map_flavor(flavor: RtkFlavor, rewritten: &str) -> RewriteOutcome {
    match flavor {
        RtkFlavor::Rtk => RewriteOutcome::Rewritten(rewritten.to_string()),
        RtkFlavor::RrRtk => match fixup_rr_rtk_prefixes(rewritten) {
            Some(fixed) => RewriteOutcome::Rewritten(fixed),
            None => RewriteOutcome::Passthrough(PassthroughReason::ShapeMismatch),
        },
    }
}

/// Rewrite each top-level segment whose first token is exactly `rtk` to
/// `rr-rtk`. Prefix insertion at segment heads is the engine's *only*
/// transform, so this is a complete inverse.
///
/// Returns `None` (⇒ `ShapeMismatch`) when no segment carried an `rtk` prefix:
/// an `rtk rewrite` exit 0/3 asserts a rewrite happened, so a rewrite with no
/// `rtk` head is a shape we can't fully account for — safer to pass through.
///
/// Quote-aware and UTF-8-safe: it walks `char`s, tracks single/double quotes
/// **and backslash escapes**, and splits on top-level `&&` / `||` / `;` / `|` /
/// `&` and newlines (ASCII, always char boundaries).
///
/// Backslash handling matters for correctness: outside single quotes a `\`
/// escapes the next char, so `\"` inside a double-quoted argument does not close
/// the quote (without this, an odd number of escaped quotes desyncs the quote
/// state and a literal `rtk` inside a quoted argument would be rewritten).
/// Newlines are separators so a multi-line rewrite gets every segment head fixed
/// (else the second `rtk` would execute as a missing binary).
fn fixup_rr_rtk_prefixes(rewritten: &str) -> Option<String> {
    let chars: Vec<char> = rewritten.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(rewritten.len() + 8);
    let mut in_single = false;
    let mut in_double = false;
    let mut at_segment_start = true;
    let mut any_prefixed = false;
    let mut i = 0;

    while i < n {
        // At the head of a segment (outside quotes): skip leading whitespace,
        // then rewrite a bare `rtk` token to `rr-rtk`.
        if at_segment_start && !in_single && !in_double {
            while i < n && is_segment_whitespace(chars[i]) {
                out.push(chars[i]);
                i += 1;
            }
            if is_rtk_token(&chars, i) {
                out.push_str("rr-rtk");
                i += 3;
                any_prefixed = true;
            }
            at_segment_start = false;
            continue;
        }

        let c = chars[i];

        // Backslash escape (everywhere except inside single quotes, where it is
        // literal): the next char is consumed verbatim and can neither toggle a
        // quote nor act as a separator.
        if c == '\\' && !in_single {
            out.push(c);
            i += 1;
            if i < n {
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }

        if c == '\'' && !in_double {
            in_single = !in_single;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '"' && !in_single {
            in_double = !in_double;
            out.push(c);
            i += 1;
            continue;
        }

        if !in_single && !in_double {
            match c {
                ';' | '\n' | '\r' => {
                    out.push(c);
                    i += 1;
                    at_segment_start = true;
                    continue;
                }
                '&' => {
                    out.push(c);
                    i += 1;
                    if i < n && chars[i] == '&' {
                        out.push('&');
                        i += 1;
                    }
                    at_segment_start = true;
                    continue;
                }
                '|' => {
                    out.push(c);
                    i += 1;
                    if i < n && chars[i] == '|' {
                        out.push('|');
                        i += 1;
                    }
                    at_segment_start = true;
                    continue;
                }
                _ => {}
            }
        }

        out.push(c);
        i += 1;
    }

    any_prefixed.then_some(out)
}

/// Whitespace that can precede a segment head (also the token boundary set).
fn is_segment_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// True when `chars[i..]` is the token `rtk` bounded by segment whitespace or end.
fn is_rtk_token(chars: &[char], i: usize) -> bool {
    chars.get(i) == Some(&'r')
        && chars.get(i + 1) == Some(&'t')
        && chars.get(i + 2) == Some(&'k')
        && chars.get(i + 3).is_none_or(|&c| is_segment_whitespace(c))
}
