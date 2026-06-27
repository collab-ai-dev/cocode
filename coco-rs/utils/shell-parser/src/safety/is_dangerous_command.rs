//! Dangerous command detection.
//!
//! Ported from codex-rs/shell-command/src/command_safety/is_dangerous_command.rs.

use std::path::Path;

use crate::summary::shell_invoke::parse_shell_lc_plain_commands;

pub fn command_might_be_dangerous(command: &[String]) -> bool {
    if is_dangerous_to_call_with_exec(command) {
        return true;
    }

    // Support `bash -lc "<script>"` where any part of the script might contain
    // a dangerous command.
    if let Some(all_commands) = parse_shell_lc_plain_commands(command)
        && all_commands
            .iter()
            .any(|cmd| is_dangerous_to_call_with_exec(cmd))
    {
        return true;
    }

    false
}

fn is_git_global_option_with_value(arg: &str) -> bool {
    matches!(
        arg,
        "-C" | "-c"
            | "--config-env"
            | "--exec-path"
            | "--git-dir"
            | "--namespace"
            | "--super-prefix"
            | "--work-tree"
    )
}

fn is_git_global_option_with_inline_value(arg: &str) -> bool {
    (arg.starts_with("--config-env=")
        || arg.starts_with("--exec-path=")
        || arg.starts_with("--git-dir=")
        || arg.starts_with("--namespace=")
        || arg.starts_with("--super-prefix=")
        || arg.starts_with("--work-tree="))
        || ((arg.starts_with("-C") || arg.starts_with("-c")) && arg.len() > 2)
}

pub(crate) fn executable_name_lookup_key(raw: &str) -> Option<String> {
    Path::new(raw)
        .file_name()
        .and_then(|name| name.to_str())
        .map(std::borrow::ToOwned::to_owned)
}

/// Find the first matching git subcommand, skipping known global options that
/// may appear before it (e.g., `-C`, `-c`, `--git-dir`).
pub(crate) fn find_git_subcommand<'a>(
    command: &'a [String],
    subcommands: &[&str],
) -> Option<(usize, &'a str)> {
    let cmd0 = command.first().map(String::as_str)?;
    if executable_name_lookup_key(cmd0).as_deref() != Some("git") {
        return None;
    }

    let mut skip_next = false;
    for (idx, arg) in command.iter().enumerate().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }

        let arg = arg.as_str();

        if is_git_global_option_with_inline_value(arg) {
            continue;
        }

        if is_git_global_option_with_value(arg) {
            skip_next = true;
            continue;
        }

        if arg == "--" || arg.starts_with('-') {
            continue;
        }

        if subcommands.contains(&arg) {
            return Some((idx, arg));
        }

        // In git, the first non-option token is the subcommand. If it isn't
        // one of the subcommands we're looking for, we must stop scanning to
        // avoid misclassifying later positional args (e.g., branch names).
        return None;
    }

    None
}

fn is_dangerous_to_call_with_exec(command: &[String]) -> bool {
    let cmd0 = command.first().map(String::as_str);

    match cmd0 {
        Some("rm") => matches!(command.get(1).map(String::as_str), Some("-f" | "-rf")),
        // for sudo <cmd> simply do the check for <cmd>
        Some("sudo") => is_dangerous_to_call_with_exec(&command[1..]),
        _ => false,
    }
}

#[cfg(test)]
#[path = "is_dangerous_command.test.rs"]
mod tests;
