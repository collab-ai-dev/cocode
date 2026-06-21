//! Wrapper command stripping for security analysis.
//!
//! Safe wrapper commands (`time`, `nohup`, `timeout`, `nice`, `env`, `stdbuf`)
//! are stripped before running security analyzers so that the *inner* command
//! is what gets analyzed.
//!
//! All strippers are **fail-closed**: unknown flags or suspicious patterns cause
//! the function to return `None`, which means the original command is analyzed
//! as-is (no stripping applied).

use once_cell::sync::Lazy;
use regex::Regex;

/// Strip safe wrapper commands from the front of an argv array.
///
/// Returns `Some(inner_args)` if a wrapper was stripped, `None` if the first
/// command is not a recognized wrapper or the wrapper syntax is suspicious.
///
/// This function strips iteratively — nested wrappers like `time nohup cmd`
/// are handled by repeated calls.
pub fn strip_wrappers(args: &[String]) -> Option<Vec<String>> {
    let first = args.first()?.as_str();
    match first {
        "time" => strip_time(args),
        "nohup" => strip_nohup(args),
        "timeout" => strip_timeout(args),
        "nice" => strip_nice(args),
        "env" => strip_env(args),
        "stdbuf" => strip_stdbuf(args),
        _ => None,
    }
}

/// Iteratively strip all wrapper layers until no more can be removed.
///
/// Returns `None` if no wrappers were stripped at all.
pub fn strip_all_wrappers(args: &[String]) -> Option<Vec<String>> {
    let mut current = strip_wrappers(args)?;
    loop {
        match strip_wrappers(&current) {
            Some(inner) => current = inner,
            None => return Some(current),
        }
    }
}

// -- Individual wrapper strippers --

/// `time cmd...` — simply drop `time`.
fn strip_time(args: &[String]) -> Option<Vec<String>> {
    if args.len() < 2 {
        return None;
    }
    Some(args[1..].to_vec())
}

/// `nohup cmd...` — simply drop `nohup`.
fn strip_nohup(args: &[String]) -> Option<Vec<String>> {
    if args.len() < 2 {
        return None;
    }
    Some(args[1..].to_vec())
}

/// `timeout [FLAGS] DURATION cmd...`
///
/// Known flags (no value): `--foreground`, `--preserve-status`, `--verbose`, `-v`
/// Known flags (value): `--kill-after=N`, `--signal=SIG`, `-k DURATION`, `-s SIGNAL`
fn strip_timeout(args: &[String]) -> Option<Vec<String>> {
    #[allow(clippy::expect_used)]
    static DURATION_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\d+(?:\.\d+)?[smhd]?$").expect("valid regex"));

    let mut i = 1; // skip "timeout"
    let len = args.len();

    while i < len {
        let arg = args[i].as_str();

        // No-value long flags
        if matches!(arg, "--foreground" | "--preserve-status" | "--verbose") {
            i += 1;
            continue;
        }
        // Short no-value flag
        if arg == "-v" {
            i += 1;
            continue;
        }
        // Value-taking long flags (fused with =)
        if (arg.starts_with("--kill-after=") || arg.starts_with("--signal="))
            && arg.split('=').nth(1).is_some_and(|v| {
                v.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '+' || c == '-')
            })
        {
            i += 1;
            continue;
        }
        // Value-taking long flags (space-separated)
        if matches!(arg, "--kill-after" | "--signal") {
            i += 2; // skip flag + value
            continue;
        }
        // Short value-taking flags: -k, -s (separate or fused)
        if matches!(arg, "-k" | "-s") {
            i += 2;
            continue;
        }
        // Fused short: `-k5s` or `-sTERM` — validate value is alphanumeric
        if (arg.starts_with("-k") || arg.starts_with("-s"))
            && arg.len() > 2
            && arg[2..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '+' || c == '-')
        {
            i += 1;
            continue;
        }

        // Next non-flag token should be the DURATION
        if DURATION_RE.is_match(arg) {
            i += 1;
            break;
        }

        // Unknown flag or non-matching duration → fail-closed
        return None;
    }

    if i >= len {
        return None;
    }
    Some(args[i..].to_vec())
}

/// `nice [-n N] cmd...` or `nice -N cmd...` (legacy)
fn strip_nice(args: &[String]) -> Option<Vec<String>> {
    #[allow(clippy::expect_used)]
    static EXPANSION_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\$\(|\$\{|`").expect("valid regex"));

    if args.len() < 2 {
        return None;
    }

    let mut i = 1;
    let arg1 = args.get(i)?.as_str();

    // Reject expansions in any flag argument
    if EXPANSION_RE.is_match(arg1) {
        return None;
    }

    // `-n N` form
    if arg1 == "-n" {
        i += 1;
        // Validate N is a number (possibly negative)
        let n_val = args.get(i)?.as_str();
        // Reject expansions in the numeric argument too
        if EXPANSION_RE.is_match(n_val) {
            return None;
        }
        if n_val.is_empty()
            || !n_val
                .strip_prefix('-')
                .unwrap_or(n_val)
                .chars()
                .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        i += 1;
    }
    // Legacy `-N` form (e.g. `-10`): single dash followed by digits only.
    // Reject `--5` (double dash is invalid for nice legacy form).
    else if arg1.starts_with('-')
        && !arg1.starts_with("--")
        && arg1.len() > 1
        && arg1[1..].chars().all(|c| c.is_ascii_digit())
    {
        i += 1;
    }
    // Unknown flag starting with `-` → fail-closed
    else if arg1.starts_with('-') {
        return None;
    }
    // No flag — just `nice cmd`
    // (arg1 is the command itself, don't skip it)

    if i >= args.len() {
        return None;
    }
    Some(args[i..].to_vec())
}

/// `env [VAR=val]... [FLAGS] cmd...`
///
/// Safe to skip: `VAR=val` assignments, `-i`, `-0`, `-v`, `-u NAME`
/// Reject (fail-closed): `-S` (argv splitter), `-C`, `-P`, unknown flags, `--anything`
fn strip_env(args: &[String]) -> Option<Vec<String>> {
    #[allow(clippy::expect_used)]
    static VAR_ASSIGN_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*(\[[^\]]*\])?\+?=").expect("valid regex"));

    let mut i = 1; // skip "env"
    let len = args.len();

    while i < len {
        let arg = args[i].as_str();

        // VAR=val or VAR+=val or VAR[idx]=val
        if VAR_ASSIGN_RE.is_match(arg) {
            i += 1;
            continue;
        }

        // Safe flags
        if matches!(arg, "-i" | "-0" | "-v") {
            i += 1;
            continue;
        }
        // -u NAME (unset one var)
        if arg == "-u" {
            i += 2;
            continue;
        }

        // Dangerous flags → fail-closed
        if matches!(arg, "-S" | "-C" | "-P") || arg.starts_with("--") || arg.starts_with('-') {
            return None;
        }

        // First non-flag, non-assignment token is the command
        break;
    }

    if i >= len {
        return None;
    }
    Some(args[i..].to_vec())
}

/// `stdbuf [-ioe MODE]... cmd...`
///
/// Short forms: `-i MODE`, `-o MODE`, `-e MODE` (separate) or `-iMODE` (fused)
/// Long forms: `--input=MODE`, `--output=MODE`, `--error=MODE`
fn strip_stdbuf(args: &[String]) -> Option<Vec<String>> {
    #[allow(clippy::expect_used)]
    static SHORT_SEP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^-[ioe]$").expect("valid regex"));
    #[allow(clippy::expect_used)]
    static SHORT_FUSED_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^-[ioe].").expect("valid regex"));
    #[allow(clippy::expect_used)]
    static LONG_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^--(input|output|error)=").expect("valid regex"));

    let mut i = 1; // skip "stdbuf"
    let len = args.len();

    while i < len {
        let arg = args[i].as_str();

        // Long form: --input=MODE, --output=MODE, --error=MODE
        if LONG_RE.is_match(arg) {
            i += 1;
            continue;
        }
        // Short fused: -iMODE, -oMODE, -eMODE
        if SHORT_FUSED_RE.is_match(arg) {
            i += 1;
            continue;
        }
        // Short separate: -i MODE, -o MODE, -e MODE
        if SHORT_SEP_RE.is_match(arg) {
            i += 2; // skip flag + MODE
            continue;
        }

        // Any other flag → fail-closed
        if arg.starts_with('-') {
            return None;
        }

        // First non-flag is the command
        break;
    }

    if i >= len {
        return None;
    }
    Some(args[i..].to_vec())
}

#[cfg(test)]
#[path = "wrappers.test.rs"]
mod tests;
