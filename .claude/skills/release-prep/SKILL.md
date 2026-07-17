---
allowed-tools: Read, Edit, Write, Glob, Grep, Bash(git:*), Bash(just:*), Bash(cargo:*), Bash(python3 scripts/bench-startup.py*), Bash(ls *), Bash(rg:*)
description: Prepare all user-facing docs for a release — regenerate the source-derived tables, write the CHANGELOG entry from tag→HEAD, re-measure the published performance numbers, and check the EN/ZH READMEs for drift. Use this whenever a release, version bump, tag, or CHANGELOG comes up, whenever someone asks "what changed since the last version", and before publishing the npm package — even if they only mention one of those pieces, because the four rot together.
argument-hint: [base-tag] (default: most recent tag)
---

## Context

- Base tag: $ARGUMENTS (default: latest `coco-v*` tag)
- Release tags: !`git tag -l 'coco-v*' --sort=-v:refname | head -3`
- Current version: !`rg -m1 '^version' coco-rs/Cargo.toml`
- npm version: !`rg -m1 '"version"' coco-cli/package.json`
- Commits since latest release tag: !`git rev-list --count "$(git tag -l 'coco-v*' --sort=-v:refname | head -1)"..HEAD 2>/dev/null || echo "?"`

Match release tags as `coco-v*` and sort by `-v:refname`, not by date. The repo also
carries non-version tags (e.g. `backup/…`), and `git describe --tags` will happily
pick one of those and report zero commits since "the last release".

## What this does

Four things rot between releases, and they rot independently:

1. **Reference tables** in `docs/` — already machine-enforced, just regenerate.
2. **CHANGELOG** — needs judgment about what users can actually see.
3. **Performance numbers** in both READMEs — measured, so they go stale as the code changes.
4. **EN/ZH README drift** — two files, one of them always forgotten.

Work through them in order. Report at the end; let the user decide the version number.

## Step 1 — Regenerate the derived tables

```bash
cd coco-rs && just docs-gen
```

This rewrites the `<!-- BEGIN GENERATED: ... -->` blocks (provider catalog, feature
gates, model roles, CLI flags, CLI subcommands) from source. `just check-docs`
already gates this in CI, so it is usually a no-op — but run it first so the
CHANGELOG you write next describes tables that are already correct.

If it errors, it is telling you the code gained something undocumented (a new
provider, a new flag). Fix the generator's entry, don't hand-edit the table.

## Step 2 — Write the CHANGELOG entry

```bash
git log --format='%ad %s' --date=short <base-tag>..HEAD
```

**The commit log is raw material, not the answer.** Turning it into a changelog is
the judgment this step exists for.

### Translate to user-visible behavior

A changelog entry answers "what can I do now that I couldn't before?" A commit
subject usually answers "what did I move?" Those are different.

- `refactor(multisession): type wire discriminators and harden replace-commit`
  → users see nothing. **Omit it.**
- `feat(auth): add Grok subscription login`
  → "Added Grok subscription login (`coco login grok`), which uses a device-code
  flow so it works over SSH with no browser." The flow detail is the part a user
  cares about, and it is not in the subject line — you had to read the code.

Skip pure refactors, test-only changes, and internal renames unless they change
something observable.

### Verify the thing actually works before announcing it

This codebase has shipped `println!` stubs that read like features. `coco mcp list`
prints `"MCP servers: (none connected)"` unconditionally; `coco config set` prints
`"Would set …"` and writes nothing; `coco status` prints a hardcoded `v0.0.0`. A
changelog that announces those is worse than no changelog — it sends people to a
command that lies to them.

For every entry you are about to write, find the implementation and confirm it does
the thing. If it is gated, say so honestly ("behind the `voice` feature, off by
default") rather than implying it is on.

### Group and format

Follow the existing `CHANGELOG.md`: Keep a Changelog, newest first, with
`### Added` / `### Changed` / `### Fixed` / `### Security` under each version.
Prose bullets, wrapped at ~80 columns, no per-file recaps.

Put anything that changes a trust boundary under **Security**, and describe the
exposure plainly — what someone could have done, and what stops it now.

### The version number is the user's call

Do not invent a version. Default to `## Unreleased` and say so. Cutting a version
means tagging and publishing to npm; that is a release decision, not a docs
decision. If the user wants a version cut, bump `coco-rs/Cargo.toml` and
`coco-cli/package.json` together — they must match.

## Step 3 — Re-measure the performance numbers

The READMEs publish measured numbers inside `<!-- BEGIN MEASURED -->` blocks. They
are only worth publishing if they are true of the current commit.

```bash
cd coco-rs && cargo build --release -p coco-cli --bin coco --features jemalloc
cd .. && python3 scripts/bench-startup.py ./coco-rs/target/release/coco 6
```

Three things will silently corrupt this measurement:

- **Wrong build flags.** Measure what ships: `--release --features jemalloc`, the
  same flags `.github/workflows/coco-release.yml` uses. A debug binary is a
  different program.
- **A polluted config.** The developer's own `~/.cocode/settings.json` may enable
  debug logging and TUI perf instrumentation, which measurably slows startup
  (~575 ms when this was last checked). Measure with a clean config dir:
  `COCO_CONFIG_DIR=$(mktemp -d)` holding only `{"models":{"main":"..."}}`.
- **A cold binary.** The first launch after a link pages in ~78 MB. Discard it;
  the harness takes a median of N runs for this reason.

Report both physical footprint and RSS — they differ by ~1.6× and quoting only the
flattering one is a lie of omission. Update the commit hash and machine in the
block's caption; a number without its conditions is not reproducible.

**Never source a performance number from `docs/internal/jcode/*`.** Every figure in
those files belongs to jcode, a different project, measured on a different machine.
They are there for an architecture comparison and have burned people before.

Only publish a number you would defend. If startup time is unflattering, leaving it
out is honest; claiming it is fast is not.

## Step 4 — EN/ZH README drift

`README.md` is the source; `README.zh-CN.md` follows it. Check that every section,
table row, and measured number matches. The Chinese README is a translation, not a
fork — if the English gained a provider row or a new feature bullet, port it.

Docs under `docs/` are English-only by policy. Do not translate them.

## Step 5 — Report

Tell the user:

- what the generated tables changed (if anything)
- the CHANGELOG entries you wrote, and **anything you deliberately omitted** as a
  stub or as invisible-to-users
- the new measured numbers vs the old ones, with the conditions
- any EN/ZH drift you fixed
- that the version number is theirs to decide

Then stop. Do not tag, do not publish, do not commit unless asked.

## When something disagrees with the code

You will find docs that contradict the source. That is a different job — see the
`docs-verify` skill, which knows how to tell "the doc is stale" apart from "the
code has a bug". Do not silently rewrite prose to match code you have not
understood.
