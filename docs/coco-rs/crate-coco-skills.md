# coco-skills — Crate Plan

TS source: `skills/` (20 files, 1.35K LOC)

## Dependencies

```
coco-skills depends on:
  - coco-types (ThinkingLevel), coco-config, utils/frontmatter

coco-skills does NOT depend on:
  - coco-tools, coco-query, coco-inference, any app/ crate
```

## Data Definitions

```rust
pub struct SkillDefinition {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub argument_hint: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub model: Option<String>,
    pub thinking_level: Option<ThinkingLevel>,
    pub context: SkillContext,  // Inline or Fork (subagent)
    pub agent: Option<String>,
    pub hooks: Option<serde_json::Value>,  // Deserialized as HooksSettings by coco-hooks at runtime (config isolation pattern)
    pub paths: Option<Vec<String>>,  // glob patterns for applicability
    pub prompt_content: String,       // markdown body
}

pub enum SkillContext { Inline, Fork }
```

## Core Logic

```rust
pub struct SkillManager {
    skills: Vec<SkillDefinition>,
}

impl SkillManager {
    /// Multi-source discovery (loading order determines precedence):
    /// 1. Bundled skills (remember, verify, loop, claude-api, batch, etc.)
    /// 2. Plugin skills (via PluginContributions)
    /// 3. User skills: ~/.claude/skills/*.md
    /// 4. Project skills: .claude/skills/*.md
    /// 5. Managed skills (enterprise-provisioned)
    ///
    /// Deduplication: realpath canonical path comparison.
    /// If same file reached via symlink, loaded only once.
    pub fn load(cwd: &Path) -> Self;

    /// Parse YAML frontmatter + markdown body from .md files.
    /// Frontmatter fields: name, description, whenToUse, argumentHint,
    /// tools, model, effort, context (Inline|Fork), agent, hooks,
    /// paths (glob patterns for conditional activation).
    fn parse_skill_file(path: &Path) -> Result<SkillDefinition, SkillError>;
    pub fn find(&self, name: &str) -> Option<&SkillDefinition>;
    pub fn to_commands(&self) -> Vec<Command>;

    /// Conditional activation: only activate skills whose `paths` glob
    /// matches the current working directory or edited files.
    pub fn activate_for_paths(&mut self, paths: &[&Path]);

    /// Memoization: skill list cached per session.
    /// Invalidated on: skill file change (via file watcher), plugin reload.
    pub fn clear_caches(&mut self);
}

/// Bundled skills registry (compiled into binary).
/// Security: bundled skill files extracted to per-process nonce directory
/// (~/.claude/bundled-skills/<nonce>/), written with O_NOFOLLOW | O_EXCL
/// (symlink attack prevention), dir mode 0o700, file mode 0o600.
/// Path traversal validated (no ".." allowed).
/// "Base directory for this skill: <dir>" prepended to prompt.
pub fn get_bundled_skills() -> Vec<SkillDefinition>;

/// Token estimation for skill frontmatter (used in context budgeting).
pub fn estimate_skill_frontmatter_tokens(skill: &SkillDefinition) -> i64;
```

## Dynamic Discovery (from `utils/skills/skillChangeDetector.ts`, 311 LOC)

```rust
/// Watches skill directories for changes and triggers cache invalidation.
/// Uses debounced file watching (300ms debounce, 1s stability threshold).
///
/// Watched paths: ~/.coco/skills, ~/.coco/commands (user)
///                .claude/skills, .claude/commands (project)
///                + additional dirs from --add-dir flags
///
/// State machine:
///   initialize() → watcher created, callback registered
///   file change event → scheduleReload(path)
///   scheduleReload → add to pending_paths, set 300ms debounce timer
///   timer fires → execute ConfigChange hooks (can block) → clear caches → emit signal
///   dispose() → close watcher, clear timer
///
/// Hook integration: executeConfigChangeHooks('skills', path) can block reload.
pub struct SkillChangeDetector {
    initialized: bool,
    disposed: bool,
    watcher: Option<FileWatcher>,
    reload_timer: Option<JoinHandle<()>>,
    pending_changed_paths: HashSet<PathBuf>,
}
```
