//! Filesystem permission checking.
//!
//! Validates file paths against allowed directories, detects path traversal,
//! dangerous files/directories, and suspicious Windows patterns.

use std::path::Path;
use std::path::PathBuf;

// ── Dangerous paths ──

/// Files that should never be auto-edited (dotfiles, config).
const DANGEROUS_FILES: &[&str] = &[
    ".gitconfig",
    ".gitmodules",
    ".bashrc",
    ".bash_profile",
    ".zshrc",
    ".zprofile",
    ".profile",
    ".ripgreprc",
    ".mcp.json",
    ".claude.json",
];

/// Directories whose contents should not be auto-edited.
///
/// `.claude` and `.codex` are kept because coco reads those agents' config dirs for compat (see
/// `is_protected_config`; Codex/AGENTS.md convention); `.coco` is coco's own
/// config home and must be guarded the same way. Coco-managed
/// sub-paths the agent legitimately writes (the session plan file, agent memory)
/// are carved out earlier in the write check via `is_editable_internal_path`,
/// which runs before this safety gate.
const DANGEROUS_DIRECTORIES: &[&str] = &[".git", ".vscode", ".idea", ".claude", ".coco", ".codex"];

/// System directories blocked for all writes.
const BLOCKED_SYSTEM_DIRS: &[&str] = &[
    "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/dev", "/proc", "/sys", "/var/run",
];

// ── Path traversal detection ──

/// Check if a path contains `..` traversal components.
///
/// Regex: `(?:^|[\\/])\.\.(?:[\\/]|$)`
pub fn contains_path_traversal(path: &str) -> bool {
    for component in path.split(&['/', '\\']) {
        if component == ".." {
            return true;
        }
    }
    false
}

// ── Dangerous file/directory detection ──

/// Check if a path targets a dangerous file (dotfiles, config files).
pub fn is_dangerous_file_path(path: &str) -> bool {
    let lower = path.to_lowercase();

    // Check filename against dangerous files
    if let Some(filename) = Path::new(&lower).file_name().and_then(|n| n.to_str()) {
        for &dangerous in DANGEROUS_FILES {
            if filename == dangerous {
                return true;
            }
        }
    }

    // Check if any path component is a dangerous directory. The check
    // inspects `pathSegments[i + 1]` — so the `.coco/worktrees/` exemption
    // is anchored at a component boundary, not a position-agnostic substring.
    // A nested `.coco` deeper in the path (e.g. a settings.json inside a
    // worktree) is still evaluated and blocked.
    let segments: Vec<&str> = lower.split(['/', '\\']).collect();
    for i in 0..segments.len() {
        let component = segments[i];
        if component.is_empty() {
            continue;
        }
        for &dangerous_dir in DANGEROUS_DIRECTORIES {
            if component != dangerous_dir {
                continue;
            }
            // Structural git-worktree path: skip ONLY this `.coco` segment
            // when the immediately-following component is `worktrees`.
            if dangerous_dir == ".coco" && segments.get(i + 1).copied() == Some("worktrees") {
                break;
            }
            return true;
        }
    }

    // Block UNC paths
    if lower.starts_with("\\\\") || lower.starts_with("//") {
        return true;
    }

    false
}

/// Check if a path is a coco settings or config file.
///
/// coco serves its config from `.coco/`: project `settings.json` /
/// `settings.local.json`, `commands/`, `agents/`, and `skills/` all live
/// under `<cwd>/.coco/`. Editing any of these is treated as a config edit
/// requiring approval. The match is a path-substring over-approximation —
/// coco threads no cwd through `check_path_safety_for_auto_edit`, and
/// over-matching only over-prompts, which is safe for an approval gate.
pub fn is_coco_config_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/").to_lowercase();
    normalized.contains("/.coco/settings.json")
        || normalized.contains("/.coco/settings.local.json")
        || normalized.contains("/.coco/commands/")
        || normalized.contains("/.coco/agents/")
        || normalized.contains("/.coco/skills/")
}

// ── Suspicious Windows path patterns ──

/// Check for suspicious Windows path patterns that could bypass security.
pub fn has_suspicious_windows_pattern(path: &str) -> bool {
    // NTFS Alternate Data Streams: "file.txt::$DATA", "settings.json:stream".
    // Colons are valid filename characters on Linux/macOS, so this check is
    // Windows-only. Without the guard a legitimate Unix path like
    // `/tmp/log:2026-06-01.txt` is spuriously flagged. Skip the drive letter
    // at position 1 (e.g. C:).
    if cfg!(target_os = "windows") && path.len() > 2 && path[2..].contains(':') {
        return true;
    }

    // 8.3 short name pattern: "GIT~1", "CLAUDE~1"
    if path.contains("~1")
        || path.contains("~2")
        || path.contains("~3")
        || path.contains("~4")
        || path.contains("~5")
        || path.contains("~6")
        || path.contains("~7")
        || path.contains("~8")
        || path.contains("~9")
    {
        return true;
    }

    // Long path prefixes: \\?\, \\.\, //?/, //./
    if path.starts_with("\\\\?\\")
        || path.starts_with("\\\\.\\")
        || path.starts_with("//?/")
        || path.starts_with("//./")
    {
        return true;
    }

    // Trailing dots or spaces
    if path.ends_with('.') || path.ends_with(' ') {
        return true;
    }

    // DOS device names as extensions: .CON, .PRN, .AUX, .NUL, .COM1-9, .LPT1-9
    let upper = path.to_uppercase();
    for suffix in &[
        ".CON", ".PRN", ".AUX", ".NUL", ".COM1", ".COM2", ".COM3", ".COM4", ".COM5", ".COM6",
        ".COM7", ".COM8", ".COM9", ".LPT1", ".LPT2", ".LPT3", ".LPT4", ".LPT5", ".LPT6", ".LPT7",
        ".LPT8", ".LPT9",
    ] {
        if upper.ends_with(suffix) {
            return true;
        }
    }

    // Triple+ dots as path component: .../file.txt
    for component in path.split(&['/', '\\']) {
        if component.len() >= 3 && component.chars().all(|c| c == '.') {
            return true;
        }
    }

    false
}

// ── Path safety validation ──

/// Result of a path safety check.
#[derive(Debug, Clone)]
pub enum PathSafetyResult {
    Safe,
    Blocked {
        message: String,
        /// Whether an LLM classifier can still approve this.
        classifier_approvable: bool,
    },
}

/// Check path safety for auto-edit mode.
///
/// Validation order:
/// 1. Suspicious Windows patterns (NTFS ADS, 8.3, long-path, etc.)
/// 2. Shell expansion patterns ($VAR, `cmd`, %VAR%)
/// 3. Dangerous tilde variants (~user, ~+, ~-)
/// 4. Path traversal (..)
/// 5. Claude config files
/// 6. Dangerous files/directories
pub fn check_path_safety_for_auto_edit(path: &str) -> PathSafetyResult {
    // 1. Suspicious Windows patterns
    if has_suspicious_windows_pattern(path) {
        return PathSafetyResult::Blocked {
            message: format!("suspicious path pattern detected: {path}"),
            classifier_approvable: false,
        };
    }

    // 2. Shell expansion — TOCTOU: path is literal to us but shell may expand
    if has_shell_expansion(path) {
        return PathSafetyResult::Blocked {
            message: format!("path contains shell expansion patterns: {path}"),
            classifier_approvable: false,
        };
    }

    // 3. Dangerous tilde variants
    if has_dangerous_tilde(path) {
        return PathSafetyResult::Blocked {
            message: format!("unsafe tilde expansion in path: {path}"),
            classifier_approvable: false,
        };
    }

    // 4. Path traversal
    if contains_path_traversal(path) {
        return PathSafetyResult::Blocked {
            message: format!("path contains traversal (..): {path}"),
            classifier_approvable: false,
        };
    }

    // 5. coco config files
    if is_coco_config_path(path) {
        return PathSafetyResult::Blocked {
            message: format!("editing coco config files requires approval: {path}"),
            classifier_approvable: true,
        };
    }

    // 6. Dangerous files/directories
    if is_dangerous_file_path(path) {
        return PathSafetyResult::Blocked {
            message: format!("dangerous file path requires approval: {path}"),
            classifier_approvable: true,
        };
    }

    PathSafetyResult::Safe
}

// ── Working directory containment ──

/// Check if a path is within the working directory (or allowed directories).
///
/// Handles macOS /private/ symlinks and case-insensitive comparison.
pub fn path_in_working_path(path: &str, working_path: &str) -> bool {
    let abs_path = normalize_for_comparison(&resolve_path(path, working_path));
    let abs_working = normalize_for_comparison(&resolve_path(working_path, working_path));

    // Exact match
    if abs_path == abs_working {
        return true;
    }

    // Path must start with working_path + separator
    let with_sep = if abs_working.ends_with('/') {
        abs_working
    } else {
        format!("{abs_working}/")
    };

    abs_path.starts_with(&with_sep)
}

/// Check if a path is within the allowed working directories.
///
/// Checks cwd + `additionalWorkingDirectories` only. The coco-managed
/// exemptions (session plan file, agent memory) live in
/// [`is_readable_internal_path`] / [`is_editable_internal_path`], NOT here:
/// conflating them let an arbitrary out-of-tree write auto-pass the cwd gate.
pub fn is_path_within_allowed_dirs(path: &str, cwd: &str, additional_dirs: &[String]) -> bool {
    // Check cwd
    if path_in_working_path(path, cwd) {
        return true;
    }

    // Check additional working directories
    for dir in additional_dirs {
        if path_in_working_path(path, dir) {
            return true;
        }
    }

    false
}

/// Check if a path is a scratchpad directory (temporary workspace).
pub fn is_scratchpad_dir(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("/tmp/")
        || lower.contains("/scratch/")
        || lower.contains("/scratchpad/")
        || lower.contains("/.cache/")
}

/// Normalize and resolve a path for permission checking.
fn resolve_path(path: &str, cwd: &str) -> String {
    let expanded = expand_tilde(path);
    let absolute = if expanded.starts_with('/') {
        PathBuf::from(expanded)
    } else {
        PathBuf::from(cwd).join(expanded)
    };
    coco_paths::normalize_lexical(&absolute)
        .to_string_lossy()
        .to_string()
}

/// Normalize a path for comparison: lowercase, handle macOS /private/ symlinks.
fn normalize_for_comparison(path: &str) -> String {
    let mut p = path.to_lowercase();
    // macOS symlink normalization
    if p.starts_with("/private/var/") {
        p = p.replacen("/private/var/", "/var/", 1);
    }
    if p.starts_with("/private/tmp/") {
        p = p.replacen("/private/tmp/", "/tmp/", 1);
    } else if p.starts_with("/private/tmp") {
        p = p.replacen("/private/tmp", "/tmp", 1);
    }
    p
}

/// Expand `~` to `$HOME`. Rejects dangerous tilde variants.
///
/// Blocks `~user`, `~+`, `~-`, `=cmd` to prevent TOCTOU and shell expansion attacks.
fn expand_tilde(path: &str) -> String {
    if path.starts_with("~/") || path == "~" {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        path.replacen('~', &home, 1)
    } else {
        path.to_string()
    }
}

/// Check for dangerous tilde variants that could expand unexpectedly.
///
/// Blocks `~user`, `~+`, `~-` to prevent TOCTOU attacks where shell expands these at runtime.
pub fn has_dangerous_tilde(path: &str) -> bool {
    if !path.starts_with('~') {
        return false;
    }
    // Safe: "~" alone or "~/..."
    if path == "~" || path.starts_with("~/") {
        return false;
    }
    // Everything else is suspicious: ~user, ~+, ~-, ~1, etc.
    true
}

/// Check for shell variable expansion patterns that bypass path validation.
///
/// Blocks `$VAR`, `${VAR}`, `$(cmd)`, backticks, and Zsh `=cmd` expansion.
/// These are literals to the filesystem but shells expand them, creating a
/// TOCTOU gap between validation and execution.
pub fn has_shell_expansion(path: &str) -> bool {
    // $VAR or ${VAR}
    if path.contains('$') {
        return true;
    }
    // Backtick command substitution
    if path.contains('`') {
        return true;
    }
    // Zsh equals expansion: =cmd expands to path of `cmd` binary
    if path.starts_with('=') && path.len() > 1 && path.as_bytes()[1].is_ascii_alphabetic() {
        return true;
    }
    // Windows %VAR%
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(end) = bytes[i + 1..].iter().position(|&b| b == b'%')
            && end > 0
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Get the default allowed directories for a project.
pub fn get_default_allowed_dirs(cwd: &Path) -> Vec<PathBuf> {
    vec![cwd.to_path_buf(), PathBuf::from("/tmp")]
}

/// Validate that a file path is safe for write operations.
///
/// Returns None if safe, Some(reason) if blocked.
pub fn validate_write_path(path: &str, cwd: &str, additional_dirs: &[String]) -> Option<String> {
    let resolved = resolve_path(path, cwd);

    // Path traversal check
    if contains_path_traversal(path) {
        return Some(format!("path traversal detected: {path}"));
    }

    // Block writes to system directories
    for &blocked in BLOCKED_SYSTEM_DIRS {
        if resolved.starts_with(blocked) {
            return Some(format!("write to system directory blocked: {blocked}"));
        }
    }

    // Check if within allowed directories
    if !is_path_within_allowed_dirs(path, cwd, additional_dirs) {
        return Some(format!("path outside allowed directories: {path}"));
    }

    None
}

// ── Symlink chain resolution ──

/// Maximum symlink chain depth (matches POSIX SYMLOOP_MAX).
const MAX_SYMLINK_DEPTH: usize = 40;

/// Collect all paths that must be checked for a single input path.
///
/// Follows symlink chains (up to 40 hops), collecting every intermediate
/// target. This prevents symlink bypass attacks where `./link → /etc/passwd`
/// would pass a CWD-relative check.
///
/// Returns: Vec of absolute paths to check (original + all intermediate + resolved).
pub fn get_paths_for_permission_check(path: &str, cwd: &str) -> Vec<String> {
    let abs_path = resolve_path(path, cwd);
    let mut paths = vec![abs_path.clone()];

    // UNC paths: block before any filesystem access
    if abs_path.starts_with("//") || abs_path.starts_with("\\\\") {
        return paths;
    }

    let mut current = PathBuf::from(&abs_path);
    let mut visited = std::collections::HashSet::new();

    for _ in 0..MAX_SYMLINK_DEPTH {
        // Check if path exists and is a symlink
        let meta = match std::fs::symlink_metadata(&current) {
            Ok(m) => m,
            Err(_) => {
                // Non-existent: resolve deepest existing ancestor
                if let Some(resolved) = resolve_deepest_existing_ancestor(&current) {
                    let resolved_str = resolved.to_string_lossy().to_string();
                    if !paths.contains(&resolved_str) {
                        paths.push(resolved_str);
                    }
                }
                break;
            }
        };

        if !meta.file_type().is_symlink() {
            break;
        }

        // Circular symlink detection
        if !visited.insert(current.clone()) {
            break;
        }

        // Read the symlink target
        let target = match std::fs::read_link(&current) {
            Ok(t) => t,
            Err(_) => break,
        };

        // Resolve relative targets against symlink's parent directory
        let resolved_target = if target.is_absolute() {
            target
        } else if let Some(parent) = current.parent() {
            parent.join(&target)
        } else {
            target
        };

        let target_str = resolved_target.to_string_lossy().to_string();
        if !paths.contains(&target_str) {
            paths.push(target_str);
        }

        current = resolved_target;
    }

    // Final canonical resolution via realpath
    if let Ok(canonical) = std::fs::canonicalize(&abs_path) {
        let canonical_str = canonical.to_string_lossy().to_string();
        if !paths.contains(&canonical_str) {
            paths.push(canonical_str);
        }
    }

    paths
}

/// For a non-existent path, resolve the deepest existing ancestor to
/// find where the file would actually be created after symlink resolution.
fn resolve_deepest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    let mut tail_segments: Vec<std::ffi::OsString> = Vec::new();

    // Walk up until we find an existing path
    loop {
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                // Found existing path — resolve it
                if meta.file_type().is_symlink()
                    && let Ok(resolved) = std::fs::canonicalize(&current)
                {
                    // Rejoin tail segments
                    let mut result = resolved;
                    for seg in tail_segments.into_iter().rev() {
                        result.push(seg);
                    }
                    return Some(result);
                }
                // Existing non-symlink: resolve via canonicalize
                if let Ok(resolved) = std::fs::canonicalize(&current)
                    && resolved != current
                {
                    let mut result = resolved;
                    for seg in tail_segments.into_iter().rev() {
                        result.push(seg);
                    }
                    return Some(result);
                }
                return None; // No symlinks in ancestry
            }
            Err(_) => {
                // Not found — save segment and move up
                if let Some(name) = current.file_name() {
                    tail_segments.push(name.to_owned());
                }
                if !current.pop() {
                    return None; // Reached root
                }
            }
        }
    }
}

// ── Internal path exemptions ──

/// Resolved per-session inputs the internal-path carve-outs key on.
///
/// Bundles the values the exemptions need so callers pass one struct instead
/// of a widening list of positional args (CLAUDE.md: typed params over
/// ambiguous positionals). All fields are borrows — the struct is built fresh
/// at each call site from the live tool permission context.
pub struct InternalPathContext<'a> {
    /// Working directory the target path is resolved against.
    pub cwd: &'a str,
    /// Pre-resolved session plan file (`<plansDir>/<slug>.md`). Plan-file
    /// reads/writes are exempted when the target shares this prefix. The
    /// engine resolves it once and threads it through the permission context.
    pub session_plan_file: Option<&'a Path>,
}

/// The plan file is `<plansDir>/<slug>.md`; subagent plans are
/// `<slug>-agent-<id>.md`. Strip the `.md` suffix off the resolved session
/// plan file to recover the `<plansDir>/<slug>` prefix so both forms match
/// from a single context. `normalized` is produced by the caller via
/// [`resolve_path`] (which collapses `..` lexically) + [`normalize_for_comparison`]
/// (lowercase), so this is a traversal-safe pure string prefix test; we run
/// the stored plan file through the same lowercasing for case-consistent
/// comparison.
fn is_session_plan_file(normalized: &str, session_plan_file: Option<&Path>) -> bool {
    let Some(plan_file) = session_plan_file else {
        return false;
    };
    let plan = normalize_for_comparison(&plan_file.to_string_lossy());
    let Some(prefix) = plan.strip_suffix(".md") else {
        return false;
    };
    normalized.ends_with(".md") && normalized.starts_with(prefix)
}

/// Paths within the project memory directory that are auto-writable.
///
/// Exemptions: plan files, scratchpad, agent memory, CLAUDE.md.
pub fn is_editable_internal_path(path: &str, ctx: &InternalPathContext) -> bool {
    let normalized = normalize_for_comparison(&resolve_path(path, ctx.cwd));

    // Plan files: the session's own `<plansDir>/<slug>.md` (cocohome by
    // default). Keyed on the resolved session plan file, not a path substring,
    // so it lands wherever `plansDirectory` actually resolves and stays scoped
    // to this session's slug.
    if is_session_plan_file(&normalized, ctx.session_plan_file) {
        return true;
    }

    // Agent memory: ~/.coco/projects/{cwd}/memory/
    let config_home = coco_config::global_config::config_home()
        .to_string_lossy()
        .to_lowercase();
    if normalized.starts_with(&format!("{config_home}/projects/"))
        && normalized.contains("/memory/")
    {
        return true;
    }

    // CLAUDE.md files (project instructions)
    let filename = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if filename == "CLAUDE.md" || filename == "CLAUDE.local.md" {
        return is_path_within_allowed_dirs(path, ctx.cwd, &[]);
    }

    false
}

/// Paths within internal directories that are auto-readable.
///
/// Exemptions: session memory, project dir, plan files, tool results,
/// scratchpad, project temp, agent memory.
pub fn is_readable_internal_path(path: &str, ctx: &InternalPathContext) -> bool {
    let normalized = normalize_for_comparison(&resolve_path(path, ctx.cwd));
    let config_home = coco_config::global_config::config_home()
        .to_string_lossy()
        .to_lowercase();

    // Project dir: ~/.coco/projects/{sanitized-cwd}/ (covers agent memory).
    if normalized.starts_with(&format!("{config_home}/projects/")) {
        return true;
    }

    // Plan files (readable in all modes) — the session's own plan file. Same
    // key as the write carve-out.
    if is_session_plan_file(&normalized, ctx.session_plan_file) {
        return true;
    }

    false
}

// ── Dangerous removal detection ──

/// Check if a path is dangerously broad for removal operations (rm, rmdir).
///
/// Blocks: /, ~, /*, wildcards, drive roots, root children.
pub fn is_dangerous_removal_path(path: &str) -> bool {
    let trimmed = path.trim();

    // Wildcard patterns: *, /*, \*
    if trimmed == "*" || trimmed.ends_with("/*") || trimmed.ends_with("\\*") {
        return true;
    }

    // Root directory
    if trimmed == "/" || trimmed == "\\" {
        return true;
    }

    // Home directory
    if trimmed == "~" || trimmed == "~/" {
        return true;
    }

    // Expand and check
    let expanded = expand_tilde(trimmed);
    let normalized = expanded.replace('\\', "/");

    // Direct children of root: /usr, /etc, /tmp, /var, /home, etc.
    if normalized.starts_with('/') && !normalized[1..].contains('/') && normalized.len() > 1 {
        return true;
    }

    // Windows drive roots: C:\, D:\
    if normalized.len() >= 2
        && normalized.as_bytes()[0].is_ascii_alphabetic()
        && normalized.as_bytes()[1] == b':'
    {
        // C: or C:\ or C:/
        if normalized.len() <= 3 {
            return true;
        }
        // Direct children: C:\Windows, C:\Users
        let after_drive = &normalized[3..];
        if !after_drive.contains('/') {
            return true;
        }
    }

    false
}

#[cfg(test)]
#[path = "filesystem.test.rs"]
mod tests;
