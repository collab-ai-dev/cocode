# coco-context — Crate Plan

TS source: `context.ts`, `utils/systemPromptType.ts`, `utils/attachments.ts`, `utils/claudemd.ts`, `utils/cwd.ts`, `utils/fileHistory.ts` (~1110 LOC), `utils/fileStateCache.ts` (1.5K LOC), `utils/fileRead.ts`

## Dependencies

```
coco-context depends on:
  - coco-types   (Message, Attachment types, PermissionMode)
  - coco-config  (ModelInfo — for context_window, max_output_tokens)
  - coco-error
  - utils/git    (git status)

coco-context does NOT depend on:
  - coco-inference (no LLM calls)
  - coco-tool (no tool types)
  - any app/ crate
```

## Data Definitions

### System Context (from `context.ts`)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform { Darwin, Linux, Windows }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellKind { Bash, Zsh, Sh, PowerShell }

/// Collected once per turn, injected into system prompt
pub struct SystemContext {
    pub cwd: PathBuf,
    pub platform: Platform,
    pub shell: ShellKind,
    pub os_version: String,
    pub model: String,                // current model name (dynamic, stays String)
    pub knowledge_cutoff: String,     // "May 2025"
    pub current_date: String,
    pub git_status: Option<GitStatus>,
}

pub struct GitStatus {
    pub branch: String,
    pub main_branch: String,
    pub user: String,
    pub status: String,           // short status output
    pub recent_commits: String,   // last 5 commits
}

pub fn get_system_context() -> SystemContext;
pub fn get_git_status(cwd: &Path) -> Option<GitStatus>;
```

### File History & Encoding (from `fileHistory.ts` ~1110 LOC, `fileRead.ts`, `fileStateCache.ts` 1.5K LOC)

```rust
/// File edit tracking per turn: content-addressed backup files on disk.
/// Used by /rewind command to restore file state to a previous turn.
/// IMPORTANT: Uses an **ordered Vec** (not HashMap) — order matters for
/// findLast() and snapshots.last() operations.
pub struct FileHistoryState {
    /// Ordered array of snapshots (newest last), capped at MAX_SNAPSHOTS=100.
    /// Evicts oldest when full; snapshot_sequence never resets.
    snapshots: Vec<FileHistorySnapshot>,
    tracked_files: HashSet<PathBuf>,
    snapshot_sequence: i64,  // monotonically increasing (survives eviction)
}

pub struct FileHistorySnapshot {
    pub message_id: Uuid,
    pub tracked_file_backups: HashMap<PathBuf, FileHistoryBackup>,
    pub timestamp: DateTime<Utc>,
}

/// Content-addressed backup file on disk at:
///   ~/.claude/file-history/{session_id}/{sha256_prefix}@v{N}
/// backup_file_name=None means file did not exist at that version.
pub struct FileHistoryBackup {
    pub backup_file_name: Option<String>,
    pub version: i32,
    pub backup_time: DateTime<Utc>,
}

pub enum FileEncoding { Utf8, Latin1, Binary }
pub enum LineEnding { Lf, CrLf, Cr }

impl FileHistoryState {
    /// Record a file's content before modification.
    pub fn track_edit(&mut self, message_uuid: &str, path: &Path);
    /// Create a snapshot at a specific message point (for /rewind).
    pub fn make_snapshot(&self, message_uuid: &str) -> HashMap<PathBuf, FileSnapshot>;
}

/// File read with encoding detection and line ending preservation.
/// Detects encoding via BOM or byte analysis.
/// Preserves line endings (LF vs CRLF) on write-back.
pub fn read_file_with_metadata(path: &Path) -> Result<(String, FileEncoding, LineEnding), io::Error>;

/// LRU file read cache (per turn, up to 50 entries).
/// Avoids re-reading unchanged files within a turn.
pub struct FileStateCache {
    cache: lru::LruCache<PathBuf, CachedFileState>,
}
```

### Attachments (from `utils/attachments.ts` — 38K LOC, 50+ types)

The `getAttachments()` function is the main per-turn injection engine.
It runs three parallel batches with a 1000ms timeout per batch:

**Batch 1: User-input attachments** (triggered by @-mentions in user text):
- `at_mentioned_files` — FileAttachment, AlreadyReadFile, PdfReference, directory listing
- `mcp_resources` — MCP resource content
- `agent_mentions` — @agent references
- `skill_discovery` — feature-gated experimental skill search

**Batch 2: All-thread attachments** (every turn, main + sub-agents):
- `queued_commands` — user prompt queue (carries text, images, provenance)
- `date_change` — midnight crossing detection (does NOT clear getUserContext cache)
- `ultrathink_effort` — triggers extended thinking on keyword
- `deferred_tools_delta` — newly-available ToolSearch tools
- `agent_listing_delta` — diff of agent types vs already-announced
- `mcp_instructions_delta` — diff of MCP server instructions
- `changed_files` — files modified since last turn (FileStateCache)
- `nested_memory` — per-turn context-sensitive CLAUDE.md rules for @-mentioned files
- `relevant_memories` — async prefetch (up to 5 files × 4KB = 20KB/turn, 60KB session cap)
- `plan_mode` — full/sparse reminder on 5-turn cycle
- `plan_mode_exit` — one-shot on exit
- `auto_mode` / `auto_mode_exit` — transcript classifier mode
- `todo_reminders` / `task_reminder` — every 10 turns after 10 turns since last write
- `teammate_mailbox` — inter-agent DMs in swarm mode
- `team_context` — swarm team metadata
- `agent_pending_messages` — drained from LocalAgentTask queue
- `compaction_reminder` — 1M context only, fires above 25% usage

**Batch 3: Main-thread-only attachments**:
- `ide_selection` — selected lines from IDE
- `ide_opened_file` — currently open file in IDE
- `output_style` — prose style instruction
- `diagnostics` + `lsp_diagnostics` — IDE/LSP error files
- `unified_tasks` — todo/task list
- `async_hook_responses` — results from async hooks
- `token_usage` — env-gated usage attachment
- `budget_usd` — remaining budget display

```rust
pub enum Attachment {
    File(FileAttachment),
    PdfReference(PdfReferenceAttachment),
    AlreadyReadFile(AlreadyReadFileAttachment),
    AgentMention(AgentMentionAttachment),
    Hook(HookAttachment),
    Memory(MemoryAttachment),
    TeammateMail(TeammateMailboxAttachment),
    // Plus ~40 more inline types that use generic SystemReminder format:
    // date_change, plan_mode, queued_commands, changed_files, nested_memory,
    // relevant_memories, deferred_tools_delta, agent_listing_delta, mcp_instructions_delta,
    // compaction_reminder, todo_reminders, ide_selection, diagnostics, token_usage, etc.
    SystemReminder { attachment_type: String, content: String },
}

pub struct FileAttachment {
    pub filename: String,
    pub content: String,
    pub truncated: bool,
    pub display_path: String,
}

pub struct PdfReferenceAttachment {
    pub filename: String,
    pub page_count: i32,
    pub file_size: i64,
    pub display_path: String,
}

pub enum HookAttachment {
    Success { content: String, hook_name: String },
    BlockingError { error: HookBlockingError },
    Cancelled { hook_name: String },
    PermissionDecision { decision: PermissionBehavior },
}
```

### Memory-File Discovery (from `utils/claudemd.ts`, `utils/attachments.ts`)

Two passes — eager (once at session start) and lazy (per file-read trigger). Both
share a single `processed: HashSet<PathBuf>` so a file never loads twice across
the session, regardless of which pass touched it first.

```rust
// Modules: `claudemd`, `nested_memory`, `claudemd_imports`, `claude_rules`,
// `memory_filenames`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryFileSource { UserGlobal, ProjectConfig, Project, Local }

#[derive(Debug, Clone)]
pub struct MemoryFile {                  // eager-pass output
    pub path: PathBuf,
    pub content: String,
    pub source: MemoryFileSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedMemoryEntry {           // lazy-pass output
    pub path: PathBuf,
    pub content: String,
    pub source: MemoryFileSource,
}

/// Filename helpers (TS divergence: coco-rs supports both CLAUDE.md AND
/// AGENTS.md case-insensitively; TS only matches CLAUDE.md literally).
pub const MEMORY_FILE_CANDIDATES: &[&str] = &["CLAUDE.md", "AGENTS.md"];
pub const MEMORY_LOCAL_FILE_CANDIDATES: &[&str] = &["CLAUDE.local.md", "AGENTS.local.md"];
pub fn find_memory_files(dir: &Path, candidates: &[&str]) -> Vec<PathBuf>;

/// Eager pass — root → CWD inclusive. Each loaded file is fed through
/// `expand_imports` so `@./other.md` references materialise in the same call.
pub fn discover_memory_files(cwd: &Path) -> Vec<MemoryFile>;

/// Backward-compat alias; new code uses `discover_memory_files`.
#[doc(hidden)]
pub fn discover_claude_md_files(cwd: &Path) -> Vec<MemoryFile>;

/// Lazy pass — fired per file-read trigger by
/// `app/query::QueryEngine::drain_nested_memory_triggers` at end of every
/// turn batch. Loads memory files surfaced by reading `file`, deduped
/// against `loaded` so eagerly-loaded paths never re-emit.
///
/// Phases (TS parity):
/// 1. Managed (`/etc/coco/rules`) + user (`~/.coco/rules`) **conditional**
///    rules whose `paths:` glob matches `file`.
/// 2. `directories_to_process(file, cwd)` → `(nested_dirs, cwd_level_dirs)`.
/// 3. Per-`nested_dir` (CWD-exclusive → file-parent-inclusive): load
///    `{CLAUDE,AGENTS}.md`, `.claude/CLAUDE.md`, local files, and
///    `.claude/rules/**/*.md` (unconditional + matching conditional).
/// 4. Per-`cwd_level_dir` (root → CWD inclusive): only conditional rules
///    matching `file` — unconditional content was loaded eagerly.
pub fn traverse_for_file(
    file: &Path,
    cwd: &Path,
    loaded: &mut HashSet<PathBuf>,
) -> Vec<LoadedMemoryEntry>;

/// CWD/file split helper used by `traverse_for_file`.
pub fn directories_to_process(file: &Path, cwd: &Path) -> (Vec<PathBuf>, Vec<PathBuf>);

// `@import` directive (TS `extractIncludePathsFromTokens` /
// `processMemoryFile`).
pub const MAX_INCLUDE_DEPTH: u32 = 5;
pub const TEXT_FILE_EXTENSIONS: &[&str] = &[/* md, txt, rst, json, ts, rs, ... */];
pub fn extract_include_paths(body: &str) -> Vec<String>;     // skips fenced + inline code
pub fn resolve_at_path(token: &str, base_dir: &Path) -> Option<PathBuf>;
pub fn is_text_extension(path: &Path) -> bool;
pub fn expand_imports(
    path: &Path,
    content: &str,
    processed: &mut HashSet<PathBuf>,
    depth: u32,
) -> Vec<(PathBuf, String)>;

// `.claude/rules/*.md` discovery + frontmatter `paths:` glob matching.
pub struct RuleFile {
    pub path: PathBuf,
    pub content: String,           // frontmatter-stripped
    pub paths: Option<Vec<String>>, // None = unconditional, Some = conditional
}
pub fn parse_paths_field(input: &coco_frontmatter::FrontmatterValue) -> Option<Vec<String>>;
pub fn collect_rule_files(rules_dir: &Path, conditional: bool) -> Vec<RuleFile>;
pub fn filter_rules_matching(
    rules: Vec<RuleFile>,
    target_file: &Path,
    base_dir: &Path,
) -> Vec<RuleFile>;
```

#### Status of TS features

| TS feature | Rust status |
|-----------|-------------|
| Managed (`/etc/claude-code/`) | Done — `phase1_managed_user_conditional_rules` (Phase 1) reads `/etc/coco/rules` |
| User (`~/.claude/`) | Done — Phase 1 reads `~/.coco/rules`, eager pass reads `~/.coco/{CLAUDE,AGENTS}.md` |
| Project + Local + `.claude/CLAUDE.md` | Done — eager pass walks root → CWD; lazy pass walks CWD → file |
| `@import` recursion + cycle break + depth cap (5) | Done — `claudemd_imports::expand_imports` |
| Text-extension allowlist (binary blocklist) | Done — `is_text_extension` (representative subset of TS list) |
| Conditional `paths:` rules (gitignore-style globs) | Done — `claude_rules::filter_rules_matching` via `ignore` crate |
| Brace-expansion in `paths` (`src/*.{ts,tsx}`) | Done — `expand_braces` |
| AGENTS.md / case-insensitive filenames | Done — coco-rs **divergence** from TS, see `memory_filenames` |
| Additional directories (`--add-dir`) | Deferred (P3) |
| AutoMem (`MEMORY.md` memdir entrypoint) | Owned by `coco-memory` crate; this crate just discovers files |
| TeamMem (`<team-memory-content>` wrapper) | Deferred (v2 — requires team-sync infrastructure) |
| `claudeMdExcludes` settings filter | Deferred (P3) |
| HTML-comment stripping (`<!-- -->`) | Deferred — content passes through verbatim |
| `MAX_MEMORY_CHARACTER_COUNT=40000` per-file truncation | Deferred — content passes through |

### System Prompt Building (from `utils/systemPromptType.ts`)

```rust
pub struct SystemPrompt {
    pub blocks: Vec<SystemPromptBlock>,
}

pub enum SystemPromptBlock {
    Text(String),
    CacheBreakpoint,
}

/// Build system prompt from:
/// 1. Identity block (model name, date, knowledge cutoff)
/// 2. Tool policy block (tool descriptions, usage instructions)
/// 3. Security block (safety guidelines)
/// 4. CLAUDE.md memory files
/// 5. Environment block (cwd, git status, platform)
/// 6. Permission mode block
/// 7. Custom system prompt (if configured)
pub fn build_system_prompt(
    context: &SystemContext,
    memory_files: &[MemoryFileInfo],
    tool_descriptions: &[String],
    config: &SystemPromptConfig,
) -> SystemPrompt;
```

### Context Window (from `utils/context.ts`)

```rust
// Constants (defaults, not hardcoded per-model)
pub const COMPACT_MAX_OUTPUT_TOKENS: i64 = 20_000;
pub const CAPPED_DEFAULT_MAX_TOKENS: i64 = 8_000;
pub const ESCALATED_MAX_TOKENS: i64 = 64_000;

// Reads from ModelInfo (coco-config), NOT hardcoded
pub fn get_context_window(model_info: &ModelInfo) -> i64 {
    model_info.context_window  // from coco-config, default 200_000
}
pub fn get_max_output_tokens(model_info: &ModelInfo) -> (i64, i64);
pub fn calculate_context_pct(usage: &TokenUsage, window: i64) -> (f64, f64);
```
