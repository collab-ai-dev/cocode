//! Sandbox settings.
//!
//! Single source of truth for sandbox configuration consumed by both
//! `~/.coco/settings.json` deserialization and the sandbox runtime
//! (`coco-sandbox`). The runtime adapter takes a `&SandboxSettings` and
//! produces its own platform-bound `SandboxConfig` (which is a separate
//! type owned by the sandbox crate — adapter output, not user-facing).
//!
//! Two coco-rs-specific high-level fields extend the base schema:
//!
//! - `mode`: posture enum (ReadOnly/WorkspaceWrite/FullAccess/ExternalSandbox).
//!   Distinct from `enabled`: `enabled` is the feature gate, `mode` controls
//!   the policy applied when the gate is on.
//! - `allow_network`: coarse "all or nothing" toggle. Distinct from
//!   `network.allowed_domains` which is a fine-grained allowlist.
//!
//! ## Env overrides
//!
//! Four scalar fields can be overridden via env vars:
//! `COCO_SANDBOX_MODE`, `COCO_SANDBOX_EXCLUDED_COMMANDS`,
//! `COCO_SANDBOX_ALLOW_NETWORK`, `COCO_SANDBOX_FAIL_IF_UNAVAILABLE`.
//! The rich nested fields (`filesystem`, `network`) have no env paths;
//! they only come from settings.json.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use coco_types::SandboxMode;
use serde::Deserialize;
use serde::Serialize;

use crate::env::EnvKey;
use crate::env::EnvSnapshot;
use crate::settings::Settings;
use crate::settings::SettingsWithSource;
use crate::settings::source::SettingSource;

/// Whether sandbox bypass was requested for a specific command.
///
/// Used instead of a bare `bool` so callsites are self-documenting:
/// `is_sandboxed(cmd, SandboxBypass::Requested)` vs `is_sandboxed(cmd, true)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBypass {
    /// Bypass not requested; normal sandboxing applies.
    No,
    /// Bypass requested via `dangerouslyDisableSandbox` parameter.
    Requested,
}

impl SandboxBypass {
    /// Create from a boolean flag (for interop with JSON input parsing).
    pub fn from_flag(dangerously_disable_sandbox: bool) -> Self {
        if dangerously_disable_sandbox {
            Self::Requested
        } else {
            Self::No
        }
    }
}

/// Network access mode controlling HTTP method enforcement.
///
/// In `Limited` mode, only safe HTTP methods (GET, HEAD, OPTIONS) are allowed.
/// CONNECT tunnels and SOCKS5 are blocked (cannot inspect methods through tunnels).
/// In `Full` mode (default), all methods are permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    /// Full network access: all HTTP methods allowed, CONNECT tunneled, SOCKS5 active.
    #[default]
    Full,
    /// Limited (read-only) access: only GET/HEAD/OPTIONS. CONNECT and SOCKS5 blocked.
    Limited,
}

impl NetworkMode {
    /// Check if an HTTP method is allowed in this mode.
    pub fn allows_method(&self, method: &str) -> bool {
        match self {
            Self::Full => true,
            Self::Limited => matches!(method, "GET" | "HEAD" | "OPTIONS"),
        }
    }
}

/// Filesystem access configuration for the sandbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FilesystemConfig {
    /// Paths allowed for write access (defaults to CWD).
    #[serde(default)]
    pub allow_write: Vec<PathBuf>,
    /// Paths explicitly denied for writing.
    #[serde(default)]
    pub deny_write: Vec<PathBuf>,
    /// Paths explicitly denied for reading.
    #[serde(default)]
    pub deny_read: Vec<PathBuf>,
    /// Paths to re-allow reading within `deny_read` regions.
    /// Takes precedence over `deny_read` for matching paths.
    ///
    #[serde(default)]
    pub allow_read: Vec<PathBuf>,
    /// Allow writing to `.git/config` and `~/.gitconfig`.
    #[serde(default)]
    pub allow_git_config: bool,
    /// When `true` (and set in managed/policy settings), only `allow_read`
    /// paths from `policy_settings` source are honored. User, project,
    /// local, and flag settings `allow_read` entries are ignored.
    /// `deny_read` entries from all sources are still respected.
    ///
    /// Enforcement requires per-source rule plumbing through
    /// [`crate::SettingsWithSource::sourced_filesystem_allow_read`] —
    /// when the adapter receives flat unsourced rules, the gate degrades
    /// to "all sources contribute" (safe default).
    ///
    #[serde(default)]
    pub allow_managed_read_paths_only: bool,
}

/// Network access configuration for the sandbox.
///
/// The `denied_domains` and `mode` fields are coco-rs extensions that
/// gracefully degrade when consumed by clients that ignore extra fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct NetworkConfig {
    /// Network access mode (Full or Limited).
    #[serde(default)]
    pub mode: NetworkMode,
    /// Domain allow list. When non-empty, only these domains are permitted.
    /// Empty means all domains allowed (unless denied).
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Domain deny list (takes precedence over allow list).
    #[serde(default)]
    pub denied_domains: Vec<String>,
    /// Unix socket paths allowed inside the sandbox.
    #[serde(default)]
    pub allow_unix_sockets: Vec<PathBuf>,
    /// Allow all Unix sockets (disables socket blocking).
    #[serde(default)]
    pub allow_all_unix_sockets: bool,
    /// Allow binding to localhost ports inside the sandbox.
    #[serde(default)]
    pub allow_local_binding: bool,
    /// Fixed HTTP proxy port (auto-assigned if `None`).
    #[serde(default)]
    pub http_proxy_port: Option<u16>,
    /// Fixed SOCKS proxy port (auto-assigned if `None`).
    #[serde(default)]
    pub socks_proxy_port: Option<u16>,
    /// MITM proxy configuration for HTTPS inspection.
    #[serde(default)]
    pub mitm_proxy: Option<MitmProxyConfig>,
    /// Block connections to non-public IP addresses (SSRF prevention).
    ///
    /// When enabled, the proxy filter rejects connections to loopback,
    /// private (RFC 1918), link-local, CGNAT, TEST-NET, and other
    /// reserved IP ranges.
    #[serde(default)]
    pub block_non_public_ips: bool,
    /// When `true` (and set in managed/policy settings), only
    /// `allowed_domains` and `WebFetch(domain:…)` allow rules from
    /// `policy_settings` are honored. User, project, local, and flag
    /// settings allow-domains are ignored. Denied domains from all
    /// sources are still respected.
    ///
    /// Enforcement requires per-source rule plumbing through
    /// [`crate::SettingsWithSource::sourced_permission_rules`] — when
    /// the adapter receives flat unsourced rules, the gate degrades to
    /// "all sources contribute" (safe default).
    ///
    #[serde(default)]
    pub allow_managed_domains_only: bool,
}

/// MITM proxy configuration for HTTPS traffic inspection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MitmProxyConfig {
    /// Unix socket path for the MITM proxy.
    pub socket_path: PathBuf,
    /// Domains to intercept via MITM.
    #[serde(default)]
    pub domains: Vec<String>,
}

/// Per-command violation ignore patterns.
///
/// Keys are command patterns (or `"*"` for global).
/// Values are lists of violation operations to ignore.
pub type IgnoreViolationsConfig = HashMap<String, Vec<String>>;

/// Custom ripgrep configuration for sandbox-runtime ripgrep dispatch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RipgrepConfig {
    /// Path to the ripgrep binary.
    pub command: String,
    /// Default arguments prepended to ripgrep invocations.
    #[serde(default)]
    pub args: Vec<String>,
}

/// Sandbox settings.
///
/// Deserialized directly from `~/.coco/settings.json`'s `sandbox` block;
/// consumed by both the high-level posture decision (`mode`) and the
/// platform-specific runtime (filesystem/network/etc.).
///
/// # Default
///
/// `enabled: false`. When false, commands run unsandboxed. The platform
/// runtime is constructed lazily and only when `enabled == true` AND the
/// bootstrap gates pass (supported platform, deps available, etc.).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxSettings {
    // === coco-rs high-level posture (not in TS) ===
    /// Sandbox enforcement posture: ReadOnly / WorkspaceWrite / FullAccess /
    /// ExternalSandbox. Drives auto-mode classifier + permission shortcuts.
    /// Distinct from `enabled` which is the feature gate.
    #[serde(default = "default_mode")]
    pub mode: SandboxMode,

    /// Coarse "allow all network" toggle. Distinct from `network.allowed_domains`
    /// (fine-grained allowlist). When `true`, network isolation is bypassed.
    #[serde(default)]
    pub allow_network: bool,

    /// Enable sandbox mode.
    ///
    /// When `false` (default), commands run directly without sandbox wrapping.
    /// When `true`, commands are wrapped with platform-specific sandbox
    /// (Seatbelt on macOS, bubblewrap on Linux).
    #[serde(default)]
    pub enabled: bool,

    /// Hard-fail at startup when `enabled == true` but sandbox can't run.
    #[serde(default)]
    pub fail_if_unavailable: bool,

    /// Auto-approve bash commands when running in sandbox mode.
    #[serde(default = "default_true")]
    pub auto_allow_bash_if_sandboxed: bool,

    /// Allow the `dangerously_disable_sandbox` parameter to bypass sandbox.
    #[serde(default = "default_true")]
    pub allow_unsandboxed_commands: bool,

    /// Platforms where sandbox may be enabled.
    #[serde(default = "default_enabled_platforms")]
    pub enabled_platforms: Vec<String>,

    /// Commands excluded from sandbox wrapping.
    ///
    /// Supports three pattern types:
    /// - Exact: `"git"` matches `git` and `git <subcommand>`
    /// - Prefix: `"npm:*"` matches `npm` and all subcommands
    /// - Wildcard: `"npm run *"` matches `npm run build`, etc.
    #[serde(default)]
    pub excluded_commands: Vec<String>,

    /// Filesystem access configuration.
    #[serde(default)]
    pub filesystem: FilesystemConfig,

    /// Network access configuration.
    #[serde(default)]
    pub network: NetworkConfig,

    /// Per-command violation ignore patterns.
    #[serde(default)]
    pub ignore_violations: IgnoreViolationsConfig,

    /// Enable weaker nested sandbox (for Docker/WSL environments).
    #[serde(default)]
    pub enable_weaker_nested_sandbox: bool,

    /// Enable weaker network isolation (allow trustd.agent for Go TLS on macOS).
    #[serde(default)]
    pub enable_weaker_network_isolation: bool,

    /// Allow pseudo-terminal access inside the sandbox (macOS).
    /// Mirrors `SandboxConfig`'s `default_true` so an omitted key and
    /// `..Default::default()` agree with the runtime default (PTY allowed
    /// unless explicitly disabled — matches the codex-rs seatbelt base policy).
    #[serde(default = "default_true")]
    pub allow_pty: bool,

    /// Directory search depth for mandatory deny path discovery (Linux).
    #[serde(default = "default_mandatory_deny_search_depth")]
    pub mandatory_deny_search_depth: i32,

    /// Custom ripgrep configuration (sandbox-runtime ripgrep dispatch).
    #[serde(default)]
    pub ripgrep: Option<RipgrepConfig>,
}

fn default_mode() -> SandboxMode {
    SandboxMode::ReadOnly
}

fn default_true() -> bool {
    true
}

fn default_enabled_platforms() -> Vec<String> {
    vec![
        "macos".to_string(),
        "linux".to_string(),
        "windows".to_string(),
    ]
}

fn default_mandatory_deny_search_depth() -> i32 {
    3
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            allow_network: false,
            enabled: false,
            fail_if_unavailable: false,
            auto_allow_bash_if_sandboxed: true,
            allow_unsandboxed_commands: true,
            enabled_platforms: default_enabled_platforms(),
            excluded_commands: Vec::new(),
            filesystem: FilesystemConfig::default(),
            network: NetworkConfig::default(),
            ignore_violations: HashMap::new(),
            enable_weaker_nested_sandbox: false,
            enable_weaker_network_isolation: false,
            allow_pty: true,
            mandatory_deny_search_depth: default_mandatory_deny_search_depth(),
            ripgrep: None,
        }
    }
}

impl SandboxSettings {
    /// Creates settings with sandbox enabled.
    pub fn enabled() -> Self {
        Self {
            enabled: true,
            ..Default::default()
        }
    }

    /// Creates settings with sandbox disabled (same as default).
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Resolve the effective settings from `Settings` + env-var overrides.
    ///
    /// `settings.sandbox` is already a fully-defaulted `SandboxSettings`
    /// (every field has a `#[serde(default)]`). This step layers the four
    /// scalar env overrides on top — `mode`, `excluded_commands`,
    /// `allow_network`, `fail_if_unavailable`. Rich nested fields
    /// (`filesystem`, `network`) intentionally have no env path:
    /// pulling structured data through a single env var is a footgun.
    pub fn resolve(settings: &Settings, env: &EnvSnapshot) -> Self {
        let mut config = settings.sandbox.clone();

        if let Some(raw) = env.get(EnvKey::CocoSandboxMode) {
            config.mode = match raw {
                "workspace_write" | "workspace-write" | "strict" => SandboxMode::WorkspaceWrite,
                "full_access" | "full-access" | "none" => SandboxMode::FullAccess,
                "external_sandbox" | "external-sandbox" => SandboxMode::ExternalSandbox,
                _ => SandboxMode::ReadOnly,
            };
        }
        if let Some(raw) = env.get(EnvKey::CocoSandboxExcludedCommands) {
            config.excluded_commands = raw
                .split([':', ','])
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        if env.is_truthy(EnvKey::CocoSandboxAllowNetwork) {
            config.allow_network = true;
        }
        if env.is_truthy(EnvKey::CocoSandboxFailIfUnavailable) {
            config.fail_if_unavailable = true;
        }

        config
    }

    /// Check if a command should run in sandbox mode.
    ///
    /// Returns `false` (no sandbox) if:
    /// 1. Sandbox is disabled (`!self.enabled`)
    /// 2. Bypass requested and allowed
    /// 3. Command is empty
    /// 4. Command matches an excluded prefix
    pub fn is_sandboxed(&self, command: &str, bypass: SandboxBypass) -> bool {
        if !self.enabled {
            return false;
        }

        if matches!(bypass, SandboxBypass::Requested) && self.allow_unsandboxed_commands {
            return false;
        }

        if command.trim().is_empty() {
            return false;
        }

        if self.is_excluded_command(command) {
            return false;
        }

        true
    }

    /// Check if a command matches any excluded command pattern using BFS
    /// variant expansion.
    ///
    /// For each command segment, builds variants by:
    /// 1. Original token
    /// 2. Strip leading env assignments (`FOO=bar npm` -> `npm`)
    /// 3. Extract basename (`/usr/bin/npm` -> `npm`)
    /// 4. Strip safe wrappers (`timeout 30 cmd`, `nice -n 5 cmd`, `nohup cmd`)
    ///
    /// Matches variants against patterns:
    /// - `"cmd:*"` — prefix match (`npm:*` matches `npm install`)
    /// - `"cmd *"` — wildcard glob (`npm run *` matches `npm run build`)
    /// - `"cmd"` — exact first-word match (`git` matches `git status`)
    pub fn is_excluded_command(&self, command: &str) -> bool {
        if self.excluded_commands.is_empty() {
            return false;
        }

        let trimmed = command.trim();
        let variants = build_command_variants(trimmed);

        self.excluded_commands.iter().any(|pattern| {
            let pattern = pattern.trim();
            variants
                .iter()
                .any(|variant| matches_exclusion_pattern(variant, pattern))
        })
    }

    /// Check if the current platform is in the enabled platforms list.
    pub fn is_platform_enabled(&self) -> bool {
        let current = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else {
            return false;
        };
        self.enabled_platforms
            .iter()
            .any(|p| p.eq_ignore_ascii_case(current))
    }
}

/// Build command variants via BFS expansion for exclusion matching.
///
/// Given a command string, produces a set of normalized forms:
/// 1. The original trimmed command
/// 2. With leading env assignments stripped (`A=1 B=2 npm install` -> `npm install`)
/// 3. With the first token replaced by its basename (`/usr/bin/npm install` -> `npm install`)
/// 4. With one safe wrapper peeled (`timeout 30 cmd` -> `cmd`, `nice -n 5 cmd` -> `cmd`,
///    `time cmd` / `nohup cmd` -> `cmd`)
///
/// Wrapper, env, and basename strippers compose via the BFS queue, so an
/// interleaved input like `timeout 300 FOO=bar /usr/bin/bazel run` reaches
/// `bazel run` after multiple iterations — matching the TS fixed-point
/// loop in `shouldUseSandbox.ts:82-101`.
fn build_command_variants(command: &str) -> Vec<String> {
    use std::collections::HashSet;

    let mut variants = HashSet::new();
    let mut queue = std::collections::VecDeque::new();

    variants.insert(command.to_string());
    queue.push_back(command.to_string());

    while let Some(current) = queue.pop_front() {
        // Variant: strip leading env assignments (KEY=VALUE prefix)
        let stripped = strip_env_prefix(&current);
        if stripped != current && variants.insert(stripped.clone()) {
            queue.push_back(stripped);
        }

        // Variant: extract basename of first token
        let basenamed = replace_first_token_with_basename(&current);
        if basenamed != current && variants.insert(basenamed.clone()) {
            queue.push_back(basenamed);
        }

        // Variant: strip one leading safe wrapper (timeout / time / nice / nohup)
        if let Some(unwrapped) = strip_safe_wrapper(&current)
            && unwrapped != current
            && variants.insert(unwrapped.clone())
        {
            queue.push_back(unwrapped);
        }
    }

    variants.into_iter().collect()
}

/// Strip leading `KEY=VALUE` assignments from a command.
///
/// `FOO=bar BAZ=1 npm install` -> `npm install`
fn strip_env_prefix(command: &str) -> String {
    let mut rest = command;
    loop {
        rest = rest.trim_start();
        // Check if the next token looks like KEY=VALUE
        if let Some(space_idx) = rest.find(|c: char| c.is_whitespace()) {
            let token = &rest[..space_idx];
            if token.contains('=') && !token.starts_with('=') && !token.starts_with('-') {
                rest = &rest[space_idx..];
                continue;
            }
        }
        break;
    }
    rest.trim_start().to_string()
}

/// Replace the first token of a command with its basename.
///
/// `/usr/bin/npm install` -> `npm install`
/// `./node_modules/.bin/jest test` -> `jest test`
fn replace_first_token_with_basename(command: &str) -> String {
    let trimmed = command.trim_start();
    let (first, rest) = match trimmed.find(|c: char| c.is_whitespace()) {
        Some(idx) => (&trimmed[..idx], &trimmed[idx..]),
        None => (trimmed, ""),
    };

    if let Some(basename) = Path::new(first).file_name().and_then(|n| n.to_str())
        && basename != first
    {
        return format!("{basename}{rest}");
    }
    command.to_string()
}

/// Peel one leading safe wrapper from `command`. Returns `None` when no
/// safe wrapper is recognised (so the caller leaves the command alone).
///
/// Mirrors the TS `stripSafeWrappers` (`bashPermissions.ts:524`) port
/// scoped to the wrappers we observe in real-world commands —
/// `timeout`, `time`, `nice`, `nohup`. Each call peels exactly one
/// wrapper; the BFS in [`build_command_variants`] iterates so chained
/// wrappers (`nohup timeout 30 cmd`) reach the inner command.
///
/// Not a security boundary — `excluded_commands` is a UX feature
/// (TS comment, `shouldUseSandbox.ts:18-20`). Over-stripping a wrapper
/// only causes a command to dodge the sandbox; under-stripping is
/// strictly safer (sandbox engages).
fn strip_safe_wrapper(command: &str) -> Option<String> {
    let trimmed = command.trim_start();
    let (first, rest) = first_token(trimmed)?;
    let rest = rest.trim_start();
    // Some wrappers accept `--` between themselves and the wrapped command;
    // peel that too so `nohup -- cmd` matches `cmd`. Only strip when `--`
    // is a standalone token — `--foreground` (a long-flag) must NOT match.
    let rest = match rest.strip_prefix("--") {
        Some(after) if after.is_empty() || after.starts_with(char::is_whitespace) => {
            after.trim_start()
        }
        _ => rest,
    };
    match first {
        "time" | "nohup" => Some(rest.to_string()),
        "nice" => Some(strip_nice_flag(rest).unwrap_or(rest).to_string()),
        "timeout" => {
            let after_flags = skip_timeout_flags(rest).trim_start();
            let (dur, after_dur) = first_token(after_flags)?;
            if !looks_like_timeout_duration(dur) {
                return None;
            }
            Some(after_dur.trim_start().to_string())
        }
        _ => None,
    }
}

fn first_token(s: &str) -> Option<(&str, &str)> {
    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    Some(match trimmed.find(|c: char| c.is_whitespace()) {
        Some(idx) => (&trimmed[..idx], &trimmed[idx..]),
        None => (trimmed, ""),
    })
}

fn looks_like_timeout_duration(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    if i == 0 {
        return false;
    }
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    // Optional GNU suffix (s/m/h/d).
    if i < bytes.len() && matches!(bytes[i], b's' | b'm' | b'h' | b'd') {
        i += 1;
    }
    i == bytes.len()
}

fn strip_nice_flag(s: &str) -> Option<&str> {
    let s = s.trim_start();
    // `-n N` (space-separated)
    if let Some(after) = s.strip_prefix("-n ") {
        let after = after.trim_start();
        let (val, rest) = first_token(after)?;
        if val.parse::<i32>().is_ok() {
            return Some(rest.trim_start());
        }
    }
    // `-N` (fused). Only matches a leading `-` followed by digits/sign.
    if let Some(after) = s.strip_prefix('-') {
        let (val, rest) = first_token(after)?;
        if val.parse::<i32>().is_ok() {
            return Some(rest.trim_start());
        }
    }
    None
}

fn skip_timeout_flags(s: &str) -> &str {
    let mut s = s;
    loop {
        let trimmed = s.trim_start();
        if !trimmed.starts_with('-') {
            return trimmed;
        }
        let (flag, after) = match first_token(trimmed) {
            Some(t) => t,
            None => return trimmed,
        };
        // No-arg or fused-value flag — just skip.
        if flag.contains('=')
            || matches!(
                flag,
                "--foreground" | "--preserve-status" | "--verbose" | "-v"
            )
        {
            s = after;
            continue;
        }
        // Value-taking short / long flag — skip flag + next token.
        if matches!(flag, "-k" | "-s" | "--kill-after" | "--signal") {
            let after_val = after.trim_start();
            let (_val, rest) = match first_token(after_val) {
                Some(t) => t,
                None => return trimmed,
            };
            s = rest;
            continue;
        }
        // Unknown flag — bail to avoid mis-stripping.
        return trimmed;
    }
}

/// Match a command variant against an exclusion pattern.
///
/// Pattern types:
/// - `"cmd:*"` — colon-prefix: matches `cmd` and `cmd <anything>`
/// - `"cmd*"` — trailing wildcard: matches anything starting with `cmd`
/// - `"cmd"` — exact first-word: matches `cmd` alone or `cmd <args>`
fn matches_exclusion_pattern(variant: &str, pattern: &str) -> bool {
    // Colon-prefix pattern: "npm:*" matches "npm", "npm install", etc.
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return variant == prefix
            || variant.starts_with(&format!("{prefix} "))
            || variant.starts_with(&format!("{prefix}\t"));
    }

    // Trailing wildcard: "git*" matches "git", "gitk", "git status"
    if pattern.ends_with('*') {
        let prefix = pattern.trim_end_matches('*');
        return variant.starts_with(prefix);
    }

    // Exact first-word match: "git" matches "git" and "git status"
    variant == pattern || variant.starts_with(&format!("{pattern} "))
}

// ============================================================================
// Per-source rule plumbing — feeds the `allow_managed_*_only` policy gates.
// ============================================================================

/// A permission rule tagged with the [`SettingSource`] it came from.
///
/// Used by the sandbox adapter to honor `allow_managed_domains_only` —
/// when set, only `policy_settings`-sourced rules contribute domains
/// to the runtime allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcedRule {
    /// Raw rule string (e.g., `"WebFetch(domain:example.com)"`).
    pub rule: String,
    /// Source layer this rule came from.
    pub source: SettingSource,
}

impl SettingsWithSource {
    /// Flatten per-source `permissions.{allow,deny,ask}` entries into
    /// source-tagged rules. Used by the sandbox adapter to honor
    /// `allow_managed_domains_only` and by the engine config builder
    /// to populate `EngineConfigInput.{allow,deny,ask}_rules`.
    ///
    /// Walks `per_source: HashMap<SettingSource, Value>` and pulls each
    /// source's raw `permissions/allow`, `permissions/deny`, and
    /// `permissions/ask` arrays via JSON-pointer access. Plugin-
    /// contributed rules from [`SettingSource::Plugin`] are not
    /// included (they aren't tracked in `per_source` today). Order
    /// across sources isn't guaranteed — callers that need priority
    /// order should sort by `source`.
    pub fn sourced_permission_rules(
        &self,
    ) -> (Vec<SourcedRule>, Vec<SourcedRule>, Vec<SourcedRule>) {
        let mut allow = Vec::new();
        let mut deny = Vec::new();
        let mut ask = Vec::new();
        for (source, raw) in &self.per_source {
            extract_string_array(raw, "/permissions/allow", &mut |s| {
                allow.push(SourcedRule {
                    rule: s.to_string(),
                    source: *source,
                });
            });
            extract_string_array(raw, "/permissions/deny", &mut |s| {
                deny.push(SourcedRule {
                    rule: s.to_string(),
                    source: *source,
                });
            });
            extract_string_array(raw, "/permissions/ask", &mut |s| {
                ask.push(SourcedRule {
                    rule: s.to_string(),
                    source: *source,
                });
            });
        }
        (allow, deny, ask)
    }

    /// Flatten per-source `sandbox.filesystem.allow_read` paths,
    /// tagged by source. Used by the sandbox adapter to honor
    /// `allow_managed_read_paths_only`.
    pub fn sourced_filesystem_allow_read(&self) -> Vec<(SettingSource, Vec<PathBuf>)> {
        let mut out: Vec<(SettingSource, Vec<PathBuf>)> = Vec::new();
        for (source, raw) in &self.per_source {
            let mut paths: Vec<PathBuf> = Vec::new();
            extract_string_array(raw, "/sandbox/filesystem/allow_read", &mut |s| {
                paths.push(PathBuf::from(s));
            });
            if !paths.is_empty() {
                out.push((*source, paths));
            }
        }
        out
    }
}

/// Walk a JSON pointer that should resolve to an array of strings.
/// Calls `cb` for every string entry; ignores non-string elements and
/// missing/non-array pointers.
fn extract_string_array(value: &serde_json::Value, pointer: &str, cb: &mut dyn FnMut(&str)) {
    let Some(arr) = value.pointer(pointer).and_then(|v| v.as_array()) else {
        return;
    };
    for item in arr {
        if let Some(s) = item.as_str() {
            cb(s);
        }
    }
}

#[cfg(test)]
#[path = "sandbox_settings.test.rs"]
mod tests;
