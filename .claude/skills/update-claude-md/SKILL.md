---
allowed-tools: Read, Glob, Grep, Write, Bash(ls *), Bash(wc *)
description: Regenerate CLAUDE.md from workspace discovery (zero hardcoded content)
argument-hint: [target-file-path]
---

## Context

- Target file: $ARGUMENTS (default: CLAUDE.md in project root)
- Active workspace: `coco-rs/` (primary development)
- Reference projects: `coco-rs/` and `codex-rs/` (read-only reference implementations)
- Workspace root: !`ls coco-rs/Cargo.toml`
- Existing CLAUDE.md line count: !`wc -l CLAUDE.md`

## Goal

Regenerate CLAUDE.md by **discovering** all project structure at runtime. This command contains **zero hardcoded crate names, layer names, or architecture content** — everything is derived from the workspace.

## Principles

- **Size budget**: target **< 30k chars** (CC memory warns at 40k). Root CLAUDE.md is loaded every session and consumes context for every turn; the smaller it is, the more room the agent has for real work.
- **Root file = rules & conventions.** Reference content (type lists, field enumerations, exhaustive module inventories) belongs in each crate's own `CLAUDE.md`. Root links to them; it does not copy them.
- **Concise but informative**: tables over prose. One-line purpose per crate is enough — the agent opens the crate's own `CLAUDE.md` for detail.
- **No fragile counts.** Do not write `"(5)"`, `"26 crates"`, `"42 tool impls"`, `"18 contexts, 73+ actions"`, `"8 roles"`. These drift every time someone adds a file and carry no insight. Just name the thing (`ModelRoles`, `Utils`, `tools`); anyone who cares can `ls` or grep.
- **Utils is the exception — it is a capability catalog.** For every utils crate, keep a one-line description in a table so the agent can scan for existing capabilities (path handling, caching, git, encoding, fuzzy search, frontmatter, secret redaction, …) before rolling its own. A bare list of names defeats the purpose.
- **Progressive disclosure**: main file gives the overview; links point at detailed docs.
- **Single source of truth**: reference `AGENTS.md` for conventions, don't duplicate.
- **No inline code examples**: link to source instead.

## Procedure

### Step 1: Discover Workspace

1. **Parse crate list**: Read `coco-rs/Cargo.toml`, extract all paths from `[workspace] members` (resolve globs by listing matching directories)

2. **Gather crate metadata** — one-line purpose per crate:
   - First: read the crate's `Cargo.toml` for `name` and `description` fields
   - Fallback: read `src/lib.rs` (or `src/main.rs`) first 10 lines for Rust crate-level doc comments
   - Last resort: infer purpose from the crate name itself
   - **Do not extract `Key Types` for root inclusion.** Public types are already (or should be) documented in each crate's own `CLAUDE.md`. Duplicating them in root bloated past regenerations from ~18k chars to ~42k and provided no information the per-crate doc didn't already have. If a crate lacks its own `CLAUDE.md`, flag it as a gap — don't fix it by copying types into root.

3. **Auto-derive layers**: Group crates by their first path component (e.g. `common/`, `core/`, `app/`, `provider-sdks/`, `utils/`, `exec/`, `features/`, `mcp/`). Crates at the top level of `coco-rs/` (not in a subdirectory group) form a "Standalone" layer. Derive layer display names from the path prefix (e.g. `provider-sdks` → "Provider SDKs"). Sort layers in dependency order: Common → Provider SDKs → Core → App, with others (Features, Exec, MCP, Standalone, Utils) in between as appropriate.

4. **Find specialized docs**: Glob for `coco-rs/**/CLAUDE.md` — these get linked in a "Specialized Documentation" table.

5. **Read dev commands**: Read `coco-rs/justfile` to extract the key development commands (fmt, pre-commit, test, help, etc.).

6. **Scan error patterns**: For each layer, check whether crates use `snafu` or `anyhow` by grepping their `Cargo.toml` dependencies. Summarize as a table.

7. **Preserve human-authored sections**: Read the existing CLAUDE.md. Identify sections that contain human-authored design decisions, references, or notes that are NOT auto-generated crate tables. Preserve their content in the regenerated file.

8. **Discover data flows — keep to the 3 most important, tight ASCII**:
   - Agent loop driver — user input → context → streaming → tool execution → hooks → events → loop
   - Configuration resolution — JSON files + env + CLI overrides → resolved config → consumers
   - Provider call chain — high-level API → `LanguageModelV4` impl → HTTP → typed stream → result

   Other flows (shell execution, MCP integration, background tasks, …) should **not** be inlined — add a trailing sentence like `"For shell execution, MCP integration, background tasks: see the respective crate's CLAUDE.md"`. Six detailed diagrams in root was a major source of past bloat. Keep each diagram terse (≤ 10 lines) and discovered from code, not hardcoded.

9. **Discover design patterns**: Scan the workspace for cross-cutting design patterns by grepping for characteristic signatures:
   - `Arc<Mutex` / `Arc<RwLock` → shared state pattern (note which crates use it heavily)
   - `CancellationToken` → cancellation pattern
   - `mpsc::Sender` / `watch::Sender` → event-driven pattern
   - Builder structs (types ending in `Builder`) → builder pattern
   - `#[async_trait]` + `pub trait` → trait abstraction pattern
   - `is_meta` / meta messages → meta message pattern
   - Callback function types (`Fn`, `Box<dyn Fn`) → callback decoupling pattern
   - Facade types → facade pattern

   Summarize as a "Key Design Patterns" table with columns: Pattern | Where | Details. Only include patterns that are actually found in the codebase.

### Step 2: Generate Architecture Diagram

Build an ASCII art layer diagram **from the discovered layers and crate names**. Do NOT use a static template. The diagram should:
- Show layers as horizontal bands, ordered by dependency (bottom = foundational)
- List crate names within each layer
- Show key dependency arrows where obvious (e.g. loop → executor → api)
- For layers with many crates (e.g. Utils), show a count instead of listing all names

### Step 3: Compose CLAUDE.md

Assemble the file in this section order:

1. **Title + one-liner** — e.g. "Multi-provider LLM SDK and CLI. All development in `coco-rs/`."
2. **AGENTS.md reference** — "Read `AGENTS.md` for Rust conventions."
3. **Commands** — from justfile discovery (Step 1.5)
4. **Architecture** — generated diagram (Step 2)
5. **Key Data Flows** — ASCII flow diagrams for agent turn lifecycle, configuration resolution, provider call chain, and shell execution flow (Step 1.8)
6. **Crate Guide** — one table per layer. **All layers use `Crate | Purpose` only** — no `Key Types` column in root (types live in per-crate CLAUDE.md). **Do NOT put a count in the heading** (`### Utils`, not `### Utils (26)`). The Utils table is mandatory full-width (one-line description per crate) because it serves as the agent's capability catalog — "check here first before implementing any basic utility" — a bare name list defeats that purpose.
7. **Key Design Patterns** — table from Step 1.9 with columns: Pattern | Where | Details
8. **Error Handling** — from error pattern scan (Step 1.6)
9. **Specialized Documentation** — table linking all discovered CLAUDE.md files (Step 1.4)
10. **Preserved sections** — Design Decisions, References, or any other human-authored sections from Step 1.7
11. **References** — links to AGENTS.md, error docs, user docs

### Step 4: Write and Verify

Write the file, then verify:

1. **Size budget**: `wc -c CLAUDE.md` must report **< 30000** (hard target) and absolutely **< 40000** (CC warning threshold). If over, re-trim (data flows, preserved sections, over-long purposes) before proceeding.
2. **Crate coverage**: count of crates in generated file == count of workspace members (exact match). List any missing crates as errors.
3. **Link check**: every file path referenced in the CLAUDE.md actually exists on disk.
4. **CLAUDE.md coverage**: every discovered `**/CLAUDE.md` file is reachable from root (grouped per-layer link is fine; one link per file not required).
5. **Data flow coverage**: exactly **3** inlined data flow diagrams (agent turn, config resolution, provider call chain). Any others must be replaced with a one-line pointer to the owning crate's CLAUDE.md.
6. **No fragile counts**: grep the generated file for `\(\d+\)` in headings, and for patterns like `\d+ (crates|impls|roles|modules|contexts|actions|tools)`. Any hit is a regression — replace with a plain name or remove.
7. **No Key Types column**: every crate table must have only `Crate | Purpose`. A `Key Types` column in root is a regression.
8. **Utils table has per-crate descriptions**: not a bare comma-separated name list. Utils is the agent's capability catalog and earns its full table.

If any check fails, fix the issue and re-verify before proceeding.

### Step 5: Report

Output a summary:

```
## CLAUDE.md Regeneration Complete

| Metric | Before | After | Budget |
|--------|--------|-------|--------|
| Char count (`wc -c`) | X | Y | < 30000 |
| Line count | X | Y | — |
| Crates documented | X/N | Y/N | = N |

### Verification
- [ ] Char count < 30k (hard target), < 40k (CC warning)
- [ ] All workspace crates documented (exact match)
- [ ] All file links valid
- [ ] Every `**/CLAUDE.md` reachable from root
- [ ] Exactly 3 inlined data flows
- [ ] No fragile counts (no `(N)` headings, no `"N crates/impls/roles/…"`)
- [ ] No `Key Types` column in any crate table
- [ ] Utils table has per-crate one-line descriptions
```

## Important Rules

- Do NOT hardcode any crate names, layer names, or architecture content in this command
- Do NOT duplicate content from AGENTS.md
- Do NOT include inline code examples (link to source files)
- Do NOT use prose for crate descriptions (use tables; one-line purpose per crate)
- Do NOT skip any workspace crate (complete coverage is mandatory)
- Do NOT add a `Key Types` column to any crate table — those types belong in per-crate `CLAUDE.md`
- Do NOT write counts (`(5)`, `26 crates`, `42 tool impls`, `8 roles`) — they drift and carry no insight
- Do NOT inline more than 3 data flow diagrams — every extra one becomes dead weight within a year
- DO enforce a size budget: target < 30k chars, hard stop at 40k (CC memory warning threshold)
- DO make Utils a full-width table with per-crate descriptions — it is the agent's capability catalog
- DO preserve human-authored sections (Design Decisions, etc.) from the existing file
- DO add a one-line navigation hint at the top: *"Each crate has its own `CLAUDE.md` — read it when working in that crate. This root file covers conventions and high-level structure only."*

## Why these rules exist (prior-run lessons)

A past regeneration hit **42k chars** (over the 40k CC warning) because it inlined Key Types for every crate, 6 data flow diagrams, and per-file specialized-doc links. An optimization pass brought it to **~20k** with zero information loss by:

1. Dropping the Key Types column (those types live in per-crate `CLAUDE.md` already)
2. Trimming 6 data flows → 3 (others became one-line pointers)
3. Collapsing 60+ specialized-doc links into per-layer groupings
4. Deleting all `(N)` counts and `"N crates/impls/roles/…"` numbers
5. Keeping the Utils table *full* because the agent uses it as a capability catalog — a bare name list defeats the purpose

The generator that produced the bloated version followed its own instructions faithfully. The root cause was in the instructions, not the execution — hence these hard rules.
