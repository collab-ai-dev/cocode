use pretty_assertions::assert_eq;

use super::*;

fn err(message: &str) -> CommandResultInterpretation {
    CommandResultInterpretation {
        is_error: true,
        message: Some(message.to_string()),
    }
}

fn expected(message: &str) -> CommandResultInterpretation {
    CommandResultInterpretation {
        is_error: false,
        message: Some(message.to_string()),
    }
}

fn ok() -> CommandResultInterpretation {
    CommandResultInterpretation {
        is_error: false,
        message: None,
    }
}

#[test]
fn test_exit_code_zero_is_success() {
    assert_eq!(interpret_command_result("ls", 0), ok());
    assert_eq!(interpret_command_result("grep foo bar.txt", 0), ok());
}

// ── grep / rg: exit 1 = no match (NOT an error), >= 2 = error ──

#[test]
fn test_grep_no_match_is_not_error() {
    assert_eq!(
        interpret_command_result("grep pattern file.txt", 1),
        expected("No matches found"),
    );
    assert_eq!(
        interpret_command_result("rg pattern", 1),
        expected("No matches found")
    );
}

#[test]
fn test_grep_error_is_error() {
    // >= 2 is a genuine error and carries no friendly message.
    assert_eq!(
        interpret_command_result("grep pattern file.txt", 2),
        CommandResultInterpretation {
            is_error: true,
            message: None,
        },
    );
}

// ── find: exit 1 = some dirs inaccessible (NOT an error) ──

#[test]
fn test_find_partial_is_not_error() {
    assert_eq!(
        interpret_command_result("find . -name foo", 1),
        expected("Some directories were inaccessible"),
    );
    assert!(interpret_command_result("find . -name foo", 2).is_error);
}

// ── diff: exit 1 = files differ (NOT an error) ──

#[test]
fn test_diff_files_differ_is_not_error() {
    assert_eq!(
        interpret_command_result("diff file1 file2", 1),
        expected("Files differ"),
    );
    assert!(interpret_command_result("diff file1 file2", 2).is_error);
}

// ── test / [ : exit 1 = condition false (NOT an error) ──

#[test]
fn test_test_false_is_not_error() {
    assert_eq!(
        interpret_command_result("test -f missing.txt", 1),
        expected("Condition is false"),
    );
    assert_eq!(
        interpret_command_result("[ -d /nonexistent ]", 1),
        expected("Condition is false"),
    );
}

// ── DEFAULT semantic: anything else, any non-zero is an error ──

#[test]
fn test_default_nonzero_is_error() {
    assert_eq!(
        interpret_command_result("mycommand --flag", 1),
        err("Command failed with exit code 1"),
    );
    // `git diff --exit-code` exit 1: base is `git`, NOT special → error.
    assert!(interpret_command_result("git diff --exit-code", 1).is_error);
    // curl/timeout are not special in TS either.
    assert!(interpret_command_result("curl -f http://example.com/404", 22).is_error);
    assert!(interpret_command_result("timeout 5 sleep 10", 124).is_error);
}

// ── base command = LAST pipeline segment ──

#[test]
fn test_base_command_is_last_pipeline_segment() {
    // The exit code is the LAST command's: `... | grep z` exit 1 = grep no-match.
    assert_eq!(
        interpret_command_result("cat a.txt | grep z", 1),
        expected("No matches found"),
    );
    // Conversely, a leading grep followed by another command keys off the last.
    assert!(interpret_command_result("grep z a.txt | mycommand", 1).is_error);
}
