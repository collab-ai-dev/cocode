//! Command result interpretation based on exit-code semantics.
//!
//! Many commands use non-zero exit codes for expected outcomes (`grep` returns
//! 1 for "no match", `diff` returns 1 for "files differ"). This interprets the
//! exit code in the context of the command that produced it.

/// Interpretation of a command's exit code.
///
/// `is_error` drives the model-facing `Exit code N` annotation, which is
/// appended ONLY when `is_error` is true. `message` is the human-friendly
/// explanation surfaced to the TUI only — never to the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResultInterpretation {
    pub is_error: bool,
    pub message: Option<String>,
}

/// Interpret an exit code based on the command that produced it.
///
/// The base command is the first word of the LAST compound/pipeline segment —
/// the segment that actually determines the exit code.
pub fn interpret_command_result(command: &str, exit_code: i32) -> CommandResultInterpretation {
    match heuristic_base_command(command).as_str() {
        // grep / rg: 0 = match, 1 = no match, 2+ = error.
        "grep" | "rg" => expected_below_two(exit_code, "No matches found"),
        // find: 0 = ok, 1 = some dirs inaccessible, 2+ = error.
        "find" => expected_below_two(exit_code, "Some directories were inaccessible"),
        // diff: 0 = identical, 1 = differ, 2+ = error.
        "diff" => expected_below_two(exit_code, "Files differ"),
        // test / [: 0 = true, 1 = false, 2+ = error.
        "test" | "[" => expected_below_two(exit_code, "Condition is false"),
        // Default: any non-zero exit is an error.
        _ => CommandResultInterpretation {
            is_error: exit_code != 0,
            message: (exit_code != 0).then(|| format!("Command failed with exit code {exit_code}")),
        },
    }
}

/// Shared semantic for commands where exit 1 is an expected, non-error outcome
/// and only `>= 2` is a genuine error (grep/rg/find/diff/test).
fn expected_below_two(exit_code: i32, explanation: &str) -> CommandResultInterpretation {
    CommandResultInterpretation {
        is_error: exit_code >= 2,
        message: (exit_code == 1).then(|| explanation.to_string()),
    }
}

/// First word of the LAST compound/pipeline segment — the command that sets the
/// overall exit code. Splits on compound operators then takes the first
/// whitespace token of the last segment. (Heuristic — never used for security
/// decisions, only exit-code interpretation.)
fn heuristic_base_command(command: &str) -> String {
    let segments = crate::bash_permissions::split_compound_command(command);
    let last = segments.last().map(String::as_str).unwrap_or(command);
    last.split_whitespace().next().unwrap_or("").to_string()
}

#[cfg(test)]
#[path = "semantics.test.rs"]
mod tests;
