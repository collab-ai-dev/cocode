//! Path validation for shell commands.
//!
//! Checks file paths in commands for dangerous targets (/, /etc, /usr, etc.)

/// Dangerous filesystem paths that always require approval for rm/rmdir.
const DANGEROUS_REMOVAL_PATHS: &[&str] = &[
    "/",
    "/etc",
    "/usr",
    "/lib",
    "/lib64",
    "/bin",
    "/sbin",
    "/boot",
    "/dev",
    "/proc",
    "/sys",
    "/var",
    "/tmp",
    "/root",
    "/home",
    "~",
    "~/.claude",
    "~/.coco",
];

/// Check if a file path is dangerous for a destructive operation.
///
/// Returns Some(reason) if the path is dangerous, None if safe.
pub fn check_dangerous_path(_command: &str, path: &str, cwd: &str) -> Option<String> {
    let resolved = resolve_path(path, cwd);

    for &dangerous in DANGEROUS_REMOVAL_PATHS {
        let expanded = expand_home(dangerous);
        // Only flag if the resolved path IS the dangerous directory itself
        // (not a deep descendant within it, which is usually safe)
        if resolved == expanded {
            return Some(format!("dangerous target path: {path}"));
        }
        // Flag direct children of root-level dangerous paths (e.g., /usr/lib)
        if dangerous == "/" && resolved.matches('/').count() == 1 {
            return Some(format!("dangerous target path: {path}"));
        }
    }

    None
}

/// Extract file paths from command arguments based on command type.
pub fn extract_paths_from_command(command_name: &str, args: &[&str]) -> Vec<String> {
    match command_name {
        "cd" => {
            if args.is_empty() {
                vec!["~".to_string()]
            } else {
                vec![args.join(" ")]
            }
        }
        "ls" | "mkdir" | "touch" | "rm" | "rmdir" | "cat" | "head" | "tail" | "wc" | "file"
        | "stat" | "diff" | "strings" | "hexdump" | "nl" => filter_flags(args),
        "find" => extract_find_paths(args),
        "grep" | "rg" => extract_pattern_command_paths(args),
        "mv" | "cp" => filter_flags(args),
        "sed" => extract_sed_paths(args),
        "git" => Vec::new(), // Git manages its own paths
        _ => Vec::new(),
    }
}

/// Commands whose positional arguments WRITE or CREATE filesystem entries.
/// A target outside the allowed working dirs is force-asked by the bash
/// permission gate. Read commands (`cat`/`ls`/`grep`/`cd`/…) are intentionally
/// NOT fenced here: gating routine out-of-cwd navigation/inspection (`ls ..`,
/// `cat ../x`) would be too noisy, and reads are non-destructive — they rely on
/// the Read tool's own fence plus the kernel sandbox layer when enabled.
const WRITE_PATH_COMMANDS: &[&str] = &["rm", "rmdir", "mv", "cp", "touch", "mkdir"];

/// Extract write/create path targets from each subcommand (env + wrapper
/// stripped) for out-of-tree validation by the bash permission gate. Reuses the
/// per-command `PATH_EXTRACTORS` shapes. Pure — the allowed-dirs decision lives
/// in `core/tools` (it needs `coco_permissions`, which `coco-shell` must not
/// depend on; see the layering note in the hardening doc §3.3).
pub fn extract_write_path_targets(command: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for sub in crate::bash_permissions::split_compound_command(command) {
        // Strip leading env vars + safe wrappers (`timeout`/`nice`/…) so a
        // wrapped write (`FOO=1 timeout 5 cp x /etc`) can't bypass the gate.
        let stripped = crate::bash_permissions::strip_safe_wrappers(
            &crate::bash_permissions::strip_all_env_vars(sub.trim(), /*check_hijack*/ false),
        );
        let base = crate::mode_validation::extract_base_executable(&stripped);
        if !WRITE_PATH_COMMANDS.contains(&base) {
            continue;
        }
        let argv = subcommand_argv(&stripped);
        let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
        targets.extend(extract_paths_from_command(base, &args));
    }
    targets
}

// ── Force-ask gates (consumed by BashTool::check_permissions) ──
//
// These run BEFORE any allow rule / acceptEdits auto-allow, so a match returns
// an Ask the model can't override. All pure (plus an FS stat for the bare-repo
// probe) — no cross-crate dependency, so they stay in `coco-shell` (avoids a
// coco-shell → coco-permissions cycle).

const GIT_INTERNAL_SEGMENTS: &[&str] = &["HEAD", "objects", "refs", "hooks"];

/// `argv` of a single (non-compound) subcommand, with leading env-var
/// assignments stripped. Whitespace-split (quote-stripped) — sufficient for
/// flag/positional extraction.
fn subcommand_argv(sub: &str) -> Vec<String> {
    let stripped =
        crate::bash_permissions::strip_all_env_vars(sub.trim(), /*check_hijack*/ false);
    stripped
        .split_whitespace()
        .map(|t| t.trim_matches(['\'', '"']).to_string())
        .collect()
}

/// Force-ask if a destructive removal/copy/move targets a critical system path
/// (`rm -rf /`, `rm -rf ~`, `cp x /etc`, …): such a target cannot be
/// auto-allowed by permission rules.
pub fn check_dangerous_removal(command: &str, cwd: &str) -> Option<String> {
    for sub in crate::bash_permissions::split_compound_command(command) {
        let base = crate::mode_validation::extract_base_executable(sub.trim());
        if !matches!(base, "rm" | "rmdir" | "cp" | "mv") {
            continue;
        }
        let argv = subcommand_argv(&sub);
        let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
        for target in extract_paths_from_command(base, &args) {
            if check_dangerous_path(base, &target, cwd).is_some() {
                return Some(format!(
                    "Dangerous `{base}` operation on '{target}' requires explicit approval \
                     and cannot be auto-allowed by permission rules."
                ));
            }
        }
    }
    None
}

/// Cheap (no-FS) git sandbox-escape detection: a compound `cd … && git …`, or a
/// command that writes git-internal files (`HEAD`/`objects`/`refs`/`hooks`)
/// then runs git. Used by `BashTool::is_read_only` so such commands are NOT
/// auto-classified read-only.
pub fn has_git_escape_pattern(command: &str) -> bool {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string());
    has_git_escape_pattern_in_cwd(command, &cwd)
}

/// Cwd-aware variant used by the permission gate.
pub fn has_git_escape_pattern_in_cwd(command: &str, cwd: &str) -> bool {
    let analysis = crate::bash_permissions::analyze_compound_command(command, cwd);
    analysis.has_git
        && (analysis.has_non_noop_cwd_change()
            || command_writes_git_internal(&analysis.significant_subcommands))
}

/// Force-ask if a compound command changes cwd more than once after no-op cwd
/// changes have been filtered. This mirrors TS's conservative path validation.
pub fn check_multiple_cwd_changes(command: &str, cwd: &str) -> Option<String> {
    let analysis = crate::bash_permissions::analyze_compound_command(command, cwd);
    analysis
        .has_multiple_cwd_changes()
        .then(|| "Compound commands with multiple directory changes require approval.".to_string())
}

/// Force-ask gate for git sandbox-escape: `cd`+`git` compound, git-internal
/// writes before git, or git run inside a bare-repo cwd.
pub fn check_git_escape(command: &str, cwd: &str) -> Option<String> {
    let analysis = crate::bash_permissions::analyze_compound_command(command, cwd);
    if !analysis.has_git {
        return None;
    }
    if analysis.has_non_noop_cwd_change() {
        return Some(
            "Compound commands with `cd` and `git` require approval to prevent \
             bare-repository attacks."
                .into(),
        );
    }
    if command_writes_git_internal(&analysis.significant_subcommands) {
        return Some(
            "Commands that create git-internal files (HEAD/objects/refs/hooks) and run \
             git require approval."
                .into(),
        );
    }
    if is_current_dir_bare_git_repo(cwd) {
        return Some(
            "Git commands in a directory with bare-repository structure require approval.".into(),
        );
    }
    None
}

/// Whether any subcommand writes a path whose first segment is a git-internal
/// directory/file (`HEAD`/`objects`/`refs`/`hooks`).
fn command_writes_git_internal(subs: &[String]) -> bool {
    for sub in subs {
        let base = crate::mode_validation::extract_base_executable(sub.trim());
        if !matches!(base, "mkdir" | "touch" | "cp" | "mv") {
            continue;
        }
        let argv = subcommand_argv(sub);
        let args: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
        for target in extract_paths_from_command(base, &args) {
            let norm = target.trim_start_matches("./").trim_start_matches('/');
            let head = norm.split('/').next().unwrap_or(norm);
            if GIT_INTERNAL_SEGMENTS.contains(&head) {
                return true;
            }
        }
    }
    false
}

/// Checks whether the cwd itself looks like a git dir (`HEAD` + `objects/` +
/// `refs/`) but is NOT a normal working tree (`.git` absent) — a planted bare
/// repo a `git` command could be tricked into using.
fn is_current_dir_bare_git_repo(cwd: &str) -> bool {
    use std::path::Path;
    let dir = Path::new(cwd);
    if dir.join(".git").exists() {
        return false;
    }
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// Filter out flags from args, returning only positional arguments.
/// Handles the `--` end-of-options delimiter.
fn filter_flags(args: &[&str]) -> Vec<String> {
    let mut result = Vec::new();
    let mut after_double_dash = false;

    for &arg in args {
        if arg == "--" {
            after_double_dash = true;
            continue;
        }
        if after_double_dash || !arg.starts_with('-') {
            result.push(arg.to_string());
        }
    }
    result
}

/// Extract paths from `find` command arguments.
fn extract_find_paths(args: &[&str]) -> Vec<String> {
    let mut paths = Vec::new();
    for &arg in args {
        if arg.starts_with('-') || arg.starts_with('(') || arg.starts_with('!') {
            break;
        }
        paths.push(arg.to_string());
    }
    if paths.is_empty() {
        paths.push(".".to_string());
    }
    paths
}

/// Extract paths from pattern commands (grep, rg) — skip the pattern.
fn extract_pattern_command_paths(args: &[&str]) -> Vec<String> {
    if args.len() <= 1 {
        return Vec::new();
    }
    // First non-flag arg is the pattern, rest are paths
    let mut found_pattern = false;
    let mut paths = Vec::new();
    for &arg in args {
        if arg.starts_with('-') {
            continue;
        }
        if !found_pattern {
            found_pattern = true;
            continue;
        }
        paths.push(arg.to_string());
    }
    paths
}

/// Extract file paths from sed command args.
fn extract_sed_paths(args: &[&str]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut skip_next = false;
    let mut found_expression = false;

    for &arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if matches!(arg, "-e" | "--expression" | "-f" | "--file") {
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        if !found_expression {
            found_expression = true; // First positional is the expression
            continue;
        }
        paths.push(arg.to_string());
    }
    paths
}

/// Resolve a relative path against the working directory.
fn resolve_path(path: &str, cwd: &str) -> String {
    let expanded = expand_home(path);
    if expanded.starts_with('/') {
        expanded
    } else {
        format!("{cwd}/{expanded}")
    }
}

/// Expand ~ to home directory.
fn expand_home(path: &str) -> String {
    if path.starts_with("~/") || path == "~" {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        path.replacen('~', &home, 1)
    } else {
        path.to_string()
    }
}

#[cfg(test)]
#[path = "path_validation.test.rs"]
mod tests;
