//! Claude-specific glue between settings + permission rules and the sandbox
//! runtime config.
//!
//! ## Layering
//!
//! This module is the steering wheel; the rest of `coco-sandbox` is the engine.
//! It takes user-facing inputs (settings, permission rules, directories) and
//! produces the runtime [`SandboxConfig`] + resolved [`EnforcementLevel`] that
//! drives [`crate::SandboxState`]. It contains zero platform-specific logic.
//!
//! ## What gets folded in
//!
//! 1. Permission `Edit(/path)` allow rules → `SandboxConfig.writable_roots`
//! 2. Permission `Edit(/path)` deny rules → `SandboxConfig.deny_write_paths`
//! 3. Permission `Read(/path)` deny rules → `SandboxConfig.denied_read_paths`
//! 4. Permission `WebFetch(domain:HOST)` rules → `SandboxSettings.network.{allowed,denied}_domains`
//! 5. `SandboxSettings.filesystem.{allow_write, deny_write, deny_read}` paths
//!    are resolved (`~/x` → home, relative → settings_root) and folded in
//! 6. CWD + Claude temp dir always writable (mirrors TS line 225)
//! 7. Settings.json + `.coco/skills` always denied write
//! 8. Worktree main-repo path added as writable when in a worktree (TS line 286)

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use coco_types::SandboxMode;

use crate::config::EnforcementLevel;
use crate::config::SandboxConfig;
use crate::config::SandboxSettings;
use crate::config::WritableRoot;

/// Tool names referenced by the adapter — match `coco_types::ToolName::*.as_str()`.
const TOOL_EDIT: &str = "Edit";
const TOOL_READ: &str = "Read";
const TOOL_WEB_FETCH: &str = "WebFetch";

/// Prefix used on `WebFetch(domain:HOST)` rule content.
const DOMAIN_PREFIX: &str = "domain:";

/// Inputs collected by the CLI / session bootstrap and handed to the adapter.
///
/// All paths must be absolute. The adapter does not perform I/O — callers
/// (e.g., `app/cli/session_runtime`) probe filesystem state up-front and pass
/// the results in.
#[derive(Debug, Clone)]
pub struct AdapterInputs<'a> {
    /// Sandbox settings from `settings.json` (already merged across sources).
    pub settings: &'a SandboxSettings,
    /// User-facing sandbox mode (from settings + env). Drives [`EnforcementLevel`].
    pub mode: SandboxMode,
    /// Directory used to resolve settings-relative paths in permission rules.
    /// Per-source resolution collapses to the project's settings root.
    pub settings_root: &'a Path,
    /// Original working directory at session start.
    pub original_cwd: &'a Path,
    /// Current working directory (may differ from `original_cwd` after `cd`).
    pub current_cwd: &'a Path,
    /// Allow-list permission rule strings (`["Edit(/foo)", "WebFetch(domain:x.com)"]`).
    pub permission_allow_rules: &'a [String],
    /// Deny-list permission rule strings.
    pub permission_deny_rules: &'a [String],
    /// `--add-dir` / `/add-dir` directories. Always become writable roots.
    pub additional_directories: &'a [PathBuf],
    /// Claude's per-session temp dir (used by Shell.ts CWD tracking).
    pub coco_temp_dir: &'a Path,
    /// Path to the project's `~/.coco/settings.json` analogue (used to deny
    /// writes that would let the agent edit its own permissions).
    pub settings_files: &'a [PathBuf],
    /// Worktree main-repo `.git` path when `current_cwd` is a git worktree.
    /// Resolved once at session start by the bootstrap layer (worktree status
    /// doesn't change mid-session). `None` outside worktrees.
    pub worktree_main_repo: Option<&'a Path>,
    /// Per-source view of permission **allow** rules — drives the
    /// `network.allow_managed_domains_only` gate. When `None`, the gate
    /// degrades to "all sources contribute" (matches TS behavior when
    /// the flag itself is unset). Production callers (`session_runtime`)
    /// always populate; SDK / test harnesses can omit.
    ///
    /// Deny rules are not tagged because TS always honors all-source
    /// denials regardless of the gate (security floor).
    ///
    /// TS parity: `sandbox-adapter.ts:152-164` `shouldAllowManagedDomainsOnly`.
    pub sourced_permission_allow_rules: Option<&'a [coco_config::SourcedRule]>,
    /// Per-source view of `sandbox.filesystem.allow_read` paths — drives
    /// the `filesystem.allow_managed_read_paths_only` gate. Same `None`
    /// semantics as [`Self::sourced_permission_rules`].
    ///
    /// TS parity: `sandbox-adapter.ts:343-347` `shouldAllowManagedReadPathsOnly`.
    pub sourced_filesystem_allow_read: Option<&'a [(coco_config::SettingSource, Vec<PathBuf>)]>,
}

/// Output produced by the adapter and consumed by [`crate::SandboxState`].
#[derive(Debug, Clone)]
pub struct AdapterOutput {
    /// Enforcement level resolved from `mode` plus settings.
    pub enforcement: EnforcementLevel,
    /// Possibly-mutated settings (with rule-derived domain lists folded in).
    pub settings: SandboxSettings,
    /// Runtime restriction config to install on `SandboxState`.
    pub config: SandboxConfig,
}

/// Build the runtime sandbox config from settings, permission rules, and
/// session context. Pure synchronous logic — no I/O.
pub fn build_runtime_config(inputs: AdapterInputs<'_>) -> AdapterOutput {
    let mut settings = inputs.settings.clone();

    // ── Network: extract domains from WebFetch rules ──
    fold_webfetch_domains_into_network(
        &mut settings,
        inputs.permission_allow_rules,
        inputs.permission_deny_rules,
        inputs.sourced_permission_allow_rules,
    );

    // ── Filesystem: collect writable roots + deny paths + allow_read carve-outs ──
    let allow_write_roots = collect_writable_roots(&inputs, &settings);
    let deny_write_paths = collect_deny_write_paths(&inputs, &settings);
    let (denied_read_paths, denied_read_globs) = collect_deny_read_paths(&inputs, &settings);
    let allowed_read_paths = collect_allow_read_paths(&inputs, &settings);

    let enforcement = EnforcementLevel::from(inputs.mode);

    let config = SandboxConfig {
        enforcement,
        writable_roots: allow_write_roots,
        denied_paths: Vec::new(),
        denied_read_paths,
        denied_read_globs,
        glob_scan_max_depth: settings.mandatory_deny_search_depth,
        allowed_read_paths,
        deny_write_paths,
        allow_git_config: settings.filesystem.allow_git_config,
        allow_network: matches!(inputs.mode, SandboxMode::FullAccess)
            || !network_isolated(&settings),
        proxy_active: false,
        extra_bind_ro: Vec::new(),
        weaker_network_isolation: settings.enable_weaker_network_isolation,
        allow_pty: settings.allow_pty,
    };

    AdapterOutput {
        enforcement,
        settings,
        config,
    }
}

/// Extract `domain:HOST` from `WebFetch(domain:HOST)` rules and fold into
/// `settings.network.{allowed,denied}_domains`.
///
/// TS lines 188–219. When `network.allow_managed_domains_only` is set
/// AND the caller provided per-source rules, only `policy_settings`-sourced
/// allow rules contribute domains. Deny rules are honored from all sources
/// regardless (security floor: enterprise denials must always win).
///
/// `sourced_rules` falling back to `None` (test harness / SDK without
/// source provenance) degrades to "all sources contribute" — matching
/// TS behavior when the gate is off.
fn fold_webfetch_domains_into_network(
    settings: &mut SandboxSettings,
    allow_rules: &[String],
    deny_rules: &[String],
    sourced_allow_rules: Option<&[coco_config::SourcedRule]>,
) {
    let policy_only_allow = settings.network.allow_managed_domains_only;

    let mut allowed: HashSet<String> = settings.network.allowed_domains.iter().cloned().collect();
    match (policy_only_allow, sourced_allow_rules) {
        (true, Some(rules)) => {
            // TS `shouldAllowManagedDomainsOnly()` ON: only policy-source
            // allow rules contribute. The flat `allow_rules` list is ignored
            // because we can't determine its source.
            for rule in rules {
                if !matches!(rule.source, coco_config::SettingSource::Policy) {
                    continue;
                }
                if let Some(domain) = extract_webfetch_domain(&rule.rule) {
                    allowed.insert(domain);
                }
            }
        }
        _ => {
            for rule in allow_rules {
                if let Some(domain) = extract_webfetch_domain(rule) {
                    allowed.insert(domain);
                }
            }
        }
    }
    settings.network.allowed_domains = allowed.into_iter().collect();

    // Deny rules: honored from all sources regardless of the gate (TS
    // `sandbox-adapter.ts:160-163` — denied domains always respected).
    let mut denied: HashSet<String> = settings.network.denied_domains.iter().cloned().collect();
    for rule in deny_rules {
        if let Some(domain) = extract_webfetch_domain(rule) {
            denied.insert(domain);
        }
    }
    settings.network.denied_domains = denied.into_iter().collect();
}

/// Parse a permission rule and return the domain if it's `WebFetch(domain:HOST)`.
fn extract_webfetch_domain(rule: &str) -> Option<String> {
    let parsed = coco_types::parse_rule_pattern(rule);
    if parsed.tool_pattern != TOOL_WEB_FETCH {
        return None;
    }
    let content = parsed.rule_content?;
    let domain = content.strip_prefix(DOMAIN_PREFIX)?.trim();
    if domain.is_empty() {
        None
    } else {
        Some(domain.to_string())
    }
}

/// Collect writable roots from CWD, additional dirs, settings, and permission rules.
fn collect_writable_roots(
    inputs: &AdapterInputs<'_>,
    settings: &SandboxSettings,
) -> Vec<WritableRoot> {
    let mut paths: Vec<PathBuf> = Vec::new();

    // CWD + Claude temp dir always writable (TS line 225).
    paths.push(inputs.current_cwd.to_path_buf());
    paths.push(inputs.coco_temp_dir.to_path_buf());

    // --add-dir directories.
    paths.extend(inputs.additional_directories.iter().cloned());

    // Worktree main-repo path (so `git` can write `.git/index.lock`, etc.)
    if let Some(main_repo) = inputs.worktree_main_repo
        && main_repo != inputs.current_cwd
    {
        paths.push(main_repo.to_path_buf());
    }

    // Permission rule allow paths: Edit(/foo) → writable.
    for rule in inputs.permission_allow_rules {
        if let Some(p) = extract_path_for_tool(rule, TOOL_EDIT) {
            paths.push(resolve_permission_rule_path(&p, inputs.settings_root));
        }
    }

    // Settings-section paths (different resolution semantics — see #30067).
    for p in &settings.filesystem.allow_write {
        paths.push(resolve_filesystem_path(p, inputs.settings_root));
    }

    dedup_paths(&mut paths);
    paths.into_iter().map(WritableRoot::new).collect()
}

/// Collect deny-write paths from settings.json files, `.coco/skills`,
/// permission rule denies, and settings filesystem section.
fn collect_deny_write_paths(
    inputs: &AdapterInputs<'_>,
    settings: &SandboxSettings,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();

    // Settings.json files always denied (TS lines 232–245).
    paths.extend(inputs.settings_files.iter().cloned());

    // Settings file in current cwd if different from original.
    if inputs.current_cwd != inputs.original_cwd {
        paths.push(inputs.current_cwd.join(".coco").join("settings.json"));
        paths.push(inputs.current_cwd.join(".coco").join("settings.local.json"));
    }

    // Project skills in original + current cwd.
    paths.push(inputs.original_cwd.join(".coco").join("skills"));
    if inputs.current_cwd != inputs.original_cwd {
        paths.push(inputs.current_cwd.join(".coco").join("skills"));
    }

    // Permission rule deny paths: Edit(/foo) → deny.
    for rule in inputs.permission_deny_rules {
        if let Some(p) = extract_path_for_tool(rule, TOOL_EDIT) {
            paths.push(resolve_permission_rule_path(&p, inputs.settings_root));
        }
    }

    // Settings-section deny_write paths.
    for p in &settings.filesystem.deny_write {
        paths.push(resolve_filesystem_path(p, inputs.settings_root));
    }

    dedup_paths(&mut paths);
    paths
}

/// Collect deny-read paths from permission Read denies + settings.
///
/// Returns `(literal_paths, glob_patterns)`: entries that contain glob
/// metacharacters (`*`, `?`, `[`) flow through `glob_patterns` and get
/// expanded at platform-wrap time via [`crate::glob_expansion::expand`];
/// pure paths flow through `literal_paths` unchanged. Mirrors the
/// codex-rs `glob_scan_max_depth` deny-read behavior.
fn collect_deny_read_paths(
    inputs: &AdapterInputs<'_>,
    settings: &SandboxSettings,
) -> (Vec<PathBuf>, Vec<String>) {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut globs: Vec<String> = Vec::new();

    // Permission rule deny paths: Read(/foo) → deny_read.
    for rule in inputs.permission_deny_rules {
        if let Some(p) = extract_path_for_tool(rule, TOOL_READ) {
            if crate::glob_expansion::looks_like_glob(&p) {
                globs.push(p);
            } else {
                paths.push(resolve_permission_rule_path(&p, inputs.settings_root));
            }
        }
    }

    // Settings-section deny_read paths. Filesystem paths are stored as
    // `PathBuf` upstream; preserve the original `to_string_lossy()` form
    // so the glob patterns match against unresolved relative paths.
    for p in &settings.filesystem.deny_read {
        let raw = p.to_string_lossy();
        if crate::glob_expansion::looks_like_glob(&raw) {
            globs.push(raw.into_owned());
        } else {
            paths.push(resolve_filesystem_path(p, inputs.settings_root));
        }
    }

    dedup_paths(&mut paths);
    globs.sort();
    globs.dedup();
    (paths, globs)
}

/// Collect allow-read carve-out paths from settings.
///
/// TS parity: `entrypoints/sandboxTypes.ts:71-77` `allowRead` — paths that
/// re-allow reading even when shadowed by a `deny_read` entry or a
/// permission `Read(/foo)` deny rule.
///
/// When `filesystem.allow_managed_read_paths_only` is set AND the caller
/// provided per-source data, only `policy_settings`-sourced `allow_read`
/// entries contribute. Without sourced data the gate degrades to
/// "all sources contribute" (matches TS when the gate is off).
///
/// TS parity: `sandbox-adapter.ts:343-347` `shouldAllowManagedReadPathsOnly`.
fn collect_allow_read_paths(
    inputs: &AdapterInputs<'_>,
    settings: &SandboxSettings,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = Vec::new();
    let policy_only = settings.filesystem.allow_managed_read_paths_only;

    match (policy_only, inputs.sourced_filesystem_allow_read) {
        (true, Some(groups)) => {
            // TS `shouldAllowManagedReadPathsOnly()` ON: only policy-source
            // `sandbox.filesystem.allow_read` entries contribute.
            for (source, paths_from_source) in groups {
                if !matches!(source, coco_config::SettingSource::Policy) {
                    continue;
                }
                for p in paths_from_source {
                    paths.push(resolve_filesystem_path(p, inputs.settings_root));
                }
            }
        }
        _ => {
            // Default path: pull from the merged settings — every source
            // contributed during merge.
            for p in &settings.filesystem.allow_read {
                paths.push(resolve_filesystem_path(p, inputs.settings_root));
            }
        }
    }

    dedup_paths(&mut paths);
    paths
}

/// Parse a permission rule and return the path content if it matches `tool_name`.
fn extract_path_for_tool(rule: &str, tool_name: &str) -> Option<String> {
    let parsed = coco_types::parse_rule_pattern(rule);
    if parsed.tool_pattern != tool_name {
        return None;
    }
    parsed.rule_content
}

/// Resolve a path pattern from a permission rule.
///
/// Permission-rule conventions (TS `resolvePathPatternForSandbox`):
/// - `//path` → `/path` (absolute from filesystem root)
/// - `/path` → `<settings_root>/path` (settings-relative)
/// - `~/path` → home-relative
/// - `./path` or `path` → cwd-relative (left as-is for sandbox runtime)
pub fn resolve_permission_rule_path(pattern: &str, settings_root: &Path) -> PathBuf {
    if let Some(rest) = pattern.strip_prefix("//") {
        // `//foo/**` → `/foo/**` — absolute root.
        return PathBuf::from(format!("/{rest}"));
    }
    if let Some(rest) = pattern.strip_prefix('/') {
        // Single leading slash → settings-relative.
        return settings_root.join(rest);
    }
    expand_tilde(pattern).unwrap_or_else(|| PathBuf::from(pattern))
}

/// Resolve a path from `sandbox.filesystem.*` settings.
///
/// Filesystem-settings conventions (TS `resolveSandboxFilesystemPath`,
/// fix for #30067): standard path semantics — absolute paths stay absolute,
/// `~/` is expanded, relative resolves to `settings_root`.
pub fn resolve_filesystem_path(pattern: &Path, settings_root: &Path) -> PathBuf {
    let s = pattern.to_string_lossy();
    // Legacy escape: `//x` → `/x` for users who worked around #30067.
    if let Some(rest) = s.strip_prefix("//") {
        return PathBuf::from(format!("/{rest}"));
    }
    if let Some(expanded) = expand_tilde(&s) {
        return expanded;
    }
    if pattern.is_absolute() {
        return pattern.to_path_buf();
    }
    settings_root.join(pattern)
}

/// Expand `~` and `~/path` to the user's home directory.
fn expand_tilde(pattern: &str) -> Option<PathBuf> {
    if pattern == "~" {
        return dirs_home();
    }
    let rest = pattern.strip_prefix("~/")?;
    Some(dirs_home()?.join(rest))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Stable de-dup that preserves first occurrence (`Vec::dedup` only collapses
/// adjacent duplicates).
fn dedup_paths(paths: &mut Vec<PathBuf>) {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    paths.retain(|p| seen.insert(p.clone()));
}

/// Whether the sandbox settings indicate network isolation should apply.
///
/// TS isolates network whenever the sandbox is enabled and the user has not
/// explicitly opted into open network (`sandbox-adapter.ts` always routes
/// egress through the per-domain filter when `isSandboxingEnabled()`). The
/// only opt-out is the coarse `allow_network` toggle.
///
/// Network isolation is deliberately decoupled from [`NetworkMode`]:
/// `NetworkMode` gates *HTTP methods* (Full = all methods, Limited =
/// GET/HEAD/OPTIONS) and flows into the [`DomainFilter`] when the proxy starts;
/// it must never act as an on/off switch. Keying isolation off `mode != Full`
/// was a category error that, because `NetworkMode` defaults to `Full`, left
/// the dominant config (sandbox on, no `network.mode`) with `allow_network ==
/// true` — unrestricted egress, the exact escape this guards against.
fn network_isolated(settings: &SandboxSettings) -> bool {
    settings.enabled && !settings.allow_network
}

/// Compute paths under `cwd` that look like a planted bare-repo (HEAD,
/// objects, refs, hooks, config) and DON'T currently exist. Returned list is
/// scrubbed by [`scrub_bare_repo_files`] after each sandboxed command —
/// mitigation for anthropics/claude-code#29316.
///
/// Existing files are excluded (callers should add them to deny_write_paths
/// instead).
pub fn bare_repo_scrub_paths(cwd: &Path, original_cwd: &Path) -> Vec<PathBuf> {
    const FILES: &[&str] = &["HEAD", "objects", "refs", "hooks", "config"];
    let mut out = Vec::new();
    let dirs: &[&Path] = if cwd == original_cwd {
        &[original_cwd]
    } else {
        &[original_cwd, cwd]
    };
    for dir in dirs {
        for f in FILES {
            let p = dir.join(f);
            if !p.exists() {
                out.push(p);
            }
        }
    }
    out
}

/// Best-effort delete of planted bare-repo files. Errors are logged at debug
/// level and never propagated — `ENOENT` is the expected common case.
pub fn scrub_bare_repo_files(paths: &[PathBuf]) {
    for p in paths {
        let res = if p.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        };
        if let Err(e) = res
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::debug!(path = %p.display(), error = %e, "bare-repo scrub failed");
        }
    }
}

/// Detect whether `cwd` is a git worktree and return the main repo path.
///
/// In a worktree, `<cwd>/.git` is a file containing `gitdir: <main>/.git/worktrees/<name>`.
/// Returns the main repo path (not its `.git` dir) so callers can add it as a writable root.
pub fn detect_worktree_main_repo(cwd: &Path) -> Option<PathBuf> {
    let git_path = cwd.join(".git");
    let content = std::fs::read_to_string(&git_path).ok()?;
    let first = content.lines().next()?.trim();
    let gitdir = first.strip_prefix("gitdir:")?.trim();
    if gitdir.is_empty() {
        return None;
    }
    let gitdir_path = PathBuf::from(gitdir);
    let resolved = if gitdir_path.is_relative() {
        cwd.join(&gitdir_path)
    } else {
        gitdir_path
    };
    // Match `/.git/worktrees/` rather than indexOf('.git') — the latter
    // false-matches paths like `/home/user/.github-projects/...`.
    let marker = format!(
        "{sep}.git{sep}worktrees{sep}",
        sep = std::path::MAIN_SEPARATOR
    );
    let resolved_str = resolved.to_string_lossy();
    let idx = resolved_str.rfind(&marker)?;
    if idx == 0 {
        return None;
    }
    Some(PathBuf::from(&resolved_str[..idx]))
}

/// Reason a user-enabled sandbox cannot run, for surfacing to the user once at
/// startup. Returns `None` if sandbox isn't requested, or if it's working.
pub fn sandbox_unavailable_reason(
    settings: &SandboxSettings,
    platform_supported: bool,
    platform_in_enabled_list: bool,
    missing_deps: &[String],
) -> Option<String> {
    if !settings.enabled {
        return None;
    }
    let platform = current_platform_str();
    if !platform_supported {
        if platform == "wsl" {
            return Some("sandbox.enabled is set but WSL1 is not supported (requires WSL2)".into());
        }
        return Some(format!(
            "sandbox.enabled is set but {platform} is not supported (requires macOS, Linux, or WSL2)"
        ));
    }
    if !platform_in_enabled_list {
        return Some(format!(
            "sandbox.enabled is set but {platform} is not in sandbox.enabledPlatforms"
        ));
    }
    if !missing_deps.is_empty() {
        let hint = if platform == "macos" {
            "run /sandbox or /doctor for details"
        } else {
            "install missing tools (e.g. apt install bubblewrap socat) or run /sandbox for details"
        };
        return Some(format!(
            "sandbox.enabled is set but dependencies are missing: {} · {}",
            missing_deps.join(", "),
            hint
        ));
    }
    None
}

fn current_platform_str() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unknown"
    }
}

#[cfg(test)]
#[path = "adapter.test.rs"]
mod tests;
