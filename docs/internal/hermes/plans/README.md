# Hermes Absorption — Development Plans

Derived from [../hermes-opt.md](../hermes-opt.md) (the 21-release sweep and
verified gap analysis). One plan file per work item / PR batch. All plans
are **not started** unless the status line says otherwise.

## Citation conventions (cross-machine portable — no absolute paths)

- **Hermes evidence** cites repo-relative paths inside
  `NousResearch/hermes-agent` (github.com/NousResearch/hermes-agent),
  pinned to commit **`a7f65e3bc`** (`v2026.7.7.2` + 161 commits). Line
  numbers refer to that commit; symbols/constants are the stable anchor
  if lines drift. Release/PR numbers (`vYYYY.M.D`, `#NNNNN`) refer to the
  GitHub releases/PRs of that repo.
- **coco-rs paths** are workspace-relative (run from `coco-rs/`), e.g.
  `services/compact/src/prompt.rs`. Line numbers were verified on branch
  `feat/mulitsession` (2026-07-10) and will drift — re-verify before
  coding.

## Plan index

| Plan | Item(s) | Size | Depends on |
|------|---------|------|-----------|
| [p0-1-compact-prompt.md](p0-1-compact-prompt.md) | Summary language preservation + temporal anchoring | XS | — |
| [p0-2-loop-robustness.md](p0-2-loop-robustness.md) | Empty-response nudge · Edit closest-match hint · Bash ANSI strip | S | — |
| [p0-3-tool-loop-guardrails.md](p0-3-tool-loop-guardrails.md) | Warning-first repeated-call guardrails | M | — |
| [p1-1-mcp-transport.md](p1-1-mcp-transport.md) | MCP `tools/list_changed` refresh + keepalive ping | M | — |
| [p1-2-toolsearch-threshold.md](p1-2-toolsearch-threshold.md) | ToolSearch deferral threshold gate | S | — |
| [p1-3-grep-densify.md](p1-3-grep-densify.md) | Grep ≥5-match path-grouped densification | S | — |
| [p1-4-zero-llm-cron.md](p1-4-zero-llm-cron.md) | Script-only (zero-LLM) scheduled jobs | M | — |
| [p1-5-session-search.md](p1-5-session-search.md) | Deterministic session full-text search | M | — |
| [p1-6-model-lifecycle.md](p1-6-model-lifecycle.md) | Model retirement metadata + reasoning stall-timeout floor | M | model-card work in `../../cc-catchup-roadmap-v2.md` |
| [p2-1-goal-loop.md](p2-1-goal-loop.md) | `/goal` judged loop + verify-on-stop (design-level) | XL | p0-2 (empty-response seam), separate design review before code |

Covered elsewhere (no plan here): micro-compact recovery pointer →
[../../tool-result-offload-v2-design.md](../../tool-result-offload-v2-design.md) §6.

## Suggested sequencing

1. **PR 1** — p0-1 (prompt-only, two rules in `services/compact`).
2. **PR 2** — p0-2 (three independent small robustness fixes; can split).
3. **PR 3** — p0-3 (guardrails, adds config).
4. **P1 batch, value order** — p1-1 → p1-3 → p1-2 → p1-4 → p1-5 → p1-6.
5. **P2** — p2-1 needs its own adversarial design review before any code.

## Anti-lessons (constraints on ALL plans)

From hermes's own regressions — see hermes-opt.md §6 for release evidence:

1. Never extend `utils/secret-redact` into tool I/O (patch corruption;
   hermes `agent/redact.py` needed `code_file`/`file_read` opt-outs and a
   URL passthrough to recover — evidence in p1 plans is informational
   only).
2. Never LLM-back session search (p1-5 is deterministic by construction).
3. Any warning text injected into history needs freshness management
   (p0-3 injects only into the newest tool result, never retroactively).
4. Injected marker phrasing must survive provider content filters
   (hermes had to rename `[SYSTEM:` → `[IMPORTANT:`; p0-3/p2-1 marker
   strings follow suit).

## Repo conventions checklist (every plan below assumes these)

- Config: new keys land in the owning `*Config` consumed via
  `RuntimeConfig`; env vars go through `coco_config::EnvKey` with the
  `COCO_` prefix; no ad-hoc `std::env::var`.
- Enums over bools for behavior knobs; `i64` over `u64`; no `unwrap()`.
- Tests in companion `<file>.test.rs` via `#[path]`; snapshot tests for
  prompt/format changes.
- Byte-offset string cuts go through `coco_utils_string` /
  `floor_char_boundary` (several plans slice model text).
- `just quick-check` per iteration; `just pre-commit` once before commit.
