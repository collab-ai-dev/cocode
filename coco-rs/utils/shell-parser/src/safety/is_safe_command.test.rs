use super::*;
use pretty_assertions::assert_eq;

fn vec_str(args: &[&str]) -> Vec<String> {
    args.iter().map(ToString::to_string).collect()
}

#[test]
fn known_safe_examples() {
    assert!(is_known_safe_command(&vec_str(&["ls"])));
    assert!(is_known_safe_command(&vec_str(&["git", "status"])));
    assert!(is_known_safe_command(&vec_str(&["git", "branch"])));
    assert!(is_known_safe_command(&vec_str(&[
        "git",
        "branch",
        "--show-current"
    ])));
    assert!(is_known_safe_command(&vec_str(&["base64"])));
    assert!(is_known_safe_command(&vec_str(&[
        "sed", "-n", "1,5p", "file.txt"
    ])));
    assert!(is_known_safe_command(&vec_str(&[
        "nl",
        "-nrz",
        "Cargo.toml"
    ])));
    assert!(is_known_safe_command(&vec_str(&[
        "find", ".", "-name", "file.txt"
    ])));

    if cfg!(target_os = "linux") {
        assert!(is_known_safe_command(&vec_str(&["numfmt", "1000"])));
        assert!(is_known_safe_command(&vec_str(&["tac", "Cargo.toml"])));
    }
}

#[test]
fn git_branch_mutating_flags_are_not_safe() {
    assert!(!is_known_safe_command(&vec_str(&[
        "git", "branch", "-d", "feature"
    ])));
    assert!(!is_known_safe_command(&vec_str(&[
        "git",
        "branch",
        "new-branch"
    ])));
}

#[test]
fn git_branch_global_options_respect_safety_rules() {
    assert_eq!(
        is_known_safe_command(&vec_str(&["git", "-C", ".", "branch", "--show-current"])),
        true
    );
    assert_eq!(
        is_known_safe_command(&vec_str(&["git", "-C", ".", "branch", "-d", "feature"])),
        false
    );
    assert_eq!(
        is_known_safe_command(&vec_str(&["bash", "-lc", "git -C . branch -d feature"])),
        false
    );
}

#[test]
fn git_first_positional_is_the_subcommand() {
    assert!(!is_known_safe_command(&vec_str(&[
        "git", "checkout", "status",
    ])));
}

#[test]
fn git_output_and_config_override_flags_are_not_safe() {
    assert!(!is_known_safe_command(&vec_str(&[
        "git",
        "log",
        "--output=/tmp/git-log-out-test",
        "-n",
        "1",
    ])));
    assert!(!is_known_safe_command(&vec_str(&[
        "git",
        "-c",
        "core.pager=cat",
        "log",
        "-n",
        "1",
    ])));
}

#[test]
fn cargo_check_is_not_safe() {
    assert!(!is_known_safe_command(&vec_str(&["cargo", "check"])));
}

#[test]
fn zsh_lc_safe_command_sequence() {
    assert!(is_known_safe_command(&vec_str(&["zsh", "-lc", "ls"])));
}

#[test]
fn bash_lc_safe_examples() {
    assert!(is_known_safe_command(&vec_str(&["bash", "-lc", "ls"])));
    assert!(is_known_safe_command(&vec_str(&["bash", "-lc", "ls -1"])));
    assert!(is_known_safe_command(&vec_str(&[
        "bash",
        "-lc",
        "git status"
    ])));
}

#[test]
fn bash_lc_safe_examples_with_operators() {
    assert!(is_known_safe_command(&vec_str(&[
        "bash",
        "-lc",
        "ls && pwd"
    ])));
    assert!(is_known_safe_command(&vec_str(&[
        "bash",
        "-lc",
        "ls | wc -l"
    ])));
}

#[test]
fn bash_lc_unsafe_examples() {
    assert!(!is_known_safe_command(&vec_str(&[
        "bash",
        "-lc",
        "find . -name file.txt -delete"
    ])));
    assert!(!is_known_safe_command(&vec_str(&[
        "bash",
        "-lc",
        "ls && rm -rf /"
    ])));
}

#[test]
fn base64_output_options_are_unsafe() {
    for args in [
        vec_str(&["base64", "-o", "out.bin"]),
        vec_str(&["base64", "--output", "out.bin"]),
        vec_str(&["base64", "--output=out.bin"]),
    ] {
        assert!(
            !is_known_safe_command(&args),
            "expected {args:?} to be considered unsafe due to output option"
        );
    }
}

#[test]
fn ripgrep_rules() {
    assert!(is_known_safe_command(&vec_str(&["rg", "Cargo.toml", "-n"])));
    for args in [
        vec_str(&["rg", "--search-zip", "files"]),
        vec_str(&["rg", "-z", "files"]),
        vec_str(&["rg", "--pre", "pwned", "files"]),
        vec_str(&["rg", "--pre=pwned", "files"]),
    ] {
        assert!(
            !is_known_safe_command(&args),
            "expected {args:?} to be considered unsafe"
        );
    }
}

#[test]
fn find_unsafe_options() {
    for args in [
        vec_str(&["find", ".", "-name", "file.txt", "-exec", "rm", "{}", ";"]),
        vec_str(&["find", ".", "-delete", "-name", "file.txt"]),
    ] {
        assert!(
            !is_known_safe_command(&args),
            "expected {args:?} to be unsafe"
        );
    }
}
