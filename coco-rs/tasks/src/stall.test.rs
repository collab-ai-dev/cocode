use super::*;

#[test]
fn yes_no_prompt_matches() {
    assert!(matches_interactive_prompt("Continue? (y/N)"));
    assert!(matches_interactive_prompt("Are you sure? [y/n]"));
    assert!(matches_interactive_prompt("(yes/no)"));
}

#[test]
fn password_prompts_match() {
    assert!(matches_interactive_prompt("Password:"));
    assert!(matches_interactive_prompt("Enter passphrase for ssh key:"));
    assert!(matches_interactive_prompt("[sudo] password for user:"));
}

#[test]
fn directed_questions_match() {
    assert!(matches_interactive_prompt("Do you want to continue?"));
    assert!(matches_interactive_prompt("Would you like to install?"));
    assert!(matches_interactive_prompt("Shall I overwrite?"));
    assert!(matches_interactive_prompt("Are you sure?"));
    assert!(matches_interactive_prompt("Ready to deploy?"));
}

#[test]
fn directed_question_without_question_mark_does_not_match() {
    // Requires both the directive AND the trailing `?`.
    assert!(!matches_interactive_prompt(
        "Do you want to continue please"
    ));
}

#[test]
fn action_prompts_match() {
    assert!(matches_interactive_prompt("Continue?"));
    assert!(matches_interactive_prompt("Overwrite?"));
    assert!(matches_interactive_prompt("Proceed?"));
}

#[test]
fn press_key_prompts_match() {
    assert!(matches_interactive_prompt("Press any key to continue"));
    assert!(matches_interactive_prompt("Press Enter to exit"));
}

#[test]
fn only_last_line_is_checked() {
    let tail = "Some earlier text mentioning Continue?\n\nfinal line";
    // Earlier line has a prompt-like marker but final line doesn't.
    assert!(!matches_interactive_prompt(tail));
}

#[test]
fn trailing_whitespace_tolerated() {
    assert!(matches_interactive_prompt("Continue?\n"));
    assert!(matches_interactive_prompt("Continue?    "));
    assert!(matches_interactive_prompt("Continue? \n   \n  "));
}

#[test]
fn empty_tail_does_not_match() {
    assert!(!matches_interactive_prompt(""));
    assert!(!matches_interactive_prompt("   \n  \n"));
}

#[test]
fn normal_output_does_not_match() {
    assert!(!matches_interactive_prompt("Build succeeded in 12.3s"));
    assert!(!matches_interactive_prompt("error: file not found"));
    assert!(!matches_interactive_prompt("$ ls -la"));
}
