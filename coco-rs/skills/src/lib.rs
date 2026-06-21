//! Skill system — markdown workflow loading, discovery, execution.

pub mod bundled;
pub mod error;
pub mod extraction;
pub mod mcp_builders;
pub mod overrides;
pub mod prompt_render;
pub mod reminder_source;
pub mod shell_exec;
pub mod usage;
pub mod watcher;

pub use error::SkillsError;
pub use overrides::effective_skill_state;
pub use overrides::resolve_skill_baseline;
pub use overrides::resolve_skill_override_lock;
pub use shell_exec::BashToolHandle;
pub use shell_exec::NoOpBashToolHandle;
// `estimate_skill_frontmatter_bytes` is defined at the crate root
// further down; just listed here as a reminder it is part of the
// public surface used by the `/skills` dialog payload builder.

/// Crate-local Result alias. Default error type is `SkillsError` but the
/// generic stays open so `Result::ok` / 2-arg `Result<T, E>` callsites
/// (e.g. `entries.filter_map(Result::ok)` over `io::Error`) still resolve
/// against `std::result::Result`.
pub type Result<T, E = SkillsError> = std::result::Result<T, E>;

use coco_types::Feature;
use coco_types::Features;
use coco_types::ModelRole;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Execution context for a skill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillContext {
    /// Expand prompt into the current conversation.
    #[default]
    Inline,
    /// Run as an isolated sub-agent.
    Fork,
}

/// A skill definition loaded from a markdown file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDefinition {
    pub name: String,
    /// User-facing override of `name`, populated from the frontmatter
    /// `name` field. `None` falls back to `name` for display.
    ///
    /// Skill identity / lookup always uses `name` (which is path-derived);
    /// `display_name` only changes how the skill is shown in typeahead,
    /// help listings, and similar surfaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub description: String,
    pub prompt: String,
    pub source: SkillSource,
    /// Alternative names for this skill (e.g., short forms).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Semantic model role for fork-mode skill execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_role: Option<ModelRole>,
    /// Guidance for when the model should invoke this skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    /// Named parameters the skill accepts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argument_names: Vec<String>,
    /// Glob patterns for file paths this skill applies to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// Planning/exploration depth override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<coco_types::ReasoningEffort>,
    /// Execution context: inline (default) or fork (sub-agent).
    #[serde(default)]
    pub context: SkillContext,
    /// Agent type when `context` is `Fork`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Semantic version of the skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Whether the skill is disabled (skipped during loading).
    #[serde(default)]
    pub disabled: bool,
    /// Hook configuration (opaque JSON, interpreted by coco-hooks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hooks: Option<serde_json::Value>,
    /// Display hint for arguments (e.g., `[filename]`).
    /// Frontmatter key: `argument-hint`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argument_hint: Option<String>,
    /// Whether users can type `/name` to invoke this skill. Default: true.
    #[serde(default = "default_true")]
    pub user_invocable: bool,
    /// Prevents the model from invoking this skill via the Skill tool.
    #[serde(default)]
    pub disable_model_invocation: bool,
    /// Shell configuration for the skill (opaque JSON).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<serde_json::Value>,
    /// Character count of the skill's prompt content (for token estimation).
    #[serde(default)]
    pub content_length: i64,
    /// Whether the description came verbatim from the frontmatter
    /// `description` field (true) or was synthesised from the markdown
    /// body via `extract_description_from_markdown` (false).
    /// Consumers like the bundled-skill listing can decide to surface only
    /// user-written descriptions.
    #[serde(default)]
    pub has_user_specified_description: bool,
    /// UI label shown while the skill is executing (e.g. spinner caption).
    /// `None` falls back to the consumer-side default (`'running'`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_message: Option<String>,
    /// Whether this skill is hidden from typeahead/help but still invocable.
    /// Separate from `user_invocable` (which blocks user invocation entirely).
    #[serde(default)]
    pub is_hidden: bool,
    /// Optional feature gate. When set, the skill is only visible/invocable
    /// if the listed feature is enabled (`Features::enabled(feature)`).
    /// Used by every gated bundled skill (loop, schedule, dream, hunter,
    /// claude-api, claude-in-chrome, run-skill-generator).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gated_by: Option<Feature>,
    /// Reference files extracted lazily on first invocation.
    ///
    /// Keys are relative paths (forward slashes, no `..`); values are file
    /// contents. When set, the skill prompt is prefixed with
    /// `Base directory for this skill: <dir>` so the model can Read/Grep
    /// these files via the same contract as on-disk skills.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub files: HashMap<String, String>,
    /// On-disk extraction directory after first invocation.
    /// `None` until the skill is first invoked AND has `files`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill_root: Option<PathBuf>,
}

impl SkillDefinition {
    /// Whether this skill is enabled in the given feature set.
    /// Skills without `gated_by` are always enabled.
    pub fn is_enabled(&self, features: &Features) -> bool {
        match self.gated_by {
            Some(feat) => features.enabled(feat),
            None => true,
        }
    }

    /// Name to surface in typeahead / help listings / `/skills`. Returns
    /// the frontmatter-supplied [`Self::display_name`] when set, otherwise
    /// falls back to the canonical [`Self::name`] used for lookup.
    pub fn user_facing_name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }
}

fn default_true() -> bool {
    true
}

/// Where a skill was loaded from.
///
/// This is the canonical enum — also used by `skill_advanced.rs` in coco-tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSource {
    Bundled,
    User {
        path: PathBuf,
    },
    Project {
        path: PathBuf,
    },
    Plugin {
        plugin_name: String,
    },
    /// Enterprise/policy-managed skills.
    Managed {
        path: PathBuf,
    },
    /// Skills discovered from an MCP server.
    Mcp {
        server_name: String,
    },
}

/// Skill manager — discovery, loading, deduplication.
///
/// All mutation goes through interior mutability (`&self`). The catalog
/// (on-disk + MCP skills) sits behind a single `RwLock` so read paths
/// (`get`, `all`, `visible`, `len`) share concurrent access while writes
/// (`register`, `register_mcp_skill`, `unregister_skills_for_mcp_server`,
/// `clear`) serialise. Per-agent announcement state lives in a separate
/// `Mutex` so the read-heavy catalog isn't blocked when a listing pass
/// mutates the sent set.
///
#[derive(Default, Debug)]
pub struct SkillManager {
    /// Disk + MCP skill catalog. Both halves share one lock so a
    /// snapshot (`all()`) is a single read-side acquisition.
    catalog: std::sync::RwLock<SkillCatalog>,
    /// Skills already announced in a `skill_listing` reminder, keyed by
    /// `agent_id.unwrap_or("")` so the main thread (empty key) and each
    /// subagent get their own turn-0 listing. Separate lock from
    /// [`Self::catalog`] so listing-time mutation doesn't block reads.
    announcements: std::sync::Mutex<HashMap<String, HashSet<String>>>,
}

/// Internal storage for the skill catalog. Held behind one `RwLock`
/// inside [`SkillManager`] so both halves are read-consistent under a
/// single lock acquisition.
///
/// **Conditional split.** Disk skills land in one of two maps based on
/// their frontmatter `paths` field:
///
/// - `disk` — unconditional + activated-conditional skills. These are
///   visible to the model (`listing()`, `visible()`, `get()`).
/// - `disk_conditional` — skills with non-empty `paths` that have NOT
///   yet been activated by a matching file operation. Hidden until
///   [`SkillManager::activate_for_paths`] promotes them.
///
/// Promotion is one-way and session-persistent via
/// [`Self::activated_conditional_names`] — once activated, a skill
/// survives reloads and stays in `disk` for the rest of the session.
#[derive(Default, Debug)]
struct SkillCatalog {
    /// Visible disk / bundled skills, keyed by skill name.
    disk: HashMap<String, Arc<SkillDefinition>>,
    /// Path-gated disk skills awaiting activation. Hidden from
    /// `listing()` / `visible()` / `get()` until promoted.
    disk_conditional: HashMap<String, Arc<SkillDefinition>>,
    /// Names of conditional skills that have been activated this
    /// session. Survives reloads — once activated, stays active for the
    /// entire session regardless of cache clears.
    activated_conditional_names: HashSet<String>,
    /// MCP-sourced skills, keyed by `(server_name, skill_name)` so a
    /// per-server unregister can drop a slice without touching the rest.
    /// MCP skills have no on-disk `paths` semantics (see `mcp_builders.rs`).
    mcp: HashMap<(String, String), Arc<SkillDefinition>>,
    /// Canonical file identities already loaded for disk skills.
    disk_file_identities: HashSet<PathBuf>,
}

impl SkillCatalog {
    fn is_empty(&self) -> bool {
        self.disk.is_empty() && self.mcp.is_empty()
    }
    fn len(&self) -> usize {
        self.disk.len() + self.mcp.len()
    }
    /// Whether `skill` should be routed to the conditional bucket on register.
    fn is_conditional(&self, skill: &SkillDefinition) -> bool {
        !skill.paths.is_empty() && !self.activated_conditional_names.contains(&skill.name)
    }
    /// Insert `skill` into the right bucket per [`Self::is_conditional`].
    /// First loaded skill wins by canonical file identity and skill name.
    fn insert_disk(&mut self, skill: SkillDefinition) {
        let name = skill.name.clone();
        if self.disk.contains_key(&name) || self.disk_conditional.contains_key(&name) {
            return;
        }
        if let Some(identity) = canonical_skill_identity(&skill)
            && !self.disk_file_identities.insert(identity)
        {
            return;
        }
        let target = if self.is_conditional(&skill) {
            &mut self.disk_conditional
        } else {
            &mut self.disk
        };
        target.insert(name, Arc::new(skill));
    }
}

impl SkillManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the set of skill names not yet announced to `agent_id`,
    /// then mark them as sent. Returns `(new_skills, is_initial)` where
    /// `is_initial` is true on the first non-empty announcement for this
    /// agent (`sent.size === 0` check).
    pub fn take_unannounced_skills(
        &self,
        agent_id: Option<&str>,
        current: &[&str],
    ) -> (Vec<String>, bool) {
        let key = agent_id.unwrap_or("").to_string();
        let mut guard = self
            .announcements
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sent = guard.entry(key).or_default();
        let is_initial = sent.is_empty();
        let mut delta = Vec::new();
        for name in current {
            if sent.insert((*name).to_string()) {
                delta.push((*name).to_string());
            }
        }
        (delta, is_initial)
    }

    /// Clear the per-agent announcement map so every skill re-announces
    /// on the next listing pass. Called after a disk reload (the catalog
    /// changed, so an edited same-named skill must surface again) and on
    /// `/clear` (the conversation is reset).
    pub fn reset_announcements(&self) {
        let mut guard = self
            .announcements
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.clear();
    }

    /// Register (or replace) an on-disk / bundled skill. Interior-mut,
    /// safe to call on a shared `Arc<SkillManager>`.
    ///
    /// Skills with non-empty `paths` and no prior activation are routed
    /// to the hidden conditional bucket. They surface only after
    /// [`Self::activate_for_paths`] matches a file the model touched this
    /// session.
    pub fn register(&self, skill: SkillDefinition) {
        let mut guard = self.write_catalog();
        guard.insert_disk(skill);
    }

    /// Register (or replace) an MCP-sourced skill.
    ///
    /// Uses the currently-registered MCP skill builder (see
    /// [`crate::mcp_builders::mcp_skill_builder`]) to parse the spec into
    /// a typed [`SkillDefinition`] with [`SkillSource::Mcp`].
    ///
    pub fn register_mcp_skill(&self, spec: crate::mcp_builders::McpSkillSpec) -> crate::Result<()> {
        let builder = crate::mcp_builders::mcp_skill_builder();
        let key = (spec.server_name.clone(), spec.name.clone());
        let skill = builder.build(&spec)?;
        let mut guard = self.write_catalog();
        guard.mcp.insert(key, Arc::new(skill));
        Ok(())
    }

    /// Drop all MCP-sourced skills published by `server_name`.
    ///
    /// Called on MCP server disconnect / reconnect so stale skills do
    /// not survive a server cycle. Returns the number of skills removed.
    pub fn unregister_skills_for_mcp_server(&self, server_name: &str) -> usize {
        let mut guard = self.write_catalog();
        let before = guard.mcp.len();
        guard.mcp.retain(|(s, _), _| s != server_name);
        before - guard.mcp.len()
    }

    /// Replace the entire disk-skill catalog with a fresh set. Used by
    /// the watcher's reload path; MCP-sourced skills are preserved.
    ///
    /// `activated_conditional_names` survives the reload — once a conditional
    /// skill is activated this session it stays activated even if the disk
    /// catalog is rebuilt.
    ///
    /// `&self`: interior-mut via the shared `RwLock`.
    pub fn reload_disk_skills(&self, fresh: impl IntoIterator<Item = SkillDefinition>) {
        let mut guard = self.write_catalog();
        guard.disk.clear();
        guard.disk_conditional.clear();
        guard.disk_file_identities.clear();
        for skill in fresh {
            guard.insert_disk(skill);
        }
    }

    /// Activate path-gated skills whose `paths` patterns match any of
    /// the given files, moving them from the conditional bucket into
    /// the visible `disk` bucket. Returns the names of newly-activated
    /// skills (in stable sorted order) so callers can log / emit
    /// telemetry.
    ///
    /// - Patterns are gitignore-style globs anchored at `cwd`
    ///   (uses the Rust `ignore` crate).
    /// - File paths outside `cwd`, empty, or escaping via `..` are skipped.
    /// - Activation is one-way and persistent for the session via
    ///   [`SkillCatalog::activated_conditional_names`].
    ///
    /// The reminder pipeline's `skill_listing` generator picks up the
    /// newly-visible names on the next turn via
    /// [`Self::take_unannounced_skills`].
    pub fn activate_for_paths(&self, file_paths: &[PathBuf], cwd: &Path) -> Vec<String> {
        let mut guard = self.write_catalog();
        if guard.disk_conditional.is_empty() {
            return Vec::new();
        }

        // Normalize files to absolute cwd-rooted paths, skipping
        // anything outside cwd (relative paths starting with `..` or
        // absolute paths outside cwd). We need ABSOLUTE paths under `cwd`
        // because `matched_path_or_any_parents` (walks parent dirs so a
        // bare-dir pattern like `build` matches `build/foo.rs`) requires
        // its input to be a descendant of the matcher's root.
        let absolute_files: Vec<PathBuf> = file_paths
            .iter()
            .filter_map(|p| relative_to_cwd(p, cwd).map(|rel| cwd.join(rel)))
            .collect();
        if absolute_files.is_empty() {
            return Vec::new();
        }

        let mut activated: Vec<String> = Vec::new();
        let conditional_names: Vec<String> = guard.disk_conditional.keys().cloned().collect();
        for name in conditional_names {
            let skill = match guard.disk_conditional.get(&name) {
                Some(s) => s.clone(),
                None => continue,
            };
            let matcher = match build_skill_path_matcher(cwd, &skill.paths) {
                Some(m) => m,
                None => continue,
            };
            let hit = absolute_files.iter().any(|abs| {
                matches!(
                    matcher.matched_path_or_any_parents(abs, /*is_dir*/ false),
                    ignore::Match::Ignore(_)
                )
            });
            if !hit {
                continue;
            }
            if let Some(skill_arc) = guard.disk_conditional.remove(&name) {
                guard.disk.insert(name.clone(), skill_arc);
                guard.activated_conditional_names.insert(name.clone());
                activated.push(name);
            }
        }
        activated.sort();
        activated
    }

    /// Number of conditional (hidden) skills currently awaiting
    /// activation. Test/diagnostic surface only.
    pub fn conditional_skill_count(&self) -> usize {
        self.read_catalog().disk_conditional.len()
    }

    /// Look up a skill by canonical name or alias.
    ///
    /// On-disk skills win over MCP skills on name collision — disk is the
    /// stable source of truth; MCP can republish on every reconnect.
    pub fn get(&self, name: &str) -> Option<Arc<SkillDefinition>> {
        let guard = self.read_catalog();
        if let Some(s) = guard.disk.get(name) {
            return Some(s.clone());
        }
        if let Some(s) = guard
            .disk
            .values()
            .find(|s| s.aliases.iter().any(|a| a == name))
        {
            return Some(s.clone());
        }
        guard
            .mcp
            .values()
            .find(|s| s.name == name || s.aliases.iter().any(|a| a == name))
            .cloned()
    }

    /// Iterate all skills (on-disk + MCP) as shared `Arc`s.
    ///
    /// `SkillDefinition` is heap-heavy (HashMap + multiple Vecs), so
    /// returning `Arc<SkillDefinition>` avoids the per-call deep clone
    /// while keeping `&s.field` access seamless via `Deref`.
    ///
    /// **Excludes un-activated conditional (`paths`-gated) skills** —
    /// they live in `disk_conditional` until a touched file matches.
    /// Consumers that need to enumerate every known skill regardless
    /// of activation (e.g. the `/skills` dialog so a `paths`-gated
    /// skill is visible before activation) should use
    /// [`Self::all_including_conditional`].
    pub fn all(&self) -> Vec<Arc<SkillDefinition>> {
        let guard = self.read_catalog();
        let mut out: Vec<Arc<SkillDefinition>> = guard.disk.values().cloned().collect();
        out.extend(guard.mcp.values().cloned());
        out
    }

    /// Iterate every known skill — `disk ∪ disk_conditional ∪ mcp` —
    /// as shared `Arc`s.
    ///
    /// Used by the `/skills` dialog so users can toggle `paths`-gated
    /// skills before they activate. The model-facing listing path
    /// continues to use [`Self::all`] / [`Self::visible`] which
    /// honour the activation gate.
    pub fn all_including_conditional(&self) -> Vec<Arc<SkillDefinition>> {
        let guard = self.read_catalog();
        let mut out: Vec<Arc<SkillDefinition>> = guard.disk.values().cloned().collect();
        out.extend(guard.disk_conditional.values().cloned());
        out.extend(guard.mcp.values().cloned());
        out
    }

    /// Iterate skills currently enabled under the given feature set.
    /// Applied at every typeahead / Skill-tool listing call site.
    pub fn visible(&self, features: &Features) -> Vec<Arc<SkillDefinition>> {
        self.all()
            .into_iter()
            .filter(|s| s.is_enabled(features))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.read_catalog().len()
    }

    pub fn is_empty(&self) -> bool {
        self.read_catalog().is_empty()
    }

    // ─── internal lock helpers ──────────────────────────────────────

    fn read_catalog(&self) -> std::sync::RwLockReadGuard<'_, SkillCatalog> {
        self.catalog
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write_catalog(&self) -> std::sync::RwLockWriteGuard<'_, SkillCatalog> {
        self.catalog
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Discover and register skills from multiple directories.
    ///
    /// Uses SKILL.md directory format. Earlier directories win by
    /// canonical path and by skill name.
    pub fn load_from_dirs(&self, dirs: &[PathBuf]) {
        for dir in dirs {
            for skill in
                discover_skills_with_format(std::slice::from_ref(dir), SkillDirFormat::SkillMdOnly)
            {
                self.register(skill);
            }
        }
    }

    /// Load scopes in priority order (managed → user → project), tagging each skill with the
    /// correct [`SkillSource`] variant.
    pub fn load_scoped(&self, scopes: &SkillScopes) {
        if let Some(p) = &scopes.managed {
            self.load_with_source(p, SkillDirFormat::SkillMdOnly, |path| {
                SkillSource::Managed { path }
            });
        }
        if let Some(p) = &scopes.user_skills {
            self.load_with_source(p, SkillDirFormat::SkillMdOnly, |path| SkillSource::User {
                path,
            });
        }
        if let Some(p) = &scopes.project_skills {
            self.load_with_source(p, SkillDirFormat::SkillMdOnly, |path| {
                SkillSource::Project { path }
            });
        }
    }

    fn load_with_source<F>(&self, dir: &Path, format: SkillDirFormat, source_for: F)
    where
        F: Fn(PathBuf) -> SkillSource,
    {
        let dirs = vec![dir.to_path_buf()];
        for mut skill in discover_skills_with_format(&dirs, format) {
            // Replace the auto-set User source from `parse_skill_markdown` with
            // the actual scope.
            let path = match &skill.source {
                SkillSource::User { path }
                | SkillSource::Project { path }
                | SkillSource::Managed { path } => path.clone(),
                _ => PathBuf::new(),
            };
            skill.source = source_for(path);
            self.register(skill);
        }
    }

    /// Load skills from a legacy `commands/` directory ([`SkillDirFormat::Legacy`]
    /// — both `SKILL.md` directories and flat `.md` files), tagging each with
    /// the given setting-source scope.
    ///
    /// Dedups against `skills/` by realpath. The source's path is preserved so
    /// `canonical_skill_identity` can dedup against the `skills/`-loaded copies
    /// via `disk_file_identities`.
    fn load_legacy_command_scope(&self, scope: SettingScope, dir: &Path) {
        self.load_with_source(dir, SkillDirFormat::Legacy, |path| scope.source_for(path));
    }
}

/// Setting-source scope for legacy `commands/` directory loading.
#[derive(Debug, Clone, Copy)]
enum SettingScope {
    Managed,
    User,
    Project,
}

impl SettingScope {
    fn source_for(self, path: PathBuf) -> SkillSource {
        match self {
            Self::Managed => SkillSource::Managed { path },
            Self::User => SkillSource::User { path },
            Self::Project => SkillSource::Project { path },
        }
    }
}

/// Per-scope skill directory configuration for `SkillManager::load_scoped`.
/// Returns paths for managed, user, and project skill sources.
#[derive(Debug, Clone, Default)]
pub struct SkillScopes {
    /// Enterprise/policy skills (highest priority).
    pub managed: Option<PathBuf>,
    /// `~/.coco/skills/`.
    pub user_skills: Option<PathBuf>,
    /// Project `.coco/skills` directories.
    pub project_skills: Option<PathBuf>,
    /// Deprecated; ignored.
    pub user_commands: Option<PathBuf>,
    /// Deprecated; ignored.
    pub project_commands: Option<PathBuf>,
}

fn canonical_skill_identity(skill: &SkillDefinition) -> Option<PathBuf> {
    let path = match &skill.source {
        SkillSource::User { path }
        | SkillSource::Project { path }
        | SkillSource::Managed { path } => path,
        SkillSource::Bundled | SkillSource::Plugin { .. } | SkillSource::Mcp { .. } => {
            return None;
        }
    };
    Some(std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
}

/// Parse a skill definition from a markdown file.
///
/// Format:
/// - First line `# Name` → skill name
/// - Optional YAML-like frontmatter between `---` markers (description, allowed_tools, model)
/// - Remaining content → prompt field
pub fn load_skill_from_file(path: &Path) -> crate::Result<SkillDefinition> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        crate::SkillsError::generic(format!("failed to read skill file {}: {e}", path.display()))
    })?;

    parse_skill_markdown(&content, path)
}

/// Whether a directory uses the canonical SKILL.md-only format or also allows
/// parser-only flat `.md` compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillDirFormat {
    /// Only `skill-name/SKILL.md` directories.
    SkillMdOnly,
    /// Both `SKILL.md` directories and flat `.md` files. Session/project
    /// discovery does not use this mode.
    Legacy,
}

/// Walk directories and discover skill files, deduplicating by canonical path.
pub fn discover_skills(dirs: &[PathBuf]) -> Vec<SkillDefinition> {
    discover_skills_with_format(dirs, SkillDirFormat::SkillMdOnly)
}

/// Walk up from each file path to the cwd boundary and collect any
/// `<ancestor>/.coco/skills/` directories that exist on disk.
///
/// The scanner runs on every Read/Write/Edit so the model can pick up
/// nested project skills without having to opt into them at startup.
///
/// # Algorithm
/// 1. For each file path, start at the file's parent dir.
/// 2. Walk upwards while the current dir is a strict descendant of cwd
///    (i.e. `current_dir.starts_with(cwd + sep)`). The cwd boundary is
///    excluded because cwd-level skills are already loaded at startup —
///    only nested ones are interesting here.
/// 3. At each level, check `<currentDir>/.coco/skills` and add it to
///    the result list if the directory exists.
/// 4. Sort the results by path depth (deepest first) so deeper skills
///    take precedence when the manager loads them.
///
/// **Implementation notes**:
/// - No memoization cache. A module-level set to skip already-stat'd dirs
///   would save work, but this function returns the full list each call; the
///   caller (Read/Write/Edit) is responsible for deduplicating against its
///   own state if desired.
/// - No gitignore filtering. Uses `git check-ignore` semantics are delegated
///   to `coco-file-ignore` if the caller wants them; this base function stays
///   gitignore-agnostic so it has no `git` dependency.
///
/// Returns paths relative to (or absolute under) the cwd, deepest first.
pub fn discover_skill_dirs_for_paths(file_paths: &[&Path], cwd: &Path) -> Vec<PathBuf> {
    let resolved_cwd = cwd.to_path_buf();
    let mut result: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();

    // Runs `isPathGitignored` before adding each dir, so e.g.
    // `node_modules/pkg/.coco/skills` is skipped. Fails open outside a
    // git repo (PathChecker ignores nothing).
    let ignore_checker = coco_file_ignore::PathChecker::new(
        &resolved_cwd,
        &coco_file_ignore::IgnoreConfig::default(),
    );

    for &file_path in file_paths {
        // Start at the file's parent dir (skip the file itself).
        let Some(start_parent) = file_path.parent() else {
            continue;
        };
        let mut current_dir = start_parent.to_path_buf();

        // Walk upward while strictly inside cwd.
        loop {
            // Strict descendant check — `current_dir == cwd` doesn't
            // count because cwd-level skills are loaded at startup.
            if !is_strict_descendant_of(&current_dir, &resolved_cwd) {
                break;
            }

            // Skip gitignored containing dirs (node_modules, build dirs,
            // etc.) but keep walking up to non-ignored ancestors.
            if !ignore_checker.is_ignored(&current_dir) {
                let skill_dir = current_dir.join(".coco").join("skills");
                if seen.insert(skill_dir.clone()) && skill_dir.is_dir() {
                    result.push(skill_dir);
                }
            }

            // Walk to parent. Stop at root or if we cycle (shouldn't
            // happen but defensive).
            let Some(parent) = current_dir.parent() else {
                break;
            };
            if parent == current_dir {
                break;
            }
            current_dir = parent.to_path_buf();
        }
    }

    // Deepest-first ordering so the manager honors nesting precedence.
    // Sorts by path-component count via separator count, which is
    // platform-correct because PathBuf uses native sep.
    result.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    result
}

/// True iff `child` is a strict descendant of `parent` (i.e. `child`
/// starts with `parent` AND `child != parent`). Used by
/// `discover_skill_dirs_for_paths` to gate the upward walk.
fn is_strict_descendant_of(child: &Path, parent: &Path) -> bool {
    if child == parent {
        return false;
    }
    child.starts_with(parent)
}

/// Walk directories with explicit format control.
pub fn discover_skills_with_format(
    dirs: &[PathBuf],
    format: SkillDirFormat,
) -> Vec<SkillDefinition> {
    let mut skills = Vec::new();
    let mut seen_paths: HashSet<PathBuf> = HashSet::new();

    for dir in dirs {
        if !dir.is_dir() {
            tracing::debug!("skipping non-existent skill directory: {}", dir.display());
            continue;
        }

        // Check immediate children for SKILL.md directories
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.filter_map(Result::ok) {
                let entry_path = entry.path();

                if entry_path.is_dir() {
                    // Look for SKILL.md inside the directory (case-insensitive)
                    if let Some(skill_md) = find_skill_md(&entry_path) {
                        try_load_skill(&skill_md, &mut skills, &mut seen_paths);
                    }
                } else if format == SkillDirFormat::Legacy
                    && entry_path.extension().is_some_and(|ext| ext == "md")
                    && entry_path.is_file()
                {
                    // Parser-only compatibility for explicitly supplied legacy dirs.
                    try_load_skill(&entry_path, &mut skills, &mut seen_paths);
                }
            }
        }
    }

    skills
}

/// Find a SKILL.md file in a directory (case-insensitive).
fn find_skill_md(dir: &Path) -> Option<PathBuf> {
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() {
        return Some(skill_md);
    }
    // Case-insensitive fallback
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name();
            if name.to_string_lossy().eq_ignore_ascii_case("skill.md") && entry.path().is_file() {
                return Some(entry.path());
            }
        }
    }
    None
}

/// Try to load a skill from a file, deduplicating by canonical path.
///
/// `parse_skill_markdown` derives the skill name from `path` (parent dir
/// for SKILL.md, file stem otherwise), so callers don't need to pass it.
fn try_load_skill(
    path: &Path,
    skills: &mut Vec<SkillDefinition>,
    seen_paths: &mut HashSet<PathBuf>,
) {
    // Deduplicate by canonical path (realpath)
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if !seen_paths.insert(canonical) {
        tracing::debug!("skipping duplicate skill at {}", path.display());
        return;
    }

    match load_skill_from_file(path) {
        Ok(skill) if skill.disabled => {
            tracing::debug!("skipping disabled skill: {}", skill.name);
        }
        Ok(skill) => skills.push(skill),
        Err(e) => {
            tracing::warn!("failed to load skill from {}: {e}", path.display());
        }
    }
}

/// First non-empty line of `content` as a description, with `# heading`
/// markers stripped and the result capped at 100 characters.
///
/// Used by the skill loader as a fallback when `frontmatter.description` is
/// missing — every skill ends up with *some* human-readable label even if
/// the author skipped the frontmatter field.
pub fn extract_description_from_markdown(content: &str, default_description: &str) -> String {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Strip leading `#`/`##`/etc. heading markers (`^#+\s+(.+)$`).
        let body = trimmed.trim_start_matches('#').trim_start().to_string();
        let text = if body.is_empty() {
            trimmed.to_string()
        } else {
            body
        };
        // Cap at 100 chars (97 chars + '...'). char_indices to
        // stay UTF-8 safe.
        if text.chars().count() > 100 {
            let cut: String = text.chars().take(97).collect();
            return format!("{cut}...");
        }
        return text;
    }
    default_description.to_string()
}

/// Computes the skill name from its file path:
///
/// - `<dir>/SKILL.md` (case-insensitive) → `<dir>` basename
/// - `<dir>/<stem>.md` → `<stem>`
///
/// Returns an error if the path has no usable basename or its parent
/// directory has no name (e.g. root `/SKILL.md`).
fn derive_skill_name_from_path(path: &Path) -> crate::Result<String> {
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| crate::SkillsError::generic("skill path has no file name"))?;

    if file_name.eq_ignore_ascii_case("SKILL.md") {
        let parent = path.parent().ok_or_else(|| {
            crate::SkillsError::generic(format!(
                "SKILL.md at {} has no parent directory to derive a name from",
                path.display()
            ))
        })?;
        let dir_name = parent.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
            crate::SkillsError::generic(format!(
                "SKILL.md parent {} has no usable directory name",
                parent.display()
            ))
        })?;
        return Ok(dir_name.to_string());
    }

    // Flat `.md` parser compatibility: strip the extension.
    let stem = path.file_stem().and_then(|s| s.to_str()).ok_or_else(|| {
        crate::SkillsError::generic(format!("skill path {} has no file stem", path.display()))
    })?;
    Ok(stem.to_string())
}

/// Parse a comma-separated list from a frontmatter value.
fn parse_csv_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse a whitespace-separated list (split on whitespace).
/// Validates argument names: filters numeric-only names (they conflict with `$N` shorthand).
fn parse_argument_names_field(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(str::to_string)
        .filter(|s| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit()))
        .collect()
}

/// Parse skill markdown content into a `SkillDefinition`.
///
/// Parsing rules:
///
/// - The whole file is fed to [`coco_frontmatter::parse`]. Frontmatter
///   only matches when `---` opens the file (regex `^---\s*\n…`); any
///   leading `# heading` line is part of the body, not a name.
/// - The skill `name` comes from the file path, never from a heading or
///   from the frontmatter `name` field:
///     - `<dir>/SKILL.md` (case-insensitive) → `<dir>` basename
///     - `<dir>/<stem>.md` → `<stem>` (parser compatibility; not used by
///       session/project discovery)
/// - Frontmatter `name`, if present, is silently ignored. (Upstream exposes it
///   as `displayName` on the command record; coco-rs `SkillDefinition`
///   has no separate displayName field, so we drop it to avoid a
///   misleading override of the path-derived name.)
fn parse_skill_markdown(content: &str, path: &Path) -> crate::Result<SkillDefinition> {
    use coco_frontmatter::FrontmatterValue;

    let name = derive_skill_name_from_path(path)?;

    let frontmatter = coco_frontmatter::parse(content);
    let data = &frontmatter.data;

    // Look up a key under any of several aliases (kebab + snake variants).
    let lookup = |aliases: &[&str]| -> Option<&FrontmatterValue> {
        aliases.iter().find_map(|k| data.get(*k))
    };
    let lookup_str = |aliases: &[&str]| -> Option<String> {
        lookup(aliases)
            .and_then(FrontmatterValue::as_str)
            .map(str::to_owned)
    };
    let lookup_bool =
        |aliases: &[&str]| -> Option<bool> { lookup(aliases).and_then(FrontmatterValue::as_bool) };
    // Coerce scalars to strings — `version: 1.2.0` parses as a string,
    // `version: 1.2` as a float, `version: 1` as an int. Skills accept any.
    let lookup_scalar_string =
        |aliases: &[&str]| -> Option<String> { lookup(aliases).and_then(scalar_to_string) };

    // Body fallback ensures every skill ends up with *some* human-readable
    // description even if the author skipped the frontmatter field.
    let raw_description = lookup_str(&["description"]).filter(|s| !s.trim().is_empty());
    let has_user_specified_description = raw_description.is_some();
    let description = raw_description
        .unwrap_or_else(|| extract_description_from_markdown(&frontmatter.content, "Skill"));

    // Coerce numeric / bool scalars to string so authors can write
    // `name: 42` without losing it. Sequences / mappings are not valid
    // displayName shapes; treat them as absent.
    let display_name = lookup(&["name"]).and_then(scalar_to_string);

    // Lists accept either a YAML sequence (`[Bash, Read]`) or a CSV string
    // (`Bash, Read, Grep`).
    let allowed_tools = lookup(&["allowed-tools", "allowed_tools"]).map(value_to_csv_list);

    let model = lookup_str(&["model"]);
    let model_role = lookup_str(&["model-role", "model_role", "modelRole"])
        .and_then(|raw| raw.parse::<ModelRole>().ok());
    let when_to_use = lookup_str(&["when-to-use", "when_to_use"]);

    // Reads the `arguments` frontmatter key. Legacy `argument-names` /
    // `argument_names` aliases are accepted for disk skills that pre-date
    // the rename. Splits on whitespace, not commas, and drops numeric-only names.
    let argument_names = lookup(&["arguments", "argument-names", "argument_names"])
        .map(|v| match v {
            FrontmatterValue::Sequence(_) => v
                .as_string_list()
                .map(|items| items.into_iter().map(str::to_owned).collect())
                .unwrap_or_default(),
            FrontmatterValue::String(s) => parse_argument_names_field(s),
            _ => Vec::new(),
        })
        .unwrap_or_default();

    let aliases = lookup(&["aliases"])
        .map(value_to_csv_list)
        .unwrap_or_default();

    let paths: Vec<String> = lookup(&["paths"])
        .map(|v| {
            // YAML list (`[a, b]`) → take items as-is. CSV string → split
            // on top-level commas, brace-aware so `*.{ts,tsx}` isn't broken
            // on the inner comma.
            let raw: Vec<String> = match v {
                FrontmatterValue::Sequence(_) => v
                    .as_string_list()
                    .map(|items| items.into_iter().map(str::to_owned).collect())
                    .unwrap_or_default(),
                FrontmatterValue::String(s) => split_top_level_commas(s)
                    .into_iter()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .collect(),
                _ => Vec::new(),
            };
            raw.into_iter()
                .flat_map(|p| expand_braces(&p))
                // Strip trailing `/**` because the `ignore` library matches a
                // bare path as both the path and everything inside it.
                .map(|p| {
                    if let Some(stripped) = p.strip_suffix("/**") {
                        stripped.to_string()
                    } else {
                        p
                    }
                })
                .filter(|p| !p.is_empty())
                .collect()
        })
        .map(|patterns: Vec<String>| {
            // If all patterns are bare `**`, treat as no paths.
            if patterns.is_empty() || patterns.iter().all(|p| p == "**") {
                Vec::new()
            } else {
                patterns
            }
        })
        .unwrap_or_default();

    let effort = lookup_str(&["effort"]).and_then(|s| s.trim().parse().ok());
    let context = match lookup_str(&["context"]).as_deref() {
        Some("fork") => SkillContext::Fork,
        _ => SkillContext::Inline,
    };
    let agent = lookup_str(&["agent"]);
    let version = lookup_scalar_string(&["version"]);
    let disabled = lookup_bool(&["disabled"]).unwrap_or(false);
    let argument_hint = lookup_str(&["argument-hint", "argument_hint"]);
    // Default: user-invocable. Only an explicit false value disables it.
    let user_invocable = lookup_bool(&["user-invocable", "user_invocable"]).unwrap_or(true);
    let disable_model_invocation =
        lookup_bool(&["disable-model-invocation", "disable_model_invocation"]).unwrap_or(false);

    // Hooks/shell are passed through as opaque JSON so coco-hooks /
    // shell_exec interpret them. Mappings, sequences, and scalars all
    // round-trip via `FrontmatterValue::to_json`.
    let hooks = lookup(&["hooks"]).map(FrontmatterValue::to_json);
    let shell = lookup(&["shell"]).map(FrontmatterValue::to_json);

    let prompt = frontmatter.content.trim().to_string();
    let content_length = prompt.len() as i64;
    let is_hidden = !user_invocable;

    Ok(SkillDefinition {
        name,
        display_name,
        description,
        prompt,
        // Hard-codes `progressMessage: 'running'` so consumers don't have
        // to know the default.
        progress_message: Some("running".to_string()),
        has_user_specified_description,
        source: SkillSource::User {
            path: path.to_path_buf(),
        },
        aliases,
        allowed_tools,
        model,
        model_role,
        when_to_use,
        argument_names,
        paths,
        effort,
        context,
        agent,
        version,
        disabled,
        hooks,
        argument_hint,
        user_invocable,
        disable_model_invocation,
        shell,
        content_length,
        is_hidden,
        gated_by: None,
        files: HashMap::new(),
        skill_root: None,
    })
}

/// Convert a frontmatter value to a CSV-style list of strings.
/// Sequences pass through; strings split on commas; everything else → empty.
fn value_to_csv_list(v: &coco_frontmatter::FrontmatterValue) -> Vec<String> {
    use coco_frontmatter::FrontmatterValue;
    match v {
        FrontmatterValue::Sequence(_) => v
            .as_string_list()
            .map(|items| items.into_iter().map(str::to_owned).collect())
            .unwrap_or_default(),
        FrontmatterValue::String(s) => parse_csv_list(s),
        _ => Vec::new(),
    }
}

/// Coerce a scalar YAML value (string / int / float / bool) to its string
/// form. Sequences and mappings return `None` — a structured shape can't
/// be a single scalar field like `version` or `model`.
fn scalar_to_string(v: &coco_frontmatter::FrontmatterValue) -> Option<String> {
    use coco_frontmatter::FrontmatterValue;
    match v {
        FrontmatterValue::String(s) => Some(s.clone()),
        FrontmatterValue::Int(n) => Some(n.to_string()),
        FrontmatterValue::Float(f) => Some(f.to_string()),
        FrontmatterValue::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Platform-specific managed configuration base directory.
fn managed_base_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/ClaudeCode/.coco")
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Linux and other Unix platforms
        PathBuf::from("/etc/claude-code/.coco")
    }
}

/// Platform-specific managed skills directory.
pub fn get_managed_skills_path() -> PathBuf {
    managed_base_path().join("skills")
}

/// Platform-specific managed legacy `commands/` directory.
pub fn get_managed_commands_path() -> PathBuf {
    managed_base_path().join("commands")
}

/// Per-scope load gates for [`build_session_skill_manager`], resolved by the
/// app layer from `--setting-sources`, the `strictPluginOnlyCustomization`
/// policy (`skills` surface), `--add-dir`, and `COCO_DISABLE_POLICY_SKILLS`.
///
/// Controls which disk scopes load. Bundled skills always load and are not
/// gated here. When `skills_locked` is set, only managed (policy) skills
/// load — user, project, legacy, and additional dirs are all skipped (the
/// policy locks customization surfaces to plugin-only sources).
#[derive(Debug, Clone)]
pub struct SkillLoadGates {
    /// Load managed/policy `.coco/skills`. Gated by `COCO_DISABLE_POLICY_SKILLS`.
    pub managed_enabled: bool,
    /// Load user `~/.coco/skills`. Requires `userSettings` enabled and not locked.
    pub user_enabled: bool,
    /// Load project `.coco/skills` walk-up. Requires `projectSettings` enabled
    /// and not locked.
    pub project_enabled: bool,
    /// Load legacy `.coco/commands` dirs (managed → user → project up-to-home).
    /// Gated like project (`!skillsLocked`).
    pub legacy_enabled: bool,
    /// Load `.coco/skills` under each `--add-dir` path. Requires project scope
    /// enabled (and not locked).
    pub additional_dirs_enabled: bool,
    /// Resolved `--add-dir` (and settings `additional_directories`) roots.
    pub additional_dirs: Vec<PathBuf>,
    /// `strictPluginOnlyCustomization` locks the `skills` surface. When set,
    /// only managed skills load regardless of the other gates.
    pub skills_locked: bool,
}

impl SkillLoadGates {
    /// All scopes enabled, no additional dirs, nothing locked. Used by tests
    /// and callers that don't carry a resolved `RuntimeConfig`.
    pub fn all_enabled() -> Self {
        Self {
            managed_enabled: true,
            user_enabled: true,
            project_enabled: true,
            legacy_enabled: true,
            additional_dirs_enabled: true,
            additional_dirs: Vec::new(),
            skills_locked: false,
        }
    }
}

/// Build the canonical per-session skill catalog: bundled skills plus
/// managed, user, and project `.coco/skills` disk scopes.
///
/// Single source of truth so the command registry, the `/context` usage
/// detail, the `/skills` dialog, and the reminder `SkillsSource` all read
/// the same catalog and cannot drift. Always folds bundled commands into
/// the session skill set.
///
/// `gates` controls which disk scopes load (per `--setting-sources` and the
/// `strictPluginOnlyCustomization` policy). `skills_locked` forces a managed-
/// only load. Load order: managed → user → project walk-up → additional
/// `--add-dir` `.coco/skills`. The [`SkillManager`] dedups by canonical path
/// (first-wins), so overlapping dirs don't double-register.
pub fn build_session_skill_manager(
    config_home: &Path,
    cwd: &Path,
    gates: &SkillLoadGates,
) -> SkillManager {
    let manager = SkillManager::new();
    bundled::register_bundled(&manager);

    // Managed/policy skills — gated by COCO_DISABLE_POLICY_SKILLS only; the
    // `skills` lock does NOT skip managed (admin-controlled sources always load).
    if gates.managed_enabled {
        manager.load_scoped(&SkillScopes {
            managed: Some(get_managed_skills_path()),
            ..SkillScopes::default()
        });
    }

    // User + project + additional skills, and legacy commands, are ALL skipped
    // when the `skills` surface is locked.
    if !gates.skills_locked {
        if gates.user_enabled {
            manager.load_scoped(&SkillScopes {
                user_skills: Some(config_home.join("skills")),
                ..SkillScopes::default()
            });
        }
        if gates.project_enabled {
            for project_skills in project_skill_dirs_up_to_home(cwd) {
                manager.load_scoped(&SkillScopes {
                    project_skills: Some(project_skills),
                    ..SkillScopes::default()
                });
            }
        }
        if gates.additional_dirs_enabled {
            for dir in &gates.additional_dirs {
                manager.load_scoped(&SkillScopes {
                    project_skills: Some(dir.join(".coco").join("skills")),
                    ..SkillScopes::default()
                });
            }
        }
        // Legacy commands-as-skills: folds the deprecated `commands/` dirs
        // (managed → user → project up-to-home), both `SKILL.md` dirs and
        // flat `.md`. Dedup by canonical realpath inside `insert_disk`
        // (first-wins → the `skills/` copy loaded above).
        if gates.legacy_enabled {
            manager.load_legacy_command_scope(SettingScope::Managed, &get_managed_commands_path());
            manager.load_legacy_command_scope(SettingScope::User, &config_home.join("commands"));
            for project_commands in project_command_dirs_up_to_home(cwd) {
                manager.load_legacy_command_scope(SettingScope::Project, &project_commands);
            }
        }
    }

    manager
}

/// Standard skill directory paths by source, in loading priority order.
///
/// Order: managed → user → project `.coco/skills` walk-up.
pub fn get_skill_paths(config_dir: &Path, project_dir: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        // Enterprise/policy-managed skills (highest priority)
        get_managed_skills_path(),
        // User-level skills: ~/.coco/skills/
        config_dir.join("skills"),
    ];
    paths.extend(project_skill_dirs_up_to_home(project_dir));
    paths
}

pub(crate) fn project_skill_dirs_up_to_home(cwd: &Path) -> Vec<PathBuf> {
    project_dirs_up_to_home(cwd, "skills")
}

/// Legacy `.coco/commands` directories walked from `cwd` up to home,
/// mirroring [`project_skill_dirs_up_to_home`] but for the deprecated
/// commands subdir.
pub(crate) fn project_command_dirs_up_to_home(cwd: &Path) -> Vec<PathBuf> {
    project_dirs_up_to_home(cwd, "commands")
}

/// Walk from `cwd` upward, collecting `.coco/<subdir>` at each level, and stop
/// at the **git root OR home — whichever comes first**.
///
/// Stops after processing the git root specifically to "prevent commands from
/// parent directories outside the repository from appearing in the project
/// scope".
/// Without the git-root boundary, sibling repos sharing a parent dir (e.g.
/// `~/projects` holding a stray `~/projects/.coco/skills`) would leak skills
/// into every child repo. We detect the git root by the presence of a `.git`
/// entry — a directory for the main checkout, a file for a linked worktree —
/// which is equivalent to `git rev-parse --show-toplevel` for this boundary
/// without spawning a subprocess. Home remains the upper bound when cwd is not
/// inside a repo.
fn project_dirs_up_to_home(cwd: &Path, subdir: &str) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let mut dirs = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        dirs.push(current.join(".coco").join(subdir));
        // Stop after including the git root (project isolation).
        if current.join(".git").exists() {
            break;
        }
        if home.as_deref() == Some(current.as_path()) {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        if parent == current {
            break;
        }
        current = parent.to_path_buf();
    }
    dirs
}

/// Maximum characters per skill entry in the listing.
const MAX_LISTING_DESC_CHARS: usize = 250;

/// Result of building a skill listing with budget constraints.
pub struct SkillListingResult {
    pub listing: String,
    pub included: usize,
    pub total: usize,
}

/// Inject skill descriptions into a system prompt, respecting a character budget.
///
/// Caps at 1% of context window, max 250 chars per entry, bundled skills
/// never truncated.
///
/// The 4-state `skill_overrides` resolution layer applies three filters:
///
/// 1. `effective == Off` → skip the row entirely.
/// 2. `effective == NameOnly` → emit `- /name` with no description
///    (saves description tokens but keeps the name reachable).
/// 3. `XG$` — `disable_model_invocation && effective != On` → skip
///    (author intent + non-default state ⇒ hide from model).
///
/// With the default-empty [`coco_config::SkillOverrideTiers`] every
/// skill resolves to `On` so the filters are no-ops.
pub fn inject_skill_listing(
    skills: &[&SkillDefinition],
    max_budget_chars: usize,
    tiers: &coco_config::SkillOverrideTiers,
) -> SkillListingResult {
    if skills.is_empty() {
        return SkillListingResult {
            listing: String::new(),
            included: 0,
            total: 0,
        };
    }

    let total = skills.len();
    let mut listing = String::from("Available slash commands (skills):\n");
    let mut included = 0;

    // Bundled skills are always included (never truncated). Filters
    // still apply — a bundled skill explicitly `off`-overridden by
    // the user must drop out of the model listing.
    for skill in skills
        .iter()
        .filter(|s| matches!(s.source, SkillSource::Bundled))
    {
        let Some(mode) = skill_listing_mode(skill, tiers) else {
            continue;
        };
        listing.push_str(&format_skill_entry(skill, mode));
        included += 1;
    }

    // Non-bundled skills, subject to budget
    for skill in skills
        .iter()
        .filter(|s| !matches!(s.source, SkillSource::Bundled))
    {
        let Some(mode) = skill_listing_mode(skill, tiers) else {
            continue;
        };
        let entry = format_skill_entry(skill, mode);
        if listing.len() + entry.len() > max_budget_chars {
            break;
        }
        listing.push_str(&entry);
        included += 1;
    }

    SkillListingResult {
        listing,
        included,
        total,
    }
}

/// Per-skill listing mode after applying the 3-filter gate.
///
/// `None` ⇒ skip this skill entirely. `Some(Full)` ⇒ emit name +
/// description. `Some(NameOnly)` ⇒ emit just the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListingMode {
    Full,
    NameOnly,
}

fn skill_listing_mode(
    skill: &SkillDefinition,
    tiers: &coco_config::SkillOverrideTiers,
) -> Option<ListingMode> {
    let effective = crate::overrides::effective_skill_state(skill, tiers);
    // Filter 3 (XG$): author-DMI + non-default state → skip.
    if skill.disable_model_invocation && effective != coco_types::SkillOverrideState::On {
        return None;
    }
    match effective {
        coco_types::SkillOverrideState::Off => None,
        coco_types::SkillOverrideState::NameOnly => Some(ListingMode::NameOnly),
        // `user-invocable-only` still appears in the listing — the
        // Skill tool gate rejects model invocation, but the name
        // stays visible so the user-typed-slash bypass remains
        // discoverable.
        coco_types::SkillOverrideState::UserInvocableOnly | coco_types::SkillOverrideState::On => {
            Some(ListingMode::Full)
        }
    }
}

/// Char-safe truncation: returns `s` unchanged when within `max` chars, else
/// the first `max - 3` chars plus `...`. Never slices a multi-byte boundary
/// (the old byte-index `&s[..n]` form panicked on UTF-8 descriptions).
fn truncate_with_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(3)).collect();
    out.push_str("...");
    out
}

/// Format a single skill entry for the listing, capping description length.
fn format_skill_entry(skill: &SkillDefinition, mode: ListingMode) -> String {
    let mut entry = format!("- /{}", skill.name);
    if matches!(mode, ListingMode::Full) {
        if !skill.description.is_empty() {
            entry.push_str(&format!(
                ": {}",
                truncate_with_ellipsis(&skill.description, MAX_LISTING_DESC_CHARS)
            ));
        }
        if let Some(when) = &skill.when_to_use {
            let remaining = MAX_LISTING_DESC_CHARS.saturating_sub(entry.chars().count());
            if remaining > 20 {
                entry.push_str(&format!(
                    " - {}",
                    truncate_with_ellipsis(when, remaining - 5)
                ));
            }
        }
    }
    entry.push('\n');
    entry
}

/// Get the invocable skills (those available as `/commands`) as shared
/// `Arc`s.
///
/// Returns `Arc<SkillDefinition>` so callers iterate without deep-cloning
/// the heap-heavy `SkillDefinition` (see [`SkillManager::all`]).
pub fn get_invocable_skills(manager: &SkillManager) -> Vec<Arc<SkillDefinition>> {
    manager.all().into_iter().filter(|s| !s.disabled).collect()
}

/// Generate the SkillTool system prompt with skill listing.
///
/// Generates instruction text explaining how to invoke skills, plus the
/// formatted skill listing.
///
/// `tiers` drives the 4-state override filters applied inside
/// [`inject_skill_listing`]. Pass [`coco_config::SkillOverrideTiers::default()`]
/// when no override configuration is available — the default-empty
/// tiers preserve the pre-gate listing output.
pub fn generate_skill_tool_prompt(
    skills: &[&SkillDefinition],
    context_window_tokens: i64,
    tiers: &coco_config::SkillOverrideTiers,
) -> SkillListingResult {
    // Budget: 1% of context window × 4 chars/token (default 8000 chars).
    let budget = ((context_window_tokens as f64 * 0.01 * 4.0) as usize).max(2000);

    let mut result = inject_skill_listing(skills, budget, tiers);

    if !result.listing.is_empty() {
        let instructions = "\
The following skills are available for use with the Skill tool:

";
        result.listing = format!("{instructions}{}", result.listing);
    }

    result
}

/// Dynamically discover skills from a directory encountered during file operations.
///
/// Dynamic skill discovery triggered during Read/Write/Glob tool execution.
/// Skills found here are inserted after plugins but before built-in commands.
pub fn discover_dynamic_skills(dir: &Path) -> Vec<SkillDefinition> {
    let skills_dir = dir.join(".coco").join("skills");
    if !skills_dir.is_dir() {
        return Vec::new();
    }
    discover_skills(&[skills_dir])
}

/// Expand brace patterns in a glob string.
///
/// Recursively expands `*.{ts,tsx}` → `["*.ts", "*.tsx"]` and nested
/// `{a,{b,c}}` patterns.
pub fn expand_braces(pattern: &str) -> Vec<String> {
    // Find the first top-level brace group
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    // Find matching close brace (respecting nesting)
    let mut depth = 0;
    let mut close = None;
    for (i, ch) in pattern[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![pattern.to_string()];
    };

    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let inner = &pattern[open + 1..close];

    // Split on top-level commas only (not nested)
    let alternatives = split_top_level_commas(inner);

    alternatives
        .into_iter()
        .flat_map(|alt| {
            let combined = format!("{prefix}{alt}{suffix}");
            expand_braces(&combined)
        })
        .collect()
}

/// Split a string on commas that are not inside nested braces.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Estimate the token count for a skill's frontmatter (name +
/// description + `when_to_use`).
///
/// Sums `name`, `description`, and `when_to_use` with a small overhead
/// constant since char/token ratio swamps the join-space difference.
pub fn estimate_skill_tokens(skill: &SkillDefinition) -> i64 {
    (estimate_skill_frontmatter_bytes(skill) / 4) as i64
}

/// Estimate of a skill's frontmatter character length. The `/skills` dialog
/// divides this by the current model's bytes-per-token ratio to
/// render the token column, so the byte count is more useful than
/// the pre-divided token estimate when the model is mutable mid-
/// session.
pub fn estimate_skill_frontmatter_bytes(skill: &SkillDefinition) -> usize {
    let when_to_use = skill.when_to_use.as_deref().unwrap_or("").len();
    skill.name.len() + skill.description.len() + when_to_use + 20
}

/// Normalize `file_path` to a cwd-relative path suitable for gitignore
/// matching. Returns `None` for paths outside `cwd`:
/// - empty after normalization
/// - escapes via `..`
/// - absolute after relativization (Windows cross-drive case)
fn relative_to_cwd(file_path: &Path, cwd: &Path) -> Option<PathBuf> {
    let rel: PathBuf = if file_path.is_absolute() {
        file_path.strip_prefix(cwd).ok()?.to_path_buf()
    } else {
        file_path.to_path_buf()
    };
    if rel.as_os_str().is_empty() || rel.is_absolute() {
        return None;
    }
    if rel
        .components()
        .next()
        .map(|c| matches!(c, std::path::Component::ParentDir))
        .unwrap_or(true)
    {
        return None;
    }
    Some(rel)
}

/// Build a `Gitignore` matcher anchored at `cwd` from a skill's
/// `paths` list. Returns `None` if the pattern set is empty or every
/// `add_line` call errored (the latter shouldn't happen in practice —
/// `paths` is sanitized at parse time, see `parse_skill_markdown`).
fn build_skill_path_matcher(cwd: &Path, paths: &[String]) -> Option<ignore::gitignore::Gitignore> {
    if paths.is_empty() {
        return None;
    }
    let mut builder = ignore::gitignore::GitignoreBuilder::new(cwd);
    let mut any_ok = false;
    for pattern in paths {
        if builder.add_line(None, pattern).is_ok() {
            any_ok = true;
        }
    }
    if !any_ok {
        return None;
    }
    builder.build().ok()
}

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;
