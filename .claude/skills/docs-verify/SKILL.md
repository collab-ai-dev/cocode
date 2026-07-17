---
allowed-tools: Read, Edit, Glob, Grep, Bash(git:*), Bash(just:*), Bash(rg:*), Bash(ls *)
description: Interrogate the docs against the source — pull every falsifiable claim out of docs/ and the READMEs, verify each one in the code, auto-fix what is merely stale, and report what turns out to be a real code bug. Use this before a release, after changing config/CLI/providers/features/permissions, whenever someone asks whether the docs are still accurate or wants a doc reviewed, and whenever you are about to trust a doc's description of how something works — this repo's docs have shipped a removed flag, wrong config paths, and a security claim the code never enforced.
argument-hint: [doc-path | crate-path] (default: docs pages touching the current diff)
---

## Context

- Scope hint: $ARGUMENTS
- Changed files: !`git diff --name-only HEAD | head -20`
- Doc pages: !`ls docs/*.md`

## Why this exists

Reference tables in `docs/` are generated and gated (`just check-docs`), so they
cannot drift. **Prose has no such protection**, and prose is where the dangerous
errors live. Real examples found in this repo, all in prose:

| The doc said | The code said |
|---|---|
| `just coco --no-tui -p "..."` | `--no-tui` is removed, with a regression test asserting it is *rejected* |
| `~/.coco/settings.json`, `.claude/` | `~/.cocode/`, `.cocode/` |
| `ModelAlias` is a key type | no such type exists |
| MoA and the workflow runtime are missing/stubs | both implemented and shipping |
| `coco mcp list` manages servers | prints `"(none connected)"` unconditionally |
| ~28 MB idle RAM | that is **jcode's** number, a different project |
| repo is `coco-collab-dev/cocode` | git remote and `feedback.rs` say `collab-ai-dev/cocode` |
| project settings cannot set bypass mode | they could — it was a live security hole |

Note the last row. A doc that describes a safety property the code does not have is
not a documentation bug; it is a vulnerability with a false alibi.

## Method

### Step 1 — Clear the machine-checkable part first

```bash
cd coco-rs && just check-docs
```

If that fails, the generated tables are stale — run `just docs-gen` and stop
worrying about them. Everything below is about prose.

### Step 2 — Scope

Full sweeps are expensive and mostly re-verify things that did not change. Prefer
targeting:

- an explicit path in `$ARGUMENTS`, or
- the doc pages covering whatever the current diff touches. Rough map:
  `common/config` → configuration.md, providers-and-auth.md;
  `app/cli` → cli-reference.md; `common/types/src/features.rs` → configuration.md;
  `core/permissions`, `exec/sandbox` → permissions.md, sandbox.md;
  `core/tools` → tools.md; `commands/` → slash-commands.md;
  `services/provider-auth` → providers-and-auth.md; `core/subagent`, `coordinator/`
  → subagents-and-teams.md.

### Step 3 — Extract falsifiable claims

Read the page and pull out every statement that the code can refute. These are the
high-yield categories, in rough order of how often they rot:

- **Flags and subcommands** — does it exist in the clap schema? Is it in the
  removed-flag regression test (`app/cli/src/lib.test.rs`)?
- **Paths** — `~/.cocode/` vs `~/.coco/`, `.cocode/` vs `.claude/`. Check
  `utils/common/src/coco_home.rs` and `common/config/src/global_config.rs`.
- **Env vars** — does the `EnvKey` variant or the `COCO_FEATURE_*` scan honor it?
- **Feature defaults** — `common/types/src/features.rs` `FEATURES` is the only
  source of truth. Not a doc, not a comment, not a crate `CLAUDE.md`.
- **Tool and command names** — registered in `core/tools/src/lib.rs` /
  `commands/src/implementations.rs`? An `is_enabled()` returning hard `false` means
  unreachable, not "available".
- **Does it actually work** — stubs read exactly like features. Follow the
  implementation to something that does real work.
- **Numbers** — counts ("41 tools", "nine roles"), sizes, benchmarks. Recount.
  Any perf figure must trace to `scripts/bench-startup.py` on this project, never
  to `docs/internal/jcode/*`.
- **Type and API names** — grep for them. `ModelAlias` was documented for months.
- **URLs** — check `git remote -v` and the code that hardcodes them.

Verify by reading the code. A crate `CLAUDE.md` is not evidence — several are
stale, which is exactly the failure being hunted.

### Step 4 — Classify each mismatch before touching anything

This is the whole skill. "Doc says A, code does B" has **three** causes, and they
need opposite responses:

**(1) The doc is stale → fix the doc.**
The code is correct and intentional; the doc describes an older world. Renamed
paths, removed flags, changed defaults. Safe to edit directly.

**(2) The code is wrong → report, do not "fix" the doc.**
The doc describes the intent correctly and the code has a bug. Editing the doc to
match buggy code launders the bug into a spec. Seen here: clap's own help says
`--system-prompt` is "appended to default" while the implementation *replaces* it;
`coco init --help` says `.claude/` while the code creates `.cocode/`; the npm
`package.json` repository URL points at an org that does not exist.

**(3) The doc claims a safety property the code does not enforce → report loudly, never edit.**
The most dangerous class, and the easiest to get wrong. Rewriting *"project settings
cannot enable bypass mode"* into *"project settings can enable bypass mode (be
careful)"* would have documented a vulnerability as a feature and closed the case.
The right move was to report it, and the fix was in the code.

Ask: **if the doc is right and the code is wrong, who gets hurt?** If the answer
involves credentials, permissions, sandboxing, or arbitrary execution, it is class
3 — stop and surface it.

When you cannot tell (1) from (2), it is (2). Report and let a human decide.

### Step 5 — Apply

- Class 1: edit the prose. Keep the page's voice; change the claim, not the style.
- Class 2 and 3: change nothing. Collect them for the report with `file:line` for
  both sides.
- Never edit inside `<!-- BEGIN GENERATED: ... -->` — fix `coco-rs/xtask/` instead.
- Never "fix" a doc by deleting the inconvenient claim. If a documented feature does
  not exist, that is a finding.

### Step 6 — EN/ZH drift

`README.zh-CN.md` mirrors `README.md`. Compare section-by-section: a fix applied to
one and not the other recreates the problem in the other language. `docs/` is
English-only.

## Report

```markdown
## Fixed (docs were stale)
- docs/<page>.md:<line> — said X; code does Y (<file>:<line>). Updated.

## Found (code looks wrong — not touched)
- <file>:<line> — code does X; docs/<page>.md:<line> says Y, which is what a
  reader would reasonably expect. Suggest fixing the code.

## Security-relevant (needs a human)
- <file>:<line> — the docs claim <property>; the code does not enforce it.
  Exposure: <what someone could do>.

## Verified clean
- <page>: <n> claims checked.
```

Lead with the security-relevant items if there are any. Do not bury them under the
routine path renames.
