# Nested-Memory Feature Analysis & Optimization Plan

**Status**: implemented (Phases 0–6 done; remaining items listed under
"Remaining deferrals" at the end of this document).
**Scope**: nested CLAUDE.md attachment pipeline triggered by file reads —
comparison of TS reference vs current Rust (`coco-rs/`). TS file paths are
relative to the TS project's `src/` directory.

## TL;DR (current state)

- ✅ Read tool's `ctx.nested_memory_attachment_triggers` is **drained**
  end-of-batch by `QueryEngine::drain_nested_memory_triggers`
  (`app/query/src/engine_attachments.rs`); each trigger calls
  `coco_context::traverse_for_file` and stages results into
  `pending_nested_memory` for the next reminder build.
- ✅ Engine carries `loaded_nested_memory_paths: Arc<Mutex<HashSet>>`
  for session-level dedup, fed through `expand_imports` so a file
  loaded eagerly never re-emits lazily.
- ✅ `coco-context` ships `claudemd_imports` (`@import` recursion,
  cycle break, depth cap 5, binary-blob blocklist), `claude_rules`
  (frontmatter `paths:` parsing + brace expansion + gitignore matching
  via `ignore` crate), `nested_memory` (the four-phase TS-faithful
  traversal — managed/user conditional rules, per-nested-dir loads,
  CWD-level conditional rules), and `memory_filenames` (CLAUDE.md +
  AGENTS.md, case-insensitive — coco-rs **divergence** from TS).
- ✅ Eager `discover_memory_files` rewritten to TS-faithful root → CWD
  inclusive walk; the buggy "immediate children" + parent-depth-10
  hack is removed. Eager and lazy passes share a single canonical-path
  dedup set so `@import` chains never double-load.
- ✅ Test coverage: 233 lib tests + 5 e2e tests in
  `core/context/tests/nested_memory_e2e.rs` (cycle break, cross-pass
  dedup, Phase 4 conditional-rule matching, AGENTS.md alongside
  CLAUDE.md, content preservation through `@import` expansion).

The original pre-implementation TL;DR follows for historical reference.

## TL;DR (original, pre-implementation)

The Rust port has **the scaffolding but a broken data flow**, and the existing
"eager" loader is also subtly **wrong**:

- ✅ Types exist: `NestedMemoryAttachment`, `AttachmentKind::NestedMemory`, `NestedMemoryGenerator`, `MemorySource::nested_memories`.
- ✅ The Read tool populates `ctx.nested_memory_attachment_triggers` on every file read (`core/tools/src/lib.rs:357-366`).
- ✅ The reminder orchestrator wires `materialized.nested_memories` into `TurnReminderInput` and the `NestedMemoryGenerator` renders identically to TS.
- ❌ **Nothing drains `nested_memory_attachment_triggers`.** It is a write-only black hole. The only references are the populator + tests + one `Default::default()` in `tool_context.rs:233`.
- ❌ `MemoryAdapter::nested_memories()` is an explicit no-op (`reminder_adapters.rs:293-300`) with a comment claiming upstream `coco-context` handles it — but `coco-context` only does a single static traversal at prompt build (`engine_prompt.rs:48-53`, `app/cli/src/main.rs:497`).
- ❌ `loaded_nested_memory_paths` (session-level dedup) is a declared field nobody reads or writes.
- ❌ `coco-context::claudemd` is **98 lines** (vs TS's 1500-line `claudemd.ts`): no `@import` directive, no `MAX_INCLUDE_DEPTH`, no circular-include guard, no `.claude/rules/*.md` conditional rules, no glob-based filtering, no HTML-comment / frontmatter stripping.
- ⚠️ **Existing eager loader is wrong vs TS**: it walks parents up to depth 10 *and* loads `CLAUDE.md` from **immediate children** of CWD (`claudemd.rs:60-67`). TS does **neither** — TS eager walks `root → CWD` inclusive only. The "immediate children" path looks like a hacky compensation for the missing trigger pipeline; once the trigger pipeline lands, this MUST be removed or it will double-load every CLAUDE.md the trigger pipeline finds.

Net effect: reading `/proj/src/auth/handler.rs` (CWD = `/proj`) loads `/proj/CLAUDE.md` (eager, correct) plus a shotgun of `<every-immediate-child-of-/proj>/CLAUDE.md` (eager, *wrong*), but **none** of `/proj/src/CLAUDE.md`, `/proj/src/auth/CLAUDE.md`, or any frontmatter-glob rule.

---

## 1. TS Reference Pipeline (claude-code)

### 1.1 Trigger sites — `FileReadTool.ts`

```ts
// FileReadTool.ts:848  (notebook)
// FileReadTool.ts:870  (image)
// FileReadTool.ts:1038 (text)
context.nestedMemoryAttachmentTriggers?.add(fullFilePath)
```

Set is initialized fresh per-query in three places:
- `QueryEngine.ts:370,518`
- `forkedAgent.ts:382` (per subagent — isolated)
- `REPL.tsx:2480` (per message)

A separate `loadedNestedMemoryPathsRef` (`REPL.tsx:1964-1967`) survives across turns
for session-level dedup.

### 1.2 Drain — `utils/attachments.ts:2167-2194`

```ts
async function getNestedMemoryAttachments(toolUseContext): Promise<Attachment[]> {
  if (!toolUseContext.nestedMemoryAttachmentTriggers ||
      toolUseContext.nestedMemoryAttachmentTriggers.size === 0) return []
  const attachments: Attachment[] = []
  for (const filePath of toolUseContext.nestedMemoryAttachmentTriggers) {
    attachments.push(...await getNestedMemoryAttachmentsForFile(filePath, ...))
  }
  toolUseContext.nestedMemoryAttachmentTriggers.clear()  // ← drain semantics
  return attachments
}
```

Called from the attachment batch builder (`attachments.ts:872`) at the gap between
tool batch completion and the next API call.

### 1.3 Per-file traversal — `getNestedMemoryAttachmentsForFile` (`attachments.ts:1792-1862`)

Four phases for each triggered file `X`:

1. **Phase 1**: Managed (`/etc/claude-code/.claude/rules/*.md`) + User (`~/.claude/rules/*.md`) **conditional** rules whose frontmatter `paths` glob matches `X`.
2. **Phase 2**: split filesystem into `nestedDirs` (CWD → X, **CWD exclusive, file-parent inclusive, must startsWith CWD**) and `cwdLevelDirs` (root → CWD inclusive) via `getDirectoriesToProcess` (`attachments.ts:1656-1689`).
3. **Phase 3**: walk `nestedDirs` loading per directory:
   - `CLAUDE.md`
   - `.claude/CLAUDE.md`
   - `CLAUDE.local.md`
   - `.claude/rules/*.md` (unconditional + conditional matching X) — **`processMdRules` recurses into subdirs of `.claude/rules/`** with cycle-detection on visited dirs (`claudemd.ts:711-779`).
4. **Phase 4**: walk `cwdLevelDirs` loading **only** conditional `.claude/rules/*.md` matching X.

**Concrete trace** — CWD `/proj`, trigger file `/proj/src/auth/handler.rs`:

| Phase | Directory | Files attempted |
|---|---|---|
| 1 | `/etc/claude-code/.claude/rules/` + `~/.claude/rules/` | conditional `*.md` matching `src/auth/handler.rs` (recursive) |
| 3 | `/proj/src` | `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md`, `.claude/rules/**/*.md` (uncond + cond match) |
| 3 | `/proj/src/auth` | same set |
| 4 | `/`, `/home`, `/home/user`, `/proj` | only `.claude/rules/**/*.md` cond match |

**`/proj/CLAUDE.md` is NOT in this list** — it was loaded eagerly at session start (§1.8). The trigger pipeline strictly fills in dirs **between CWD and the file**.

If the read file is **outside** CWD (e.g. `/etc/foo.conf` with CWD `/proj`), `nestedDirs` is empty (the `startsWith(originalCwd)` filter rejects all candidates) and only Phase 1 + Phase 4 fire.

### 1.4 Dedup — `memoryFilesToAttachments` (`attachments.ts:1710-1775`)

Two-level:
- `loadedNestedMemoryPaths` (session Set, never evicts) — primary
- `readFileState` (LRU 100) — secondary, prevents re-injection within tool batch

When path is hit: emit `{type: 'nested_memory', path, content, displayPath}`,
populate both dedup stores, fire `InstructionsLoaded` hook with `loadReason ∈ {nested_traversal | path_glob_match | include}`.

### 1.5 `@import` directive — `claudemd.ts:451-685`

- Syntax: `@./relative`, `@~/home`, `@/absolute`, `@bare` (= `@./bare`).
- Parsed from text nodes only (skips fenced code blocks via `marked` AST walk).
- `TEXT_FILE_EXTENSIONS` allowlist (~100 extensions, `claudemd.ts:96-227`) blocks binary files.
- `MAX_INCLUDE_DEPTH = 5` (`claudemd.ts:537`) + `processedPaths: Set<string>` prevent loops.
- Symlink targets tracked separately for dedup (`claudemd.ts:646-648`).
- Included files emitted **AFTER** the parent in the result vec (parent first via `result.push(memoryFile)` then includes via the loop, `claudemd.ts:661-682`). The "parent overrides child" ordering is achieved by the model paying *more* attention to later messages — TS files are loaded with the latest having highest priority (see header comment).
- `claudeMdExcludes` setting (`claudemd.ts:547`) lets users blacklist paths from being loaded at all.
- `safelyReadMemoryFileAsync` (`claudemd.ts:424`) strips HTML comments + frontmatter before storing in `content`; raw bytes go in `rawContent` if `contentDiffersFromDisk`.

### 1.5b Soft length cap

`MAX_MEMORY_CHARACTER_COUNT = 40000` (`claudemd.ts:92`) — not a hard truncate, but `getOversizedMemoryFiles` (`claudemd.ts:1133`) flags files past this limit so the UI can warn. Worth porting as a config-driven warning, not a truncation.

### 1.6 Frontmatter `paths` — `frontmatterParser.ts:189-232`

`splitPathInFrontmatter` handles:
- comma-separated string OR YAML array
- brace expansion: `src/*.{ts,tsx}` → `[src/*.ts, src/*.tsx]`
- nested braces: `{a,b}/{c,d}` → 4 entries
- normalization: drop trailing `/**`, drop `**`-only entries

Matched against the relative-from-base file path with the `ignore` library (gitignore syntax).

### 1.7 Render — `messages.ts:3700-3707`

```ts
case 'nested_memory': {
  return wrapMessagesInSystemReminder([
    createUserMessage({
      content: `Contents of ${a.content.path}:\n\n${a.content.content}`,
      isMeta: true,
    }),
  ])
}
```

### 1.7b Memory filename matching (coco-rs extension)

**Divergence from TS**: TS only loads files literally named `CLAUDE.md` and
`CLAUDE.local.md` with exact case. coco-rs supports the broader
**memory-file** convention to interoperate with other agent tools (Codex
AGENTS spec, Cursor, etc.):

| Position | TS literal | coco-rs candidates (case-insensitive) |
|---|---|---|
| Repo / dir root | `CLAUDE.md` | `CLAUDE.md`, `AGENTS.md` |
| Local override | `CLAUDE.local.md` | `CLAUDE.local.md`, `AGENTS.local.md` |
| Config dir | `.claude/CLAUDE.md` | `.claude/CLAUDE.md` (claude-code-specific path; AGENTS.md not added here — `.claude/` is a coco/claude-code config dir, not a memory dir) |
| Rules | `.claude/rules/**/*.md` | unchanged — rules naming is content-defined (frontmatter `paths`), not basename-defined |

**Matching semantics**:

- Case-insensitive ASCII compare (`name.eq_ignore_ascii_case("CLAUDE.md")`) — covers `Claude.md`, `claude.md`, `CLAUDE.MD`, etc. Works regardless of host filesystem case-sensitivity (Linux ext4 vs macOS APFS vs Windows NTFS).
- If both `CLAUDE.md` and `AGENTS.md` exist in the same directory, **both load** — they're treated as separate memory entries with distinct paths in `loaded_nested_memory_paths` for dedup.
- Disk casing is preserved in the loaded path (e.g., `Claude.md` on disk surfaces as `Claude.md`, not `CLAUDE.md`) so the rendered `Contents of {path}:` reflects what the user actually has.
- Order within a directory: deterministic (alphabetical by lowercased basename) so identical filesystem trees produce identical reminder sequences across runs.

**Why this divergence is safe vs TS parity**: the `<system-reminder>` template
is unchanged, the dedup keys (absolute path strings) are unchanged, and the
trigger pipeline plumbing is unchanged — only the filename-resolution helper
is broader. TS users who don't have AGENTS.md see zero behavior change.

### 1.8 Eager load — `getMemoryFiles` (`claudemd.ts:790-960+`)

Memoized; runs once per session (cache cleared only on file-watch events).
Loads in this order (each later entry overrides earlier in model attention):

1. `/etc/claude-code/CLAUDE.md` (Managed) + its `.claude/rules/**/*.md` **unconditional** rules
2. `~/.claude/CLAUDE.md` (User) + its `~/.claude/rules/**/*.md` **unconditional** rules
3. For each dir from `/` walking down to CWD inclusive:
   - `dir/CLAUDE.md` (Project)
   - `dir/.claude/CLAUDE.md` (Project)
   - `dir/CLAUDE.local.md` (Local)
   - `dir/.claude/rules/**/*.md` (Project, **unconditional** only)

**Conditional rules are NEVER eager** — only Phase 1 (managed/user) and Phases 3–4 (project) of the per-file traversal load conditional rules.

**Worktree special-case** (`claudemd.ts:859-884`): when running from a nested git worktree (e.g. `.claude/worktrees/<name>/`), parents that live inside the canonical repo but outside the worktree are skipped for **Project** files (would double-load the same checked-in CLAUDE.md). `CLAUDE.local.md` is gitignored so it stays loaded.

---

## 2. Current Rust State

### 2.1 What exists ✅

| Layer | Component | File |
|---|---|---|
| Types | `NestedMemoryAttachment` | `core/context/src/attachment.rs` |
| Types | `AttachmentKind::NestedMemory` (+ `RelevantMemories`) | `common/types/src/attachment_kind.rs:80-81` |
| Types | `NestedMemoryInfo` / `RelevantMemoryInfo` | `core/system-reminder/src/generators/memory.rs:1-30` |
| Generator | `NestedMemoryGenerator::generate` — exact TS template | `core/system-reminder/src/generators/memory.rs:74-122` |
| Source trait | `MemorySource::nested_memories(agent_id, mentioned_paths)` | `core/system-reminder/src/sources/traits.rs:139-148` |
| Materialize | `MaterializedSources.nested_memories: Vec<NestedMemoryInfo>` | `core/system-reminder/src/sources/materialized.rs:71` |
| Wire | Engine builds `MaterializeContext` and passes to generator | `app/query/src/engine_turn_reminders.rs:335-347, 484` |
| Trigger field | `ToolUseContext.nested_memory_attachment_triggers: Arc<RwLock<HashSet<String>>>` | `core/tool-runtime/src/context.rs:177` |
| Trigger field | `ToolUseContext.loaded_nested_memory_paths: HashSet<String>` | `core/tool-runtime/src/context.rs:179` |
| Populator | `track_nested_memory_attachment` called from Read tool | `core/tools/src/lib.rs:357-366` + `tools/read.rs:383,391,414,420` |
| Static load | `discover_claude_md_files(cwd)` walks 6 sources at prompt build | `core/context/src/claudemd.rs:25-70`, called once in `engine_prompt.rs:48-53` |

### 2.2 What is broken / missing ❌

| Gap | Severity | Where it should live |
|---|---|---|
| **G1.** `nested_memory_attachment_triggers` never drained | Critical | `app/query/src/engine_finalize_turn.rs` (or new `engine_attachments.rs`) |
| **G2.** `MemoryAdapter::nested_memories()` is a no-op | Critical | `app/query/src/reminder_adapters.rs:293-300` |
| **G3.** `mentioned_paths` carries only @-mentions, not file-read triggers | Critical | `engine_turn_reminders.rs:311-327` builds it from `MentionType::FilePath` only |
| **G4.** `loaded_nested_memory_paths` declared but never populated/read | High | populator + drain need to update it |
| **G5.** No `@import` parser for CLAUDE.md text bodies | High | `core/context/src/claudemd.rs` (need ~500 LoC) |
| **G6.** No `.claude/rules/*.md` frontmatter conditional rules | High | new module under `core/context/src/` |
| **G7.** No CWD↔file directory split (`nestedDirs` / `cwdLevelDirs`) | High | helper inside the new traversal module |
| **G8.** Static `discover_claude_md_files` only walks parent (depth 10) + immediate children — not the full CWD→file path | Medium | rewrite + delegate per-file walking to (G7) |
| **G9.** `dynamic_skill_dir_triggers` has the same drain bug (parallel pattern, populated by `track_skill_discovery`, never drained in `app/`) | Medium | same drain site as (G1) |
| **G10.** No `MAX_INCLUDE_DEPTH` / circular-include guard | Medium | part of (G5) |
| **G11.** No symlink-target-aware dedup | Low | part of (G5) + (G7) |
| **G12.** Generator never tracks "loadReason" → no audit hook | Low | optional — add later if hooks crate exposes `InstructionsLoaded` event |
| **G13.** Existing eager loader walks parents+immediate-children, not root→CWD | High | rewrite `discover_claude_md_files` in `core/context/src/claudemd.rs` |
| **G14.** No HTML-comment / frontmatter stripping in eager-loaded CLAUDE.md content | Medium | port `safelyReadMemoryFileAsync` |
| **G15.** No `claudeMdExcludes` user-setting support | Low | part of (G5) |
| **G16.** No worktree-aware skip in eager loader (will double-load when CWD is inside `.claude/worktrees/<name>/`) | Medium | part of (G13) |
| **G17.** No `MAX_MEMORY_CHARACTER_COUNT` warning surface | Low | TUI-only, defer |
| **G18.** `processMdRules`-equivalent missing — needs recursive `.md` walk in `.claude/rules/` with cycle detection | Medium | part of (G6) |
| **G19.** `clone_for_subagent` shares the trigger `Arc<RwLock>` across subagents (TS gives each a fresh Set) | Medium | fix in `core/tool-runtime/src/context.rs:474` when wiring drain |
| **G20.** No `AGENTS.md` support — coco-rs serves multi-tool ecosystems (Codex AGENTS spec, Cursor) and must load `AGENTS.md` alongside `CLAUDE.md` at the same paths | High | new helper `find_memory_files(dir, candidates)` used by both eager loader and per-file traversal |
| **G21.** Filename match is case-sensitive — breaks on `Claude.md` / `agents.md` and on case-insensitive filesystems where the user committed lowercase | High | same helper as G20: `eq_ignore_ascii_case` against candidate set |

### 2.3 Confirming the dead path

```bash
grep -rn nested_memory_attachment_triggers coco-rs --include='*.rs'
```

returns 8 hits: 1 trait field, 1 clone in `context.rs`, 2 in `tool_context.rs`
init, 1 populator + 2 tests in `core/tools/`, **0 in `app/query/`**. The drain
side of the pipeline is genuinely absent.

---

## 3. Optimization Plan

Two execution orders are possible; I recommend **B**.

### Plan A — minimal "make it work"

Wire the drain only, accept TS feature parity gaps elsewhere. ~2 days.

1. Add `engine_attachments::drain_nested_memory_triggers(ctx, cwd) -> Vec<NestedMemoryInfo>`
   that pulls the trigger set, calls a stub `getNestedMemoryAttachmentsForFile`
   (using just the existing `discover_claude_md_files` flat output), and clears.
2. Call it inside `finalize_turn_post_tools` *before* building `TurnReminderInput`.
3. Populate `materialized.nested_memories` from the drained vec — bypass the
   no-op `MemoryAdapter` by setting it on the materialized struct directly.
4. Remove the no-op stub in `MemoryAdapter::nested_memories` (or document
   that it stays a no-op because triggers flow through the engine, not
   the adapter).

Trade-off: ships the trigger flow but ignores `@import`, conditional rules,
nested CLAUDE.md per-directory, and per-file traversal. Most of TS's value
add is in those phases — without them this is barely better than today.

### Plan B — TS-faithful port (recommended)

Rebuild the pipeline to TS parity. ~5–7 days. Phased so each phase ships value.

#### Phase 0 — memory-filename helper (0.25 day, lands first)

Tiny, isolated module that every later phase depends on. Worth landing
as its own commit so the dual-name + case-insensitive contract has a
single test surface.

**New file**: `core/context/src/memory_filenames.rs`
```rust
/// Memory-file basename candidates. Matched case-insensitively.
/// `CLAUDE.md` is the TS-original; `AGENTS.md` is the cross-ecosystem
/// convention shared with Codex / Cursor / similar agents.
pub const MEMORY_FILE_CANDIDATES: &[&str] = &["CLAUDE.md", "AGENTS.md"];

/// Local (gitignored) variants — same candidates with `.local` infix.
pub const MEMORY_LOCAL_FILE_CANDIDATES: &[&str] = &[
    "CLAUDE.local.md",
    "AGENTS.local.md",
];

/// Case-insensitively find any of `candidates` directly under `dir`.
/// Returns disk-cased absolute paths in stable alphabetical order.
/// Empty result on read errors (ENOENT/EACCES/ENOTDIR).
pub fn find_memory_files(dir: &Path, candidates: &[&str]) -> Vec<PathBuf>;
```

Tests:
- `Claude.md`, `agents.md`, `AGENTS.MD` all match.
- Both `CLAUDE.md` AND `AGENTS.md` in same dir → both returned.
- Directory entries (not files) ignored.
- Missing dir → empty vec, no error.
- Stable ordering across two calls on the same dir.

**Note**: every `dir.join("CLAUDE.md")` call site in the upcoming phases
goes through this helper instead. `discover_claude_md_files` becomes a
thin wrapper that delegates per-directory matching to it.

#### Phase 1 — drain plumbing (1 day)

Goal: make the existing scaffolding fire end-to-end with a placeholder traversal.

1. **New file**: `app/query/src/engine_attachments.rs`
   ```rust
   pub(crate) async fn drain_nested_memory_attachments(
       ctx: &coco_tool_runtime::ToolUseContext,
       cwd: &Path,
   ) -> Vec<NestedMemoryInfo>;
   ```
   - acquire write lock on `ctx.nested_memory_attachment_triggers`
   - drain into `Vec<PathBuf>`
   - call `coco_context::nested_memory::traverse_for_file(path, cwd, &mut loaded_set)` for each
   - merge results into a `Vec<NestedMemoryInfo>`
   - the caller (engine) writes the resulting paths into a session-scoped
     `loaded_nested_memory_paths` Set living on `QueryEngine` (mirror TS
     `loadedNestedMemoryPathsRef`)
2. **Call site**: `engine_finalize_turn.rs::finalize_turn_post_tools` — invoke
   the drain *before* the next iteration's prompt build. Stash result on
   the engine in a `pending_nested_memory: Vec<NestedMemoryInfo>` field.
3. **Wire into reminder input**: in `engine_turn_reminders.rs`, instead of
   `materialized.nested_memories`, take from `engine.take_pending_nested_memory()`
   so the trigger-driven flow bypasses the no-op adapter. Leave
   `MemoryAdapter::nested_memories` as no-op with updated comment.
4. **Test**: extend `core/tools/src/tools/read.test.rs` with an
   end-to-end fixture asserting that reading a file in a subdirectory
   yields the parent's CLAUDE.md in the next turn's reminder.

This phase already gives users **80% of the value** with a stub traversal
that just walks CWD→file directories looking for `CLAUDE.md` and
`.claude/CLAUDE.md` (no rules, no @import).

#### Phase 2 — per-file traversal (1.5 days)

New module `core/context/src/nested_memory.rs` (or grow `claudemd.rs` past
its current 98 lines):

```rust
pub struct TraversalContext<'a> {
    pub cwd: &'a Path,
    pub processed: &'a mut HashSet<PathBuf>,
}

/// Returns (nested_dirs, cwd_level_dirs).
/// - nested_dirs: dirs strictly between CWD (exclusive) and file's parent
///   (inclusive), filtered to startsWith(cwd). Order: CWD-side → file-side.
/// - cwd_level_dirs: dirs from filesystem root → CWD inclusive.
///   Order: root → CWD.
pub fn directories_to_process(file: &Path, cwd: &Path)
    -> (Vec<PathBuf> /*nested*/, Vec<PathBuf> /*cwd_level*/);

pub async fn traverse_for_file(
    file: &Path,
    cwd: &Path,
    loaded: &mut HashSet<PathBuf>,
) -> Vec<NestedMemoryInfo>;
```

Mirror TS phases 1–4 from `attachments.ts:1792-1862`. Skip phase 1
(managed/user conditional rules) until phase 3 below — gate behind a
TODO comment so we can ship phase 2 standalone.

**Boundary contract** (from `getDirectoriesToProcess` `attachments.ts:1656-1689`):
- Loop `currentDir = dirname(file)`; while `currentDir != cwd && currentDir != filesystem_root`: if `currentDir.startsWith(cwd)`, push; else stop. Reverse.
- Loop `currentDir = cwd`; while `currentDir != filesystem_root`: push, then `currentDir = dirname`. Reverse.
- **CWD itself is in `cwd_level_dirs` (loaded at session start eagerly), NOT in `nested_dirs`** — this prevents the eager + lazy phases from double-loading `cwd/CLAUDE.md`. Phase 1 of the rewrite must keep this invariant by also fixing `discover_claude_md_files` (G13) so it stops loading immediate children.

Tests: parity with TS examples — `/proj/src/utils/helper.ts` with CWD
`/proj` should produce loads in `/proj/src`, `/proj/src/utils` order
(`/proj` deliberately excluded — that's the eager phase's job).
Use a `tempdir` fixture.

#### Phase 3 — `@import` directive (1.5 days)

In `core/context/src/claudemd_imports.rs`:

- Markdown text-node walker (use `pulldown-cmark` — already in workspace deps).
- Path validator + resolver (`./`, `~/`, `/`, bare).
- Allowlist `TEXT_FILE_EXTENSIONS` (port the TS Set verbatim — start with `.md, .txt, .json, .yaml, .toml, .ts, .tsx, .js, .py, .rs, .go, .rb, .java, .kt`).
- Recursive expansion with `MAX_INCLUDE_DEPTH = 5` + `processed: Set<PathBuf>`.
- Symlink target tracking.
- Emit included files **before** parent in result vec (so parent later overrides).

Wire into `traverse_for_file` so each loaded `CLAUDE.md` body gets expanded.

Tests: include a circular-include fixture (a → b → a) and a depth-6 chain
asserting depth 6 is dropped.

#### Phase 4 — conditional `.claude/rules/*.md` (1 day)

In `core/context/src/claude_rules.rs`:

- Read `.claude/rules/*.md`, parse YAML frontmatter via existing
  `coco-frontmatter` util.
- Extract `paths` field; run through a `split_paths_in_frontmatter`
  helper (port `frontmatterParser.ts:189-232` — handles comma split with
  brace nesting + brace expansion).
- Build a `globset::GlobSet` from the patterns (workspace already uses
  `globset` for ignore matching).
- During `traverse_for_file`:
  - **nested dirs**: load both unconditional (no `paths`) AND conditional
    rules matching the trigger file
  - **cwd-level dirs**: load only conditional rules matching the trigger file

Tests: a rule with `paths: src/auth/**/*.rs` should fire for
`/proj/src/auth/handler.rs` and not for `/proj/src/admin/page.tsx`.

#### Phase 5 — managed/user rules + dedup polish + eager-loader fix (1 day)

- Add `getManagedAndUserConditionalRules` equivalent (Phase 1 of TS
  traversal) — checks `/etc/coco/rules/*.md` and `~/.coco/rules/*.md`.
- Promote the engine-side `loaded_nested_memory_paths` `HashSet` to a
  proper `Arc<RwLock>` if subagents need to see it; otherwise keep
  per-engine.
- Wire `dynamic_skill_dir_triggers` drain at the same site as G1 so
  that pipeline also stops being dead (same shape, different consumer).
- **Rewrite `discover_claude_md_files` (G13/G14/G16/G20/G21)**:
  - Drop the immediate-children walk (lines 60-67) — that was
    compensating for the missing trigger pipeline and will now
    double-load.
  - Replace parents-up-to-depth-10 with TS-faithful root-to-CWD walk
    via `getMemoryFiles` semantics: load every memory file (via
    `find_memory_files(dir, MEMORY_FILE_CANDIDATES)` from Phase 0,
    covering both `CLAUDE.md` and `AGENTS.md` case-insensitively),
    `.claude/CLAUDE.md`, local files (via
    `find_memory_files(dir, MEMORY_LOCAL_FILE_CANDIDATES)`), and
    `.claude/rules/**/*.md` *unconditional* on the way down.
  - Add HTML-comment + frontmatter stripping via a new
    `safely_read_memory_file` helper that mirrors
    `safelyReadMemoryFileAsync`, returning `(content, raw_content,
    differs_from_disk, include_paths)`.
  - Add worktree skip: if CWD is inside a `.claude/worktrees/<name>/`
    and the parent dir lives in the canonical repo but outside the
    worktree, skip Project loads for that dir (local files still
    load — they're gitignored).
  - Rename `ClaudeMdFile` → `MemoryFile` and `ClaudeMdSource` →
    `MemoryFileSource`; keep `discover_claude_md_files` as a deprecated
    re-export for one release cycle so external embedders don't break,
    add new `discover_memory_files` as the primary API. (Per CLAUDE.md
    "no `#[deprecated]`" rule we instead **rename outright** and update
    the two known call sites — `app/cli/src/main.rs:497` and
    `app/query/src/engine_prompt.rs:49` — in the same commit.)
- **Fix `clone_for_subagent` (G19)**: reset
  `nested_memory_attachment_triggers` to a fresh `Arc::new(RwLock::new(HashSet::new()))`
  on subagent clone, mirroring TS `forkedAgent.ts:382`.

#### Phase 6 — tests + doc (0.5 day)

- `tests/nested_memory_e2e.rs` integration test: spin up a temp
  workspace with `CLAUDE.md` at three depths + a conditional rule, run
  one Read tool call, assert the next turn's reminder contains all
  three CLAUDE.md files plus the matched rule.
- Update `docs/coco-rs/crate-coco-context.md` and
  `docs/coco-rs/crate-coco-memory.md` with the new module list.
- Update `core/context/CLAUDE.md` and `core/system-reminder/CLAUDE.md`
  with the new data flow diagram (engine drains triggers → traversal →
  `pending_nested_memory` → reminder input).

---

## 4. Suggested module layout (after phase 6)

```
core/context/src/
├── memory_filenames.rs           (NEW — Phase 0: candidate basenames + case-insensitive lookup)
├── claudemd.rs                   (refactored — discover_memory_files; uses memory_filenames)
├── nested_memory/
│   ├── mod.rs                    (public surface: traverse_for_file, NestedMemoryInfo)
│   ├── directories.rs            (directories_to_process)
│   ├── imports.rs                (@import parser + recursion guard)
│   └── rules.rs                  (frontmatter parsing + glob matching)
└── ...
app/query/src/
├── engine_attachments.rs         (NEW — drain helpers)
├── engine_finalize_turn.rs       (call drain before next-turn build)
├── engine_turn_reminders.rs      (use pending_nested_memory)
└── reminder_adapters.rs          (MemoryAdapter::nested_memories stays no-op + comment)
```

`coco-memory` is **not** the right home for nested traversal — it owns
recall + storage + extraction, not filesystem CLAUDE.md walking. The
trait comment in `reminder_adapters.rs` got the layering right
("happens upstream in coco-context"); the bug is that "upstream"
doesn't actually do it yet.

---

## 5. Risks & open questions

1. **Performance**: TS's per-turn traversal walks the filesystem for every
   read file. On a session that reads 50 files, that's 50 × (CWD → file)
   stat calls. Worth caching by directory? TS doesn't — its memoize
   cache is keyed on the global `getMemoryFiles` call, not per-file
   traversal. Suggest: ship without caching, profile, add per-directory
   `(dir, mtime) -> Vec<MemoryFileInfo>` LRU only if hot.

2. **Subagents**: TS gives each subagent a fresh `nestedMemoryAttachmentTriggers`
   Set (`forkedAgent.ts:382`). Rust currently clones the `Arc<RwLock>` in
   `ToolUseContext::clone_for_subagent` (line 474) — meaning subagents
   *share* the trigger set with the parent. **This is a divergence from
   TS** — subagents' file reads will leak triggers back to the parent's
   next-turn drain. Should be `Arc::new(RwLock::new(HashSet::new()))` for
   subagents; fix this when wiring the drain.

3. **`mentioned_paths` semantic overlap**: TS uses two separate channels
   (@-mentions in user prompts → immediate inline injection;
   file-read triggers → next-turn nested memory). Rust currently
   conflates them under `MaterializeContext.mentioned_paths`. After
   phase 1, that field becomes purely @-mentions and the new pipeline
   handles triggers — clear separation, no semantic drift.

4. **Hook integration**: TS fires `InstructionsLoaded` with
   `loadReason ∈ {nested_traversal, path_glob_match, include}`. The
   coco-rs hooks crate doesn't currently emit this event. Defer until
   phase 7 (out of scope for this plan).

5. **`always_loaded` rules (no `paths` frontmatter)**: TS treats these
   as "load eagerly in every directory, once per session". Rust's
   eager `discover_claude_md_files` doesn't load them today. Phase 4
   should pull these into the eager set (load once at prompt build via
   `discover_claude_md_files`), and the conditional path picks up the
   rest.

---

## 6. Verification checklist (post-implementation)

- [ ] `grep -rn nested_memory_attachment_triggers coco-rs --include='*.rs'`
  shows references in `app/query/src/engine_attachments.rs` (drain) and
  `app/query/src/engine_finalize_turn.rs` (call site).
- [ ] `grep -rn loaded_nested_memory_paths coco-rs --include='*.rs'` shows
  read+write call sites in the drain helper.
- [ ] Integration test: temp workspace with 3-deep CLAUDE.md + 1 conditional
  rule passes.
- [ ] `just test-crate coco-context` and `just test-crate coco-query` green.
- [ ] Read a file in a subdirectory in a real session; the next-turn
  prompt sent to the model contains `<system-reminder>Contents of
  /tmp/.../sub/CLAUDE.md:\n\n...</system-reminder>`.
- [ ] Subagent reading a file does **not** pollute the parent's trigger set
  (verified via `clone_for_subagent` test).
- [ ] Fixture with both `CLAUDE.md` and `AGENTS.md` in the same directory
  loads BOTH (separate entries with distinct paths).
- [ ] Fixture with `Claude.md` (mixed case) loads correctly.
- [ ] Fixture with `agents.md` (lowercase) loads correctly.
- [ ] No double-load when only `AGENTS.md` exists at CWD (eager) and
  the user reads a file in a subdir (lazy) — the eager load owns CWD,
  the lazy load owns descendants of CWD.

---

## 7. References

| Topic | TS file:line | Rust file:line |
|---|---|---|
| Trigger init | `QueryEngine.ts:370` | `app/query/src/tool_context.rs:233` ✅ |
| Trigger populator | `FileReadTool.ts:1038` | `core/tools/src/lib.rs:364` ✅ |
| Drain | `attachments.ts:2167` | `app/query/src/engine_attachments.rs` ✅ |
| Per-file traversal | `attachments.ts:1792-1862` | `core/context/src/nested_memory.rs` ✅ |
| Directory split | `attachments.ts:1656-1689` | `core/context/src/nested_memory.rs` (`directories_to_process`) ✅ |
| Dedup | `attachments.ts:1710-1775` | `engine.loaded_nested_memory_paths` + `expand_imports` `processed` set ✅ |
| `@import` parser | `claudemd.ts:451-685` | `core/context/src/claudemd_imports.rs` ✅ |
| Frontmatter `paths` | `frontmatterParser.ts:189-232` | `core/context/src/claude_rules.rs` (`parse_paths_field`) ✅ |
| Render template | `messages.ts:3700-3707` | `core/system-reminder/src/generators/memory.rs:74-122` ✅ |
| Source trait | `Tool.ts:215` (field on context) | `core/system-reminder/src/sources/traits.rs:139-148` ✅ |

---

## 8. Remaining deferrals

These TS behaviors were intentionally **not** ported in this pass.
Track each as its own gap if/when needed.

| Behavior | Why deferred | Effect on parity |
|---|---|---|
| `claudeMdExcludes` settings filter (`Settings → glob list of paths to skip`) | Requires settings plumb-through into `discover_memory_files`; low priority for an opinionated single-tenant setup. | Users can't blocklist a noisy `CLAUDE.md`. Workaround: rename the file. |
| `MAX_MEMORY_CHARACTER_COUNT = 40000` per-file truncation | TS truncates+warns on oversize files. Rust passes content through verbatim. | An oversize file consumes more system-prompt tokens than TS would have allowed. |
| `MAX_MEMORY_LINES = 200` MEMORY.md cap | MEMORY.md handling lives in `coco-memory`, not `coco-context`. | Caller-side concern. |
| HTML-comment stripping (`<!-- ... -->`) | TS strips block-level HTML comments via the `marked` lexer; we'd need a markdown parser dep just for this. | A `<!-- model-only note -->` survives into the prompt. Generally harmless. |
| `--add-dir` (`CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD`) | CLI flag plumb-through plus settings; covered by P3 in the gap audit. | Power-user feature; multi-repo workflows are unaffected if the user just runs from a parent dir. |
| AutoMem (`MEMORY.md` entrypoint) | Owned by `coco-memory` crate by design — `coco-context` only discovers files. | No change required here. |
| TeamMem (`<team-memory-content>` wrapper) | Requires team-sync infrastructure (v2 milestone). | Not blocking for v1. |
