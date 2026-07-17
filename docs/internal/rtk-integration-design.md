# RTK Integration — Bash Output Compression via Rust Token Killer

Status: **phase 1 + phase 2 (v0) implemented** — subprocess tier (Appendix B) and
the embedded post-exec filter core (Appendix C) both landed; phase-2 v0 ships the
declarative TOML long-tail + never-worse guard, with the git/cargo/pytest family
formatters deferred until the fork's `cmds` module decouples from the binary
`Commands` enum. · Branch: `feat/rtk` · 2026-07-11

Upstream: [rtk-ai/rtk](https://github.com/rtk-ai/rtk) v0.42.4 (analyzed from a
local checkout). rtk ("Rust Token Killer") is a CLI proxy that executes a
dev-tool command, filters/compresses its output before it reaches the LLM
context, preserves the child exit code, and falls back to raw output when a
filter fails. Self-reported savings: **60–90 % of tokens** on git / test-runner
/ linter / container output.

**Design in two tiers, one `Feature::Rtk` gate.** Phase 1: when a healthy
`rtk` (or `rr-rtk`) binary is detected on `$PATH`, `BashTool` rewrites the
command string (`git status` → `rtk git status`) via the `rtk rewrite`
subprocess protocol — **after** permission evaluation, read-only
classification, and the sandbox decision have all run on the *original*
command, immediately before spawn. Phase 2 (the built-in tier, §3.3):
embed a lib-target fork of rtk's filter core and switch to **post-exec
in-process filtering** of captured Bash output — no rewrite, no local
install, works in sandboxes and background tasks. Built-in `Grep` / `Glob` /
`Read` never route through rtk at runtime; instead their formatters absorb
rtk's grouping ideas natively (§2.3–2.4).

---

## 1. What rtk is (implementation analysis)

### 1.1 Proxy model

```
Without rtk:   model ── git status ──▶ shell ──▶ git ──▶ ~2 000 raw tokens ──▶ model
With rtk:      model ── git status ──▶ rtk ──▶ git ──▶ filter ──▶ ~200 tokens ──▶ model
```

Lifecycle per command: parse (clap) → route to an ecosystem module → execute
the real tool via `std::process::Command` (piped, threaded capture; no PTY, no
async, no internal timeout) → filter → print → track savings in SQLite.

### 1.2 Coverage and filter strategies

~40 clap subcommands / 42 command modules across ecosystems: git/gh/glab/gt,
cargo, js (eslint/tsc/vitest/jest/playwright/prettier/pnpm/…), python
(ruff/pytest/mypy/pip/uv), go, ruby, php, dotnet, jvm, cloud
(aws/docker/kubectl/curl/…), system (ls/tree/read/find/grep/json/log/…).
Plus **63 declarative TOML filters** (terraform, helm, make, brew, jq, …)
compiled into the binary, overridable per-project (`.rtk/filters.toml`) and
per-user, with a SHA-256 trust store for custom filter files.

Twelve documented strategies; the ones that matter most for an agent loop:

| Strategy | Example | Reduction |
|---|---|---|
| Stats extraction | `git status` → "3 files, +142/−89" | 90–99 % |
| Failure focus | `cargo test` → failures only, passing hidden | 90–99 % |
| Grouping | eslint/tsc errors grouped by rule/file | 80–90 % |
| Deduplication | repeated log lines → `(×N)` | 70–85 % |
| Progress stripping | wget/pnpm ANSI bars removed | 85–95 % |
| Code filtering | `rtk read -l aggressive` strips bodies | 60–90 % |

### 1.3 Safety guarantees (why it is trustworthy as a proxy)

- **Exit-code preservation** — child exit codes (incl. 128+signal) propagate;
  CI-grade contract, covered by tests.
- **Fail-safe passthrough** — unknown command, filter failure, or parse error
  → raw output; command-not-found → exit 127.
- **Never-worse guard** (`core/guard.rs`) — if the filtered output would
  estimate to *more* tokens than raw, raw is emitted. Downside is bounded.
- **Tee recovery** — on overflow/failure the full raw output is saved to a
  temp file and the path is included in the compact output, so an agent can
  `Read` the raw log when the summary is insufficient.
- **Unattestable-construct guard** — commands containing command substitution
  (`$( )`, backticks), process substitution, heredocs, or file-target
  redirects are **never rewritten** (quote-aware lexer; `2>&1`, `>/dev/null`
  stay rewritable). This is rtk's CVE-bypass defense and is heavily tested.

### 1.4 The rewrite engine (the part coco-rs consumes)

`discover::registry::rewrite_command` is a stateless, sync classifier:

- Splits compound commands on `&&` `||` `;` `&`; each segment rewritten
  independently. For pipes, **only the left command** is rewritten; `find`/`fd`
  feeding a pipe is never rewritten (grouped output would break `xargs`).
- Strips env-var/`sudo`/wrapper prefixes (`command`, `exec`, user-configured
  `transparent_prefixes`), re-appends redirect suffixes.
- `RTK_DISABLED=1` prefix in a command opts that command out.
- ~85 regex rules; last (most specific) match wins; already-`rtk` commands and
  an ignore list (`cd`, `echo`, shell keywords) pass through.
- User config `~/.config/rtk/config.toml` `[hooks] exclude_commands`
  (literal or `^regex`) is honored inside the engine.

Two stable invocation surfaces:

1. **`rtk hook claude`** — reads a Claude-Code PreToolUse JSON payload on
   stdin, emits `hookSpecificOutput.updatedInput` JSON. Tied to Claude's hook
   wire format and to rtk reading *Claude's* settings files for its
   allow/ask/deny verdict.
2. **`rtk rewrite "<cmd>"`** — bare CLI contract (stable since 0.23.0):

   | Exit | stdout | Meaning |
   |---|---|---|
   | 0 | rewritten command | rewrite, host may auto-allow |
   | 1 | — | no rtk equivalent → passthrough |
   | 2 | — | a *host* deny rule matched → passthrough |
   | 3 | rewritten command | rewrite, host should still prompt |

   coco-rs runs its own permission engine **before** the rewrite (§4.2), so
   the verdict half of this protocol is irrelevant to us: **exit 0 or 3 with
   non-empty stdout ⇒ use the rewrite; anything else ⇒ passthrough.**

### 1.5 Analytics

rtk tracks every proxied command in SQLite (`~/.local/share/rtk/history.db`,
WAL, 90-day retention): original/rewritten command, input/output/saved tokens
(estimated as `ceil(chars / 4)` — no real tokenizer), savings %, exec time.
`rtk gain [--project] [--daily] [--format json]` reports totals; `rtk
discover` scans session history for missed rewrites; `rtk cc-economics`
correlates with ccusage spend.

### 1.6 Embeddability facts

- **Binary-only crate**: no `lib.rs`, no `[features]`; ~78.5 k LoC of Rust.
- Deps include `anyhow` (public API tier conflict for coco), `rusqlite`
  (bundled SQLite), `ureq` (telemetry), `lazy_static`.
- Sync single-threaded by design; startup < 10 ms; 5–15 ms filter overhead;
  ~4 MB stripped binary.

---

## 2. Where rtk helps coco-rs — and where it must not

### 2.1 Built-in Grep / Glob / Read: **do not route through rtk**

The user-visible question was: our Grep/Glob are built-in `rg` *library*
calls, not Bash — can rtk apply there? Answer: it could only regress them.

| Tool | coco-rs implementation | rtk equivalent | Verdict |
|---|---|---|---|
| `Grep` | In-process `grep-regex` + `grep-searcher` + `ignore` walker inside `spawn_blocking` with a 20 s timeout; 250-match default head limit, 500-col line cap, 20 k-char persistence bound, mtime-sorted `files_with_matches` (`core/tools/src/tools/grep.rs`) | `rtk grep` **shells out to system `grep`/`rg`**, regroups by file, 200-result cap | Built-in is strictly superior: no subprocess, structured pagination, budget-integrated. Routing through rtk adds a PATH dependency and loses modes/pagination. §2.3 decomposes rtk's grep −80 % — most of it is already banked; the remainder is adoptable natively. |
| `Glob` | In-process `ignore::WalkBuilder`, 100-path cap, mtime-ascending (`core/tools/src/tools/glob.rs`) | `rtk find` (own `ignore`-crate walk, 50-result cap) | Same walker crate, fewer knobs. No gain. |
| `Read` | cat-n projection + image/PDF/notebook handling + `file_unchanged` dedup (`core/tools/src/tools/read.rs`) | `rtk read` (level-based comment/body stripping) | rtk's `aggressive` level (signatures-only) is genuinely interesting, but it belongs as a native `Read` option or in `retrieval`/LSP — not worth an external binary on the hot read path. Deferred; see §9. |

These outputs are already shaped and capped by the Level-1/Level-2
tool-result budget (`core/tool-runtime/src/tool_result_storage.rs`,
`app/query/src/engine_prompt.rs`). rtk's own docs concede the same split:
its Claude Code hook only fires on Bash; built-in tools bypass it by design.

### 2.2 Bash: the gap rtk actually fills

Today the Bash output pipeline is: raw stdout/stderr → **head-only** byte cap
(`truncate_output`, `core/tools/src/tools/bash.rs:1592`; default 30 000 bytes,
clamped ≤ 150 000 via `BashConfig.max_output_bytes`) → hint-strip →
`render_for_model` (`bash.rs:664`). There is **no semantic filtering and no
output-side secret redaction**.

Two concrete failure modes rtk fixes:

1. **Head truncation destroys the signal.** `cargo test` / `pytest` put the
   failure summary at the *end*; a 30 k head cut keeps hundreds of passing
   lines and drops the failures. rtk's failure-focus strategy inverts this:
   keep failures, drop the noise — smaller *and* higher-signal.
2. **Cheap commands are chatty.** `git push` ≈ 200 tokens of object-counting
   noise for one bit of information ("ok main"). Multiplied across a session
   (§7), this is the fat tail of context growth.

**Conclusion: the integration surface is exactly one tool — `Bash`.**
(`PowerShell` is out of scope for phase 1; rtk's rewrite rules target POSIX
command shapes.)

### 2.3 Decomposing rtk's grep/rg −80 % — what coco already banks

rtk's session table credits grep/rg with 16 k → 3.2 k tokens (−80 %). That
number is measured against **raw `grep`/`rg` run in a shell** — unlimited
matches, full-width lines, the file path repeated on every output line.
Mechanically, `rtk grep`/`rtk rg` runs the real engine (plus `-n -H --null`
parse flags) and compresses via four levers:

1. **Result caps** — 200 total, 25 per file (`[limits]` config).
2. **Group-by-file** — the path prints once per file block; overflow becomes
   `+N more in <file>` / `+N more files` markers.
3. **Line truncation** at 80 chars.
4. **Tee overflow** to a recovery file.

Built-in `Grep` already banks most of these: the default output mode is
`files_with_matches` (paths only — more compact than anything rtk emits),
and content mode caps at 250 matches (`DEFAULT_HEAD_LIMIT`), truncates lines
at 500 cols, paginates, and rides the 20 k-char persistence bound. **The one
structural lever coco lacks is grouping**: `format_content`
(`grep.rs:895-940`) emits flat `path:line:content` rows — the path is
repeated on every match line. There is also no per-file cap, so one hot file
can consume the entire 250-match budget.

Worked example for a full content-mode result (250 matches across 25 files,
40-char average path): flat format spends 250 × 41 ≈ 10.3 k chars on path
prefixes; grouped spends 25 × 41 ≈ 1 k — roughly **2.2 k tokens (~35–45 % of
such a result) saved with zero information loss** (grouped grep output is
exactly what rtk ships to Claude Code sessions today, so model readability
is proven).

**Adopt natively in `grep.rs::format_content` — no rtk involvement**
(phase 1.5):

- Grouped content rendering: one `file` header line per file, then
  `line: content` rows.
- Per-file cap (default 25, input-overridable) with `+N more in <file>`
  markers — a coverage win as much as a token win.
- Keep the 500-col line cap (rtk's 80 chars suits status output; models need
  code context), keep mtime sorting and pagination.

Corollary: rtk's grep/rg row does **not** transfer to coco through the Bash
rewrite (models are steered to built-in `Grep`); the §7.2 projection already
excludes it, and this native change captures the residual gain instead.

### 2.4 Glob & files-mode: smaller absorptions worth taking

Same move — absorb the formatting idea, skip the binary:

- **Glob grouped-by-directory rendering** (rtk's `find`/tree-compression
  idea). Today Glob emits one full relative path per line (100-path cap);
  repeated directory prefixes dominate the payload. A `dir/` header with
  indented filenames cuts a full 100-path result from ≈4.5 k chars to
  ≈1.5 k (−65 %); path reconstruction is a mechanical join and the format is
  proven model-readable (it is what rtk ships). One tension to resolve:
  Glob's contract is mtime-ascending (glob.rs:347, "Do not flip to
  newest-first"), so grouping must order dir-groups by their newest member
  (ascending) and files within a group by mtime — recency stays local to the
  tail. Only group when ≥20 paths span ≥3 directories; below that flat wins.
- **`files_with_matches` inline counts** — `path (N)` per line; the data is
  already computed by the count path. Slightly *more* tokens, but it steers
  the model to the right file and saves follow-up content-mode calls.
- **Overflow markers over bare pagination** — truncation footers gain
  rtk-style `+N more files` / `+N more in <file>` markers alongside the
  existing pagination hint, so the model knows *where* the tail went, not
  just that it was cut.

All of these are formatting-layer changes in `grep.rs` / `glob.rs` with
insta snapshot coverage; none need rtk at runtime.

---

## 3. Can rtk be "built in"? — integration-form decision

| Option | Description | Assessment |
|---|---|---|
| **A. Subprocess (phase 1)** | Detect `rtk`/`rr-rtk` on `$PATH`, call `rtk rewrite` pre-spawn (§4) | ~5–15 ms per Bash call (Bash is serialized anyway); zero new workspace deps; version-decoupled; absent binary ⇒ transparent no-op. The exit-code contract is small and stable. This is also how rtk integrates with *every* host it supports (Claude Code, Cursor, Gemini, …) — no agent embeds it. |
| B. `rr-rtk` community crate | [crates.io/crates/rr-rtk](https://crates.io/crates/rr-rtk) `0.42.3-rr.2`, Apache-2.0, "personal fork of rtk-ai/rtk, pre-release until upstream PR lands" | **Still binary-only** — verified from the published crate: `[[bin]] name = "rr-rtk"`, no `src/lib.rs`, no `[lib]`, no `[features]`. It solves **distribution** (`cargo install rr-rtk` works; the `rtk` name on crates.io is squatted by the unrelated "Rust Type Kit"), *not* embedding. Tracks upstream closely (0.42.3 vs upstream 0.42.4, June 2026). Caveat: it installs a binary named `rr-rtk` while its rewrite engine still emits hardcoded `rtk ` prefixes (`src/discover/registry.rs`), so its own rewrites are not executable without an `rtk` symlink or the coco-side fixup in §4.5. Adopted as a *supported install path*, not an embedding route. |
| C. Embed as library (fork adds `src/lib.rs`, or git-subtree vendor) | Depend on the filter core as source | **No lib target exists today, but adding one is mechanically small** (§3.2) — and embedding is the only route that removes the local-install requirement, so it is the **phase-2 target**. Key architectural insight: embedded integration should be *post-exec filtering*, not in-process rewrite-decision (§3.3) — which sidesteps the "binary still required at exec time" objection entirely. Costs are dependency gating and fork upkeep, not feasibility. |
| D. Hand-port the filter core | Re-implement the strategies natively in coco | Permanent re-implementation of ~85 rewrite rules + 63 TOML filters + 42 command parsers that upstream updates ~weekly (75 releases in the CHANGELOG; 0.27 → 0.42 between 2026-03 and 2026-06). Rejected — forking the *source* (option C) gets the same effect without rewriting it by hand. |
| E. User-space hook only | Ship nothing; users configure a coco PreToolUse command hook that shells to rtk | Already *possible* (coco hooks support `updated_input`), but: depends on hook-protocol parity with Claude's JSON, rtk would consult *Claude's* settings files for its verdict, no sandbox/background interplay, no observability. Kept as an escape hatch, not the product. |

Option A is also forward-compatible: if a real library crate ever exists,
only `exec/shell/src/rtk/` changes; the feature gate, config, and call-site
contract stay identical.

### 3.1 Local install requirement & managed install

**Phase 1 requires a locally installed binary** — the same contract rtk has
with every other host agent. Supported install paths, in preference order:

1. `brew install rtk` (official, binary named `rtk`)
2. upstream `install.sh` → `~/.local/bin/rtk`
3. `cargo install rr-rtk` (community crate; binary named `rr-rtk` — needs
   the §4.5 fixup or an `rtk` symlink)
4. `cargo install --git https://github.com/rtk-ai/rtk`

Absent binary ⇒ the feature silently no-ops (one `info!` per session).

**Phase 1.5 removes the UX cliff with a managed install**, following the
`services/lsp` `Installer` precedent (`installer.rs::is_installed`): when the
feature is toggled on and no binary is found, `coco doctor` / the
`/experimental` flow offers to install — `cargo install rr-rtk` when cargo
exists, else download the official GitHub release tarball (~4 MB static
binary, per-platform) into `<config_home>/bin` with SHA-256 verification.
The managed path also owns creating the `rtk` symlink when the underlying
binary is `rr-rtk`, so rewrites stay executable without the runtime fixup.
Both are interim: phase 2 (§3.3) removes the install requirement entirely.

### 3.2 Vendor or fork to embed — mechanics, costs, recommendation

Adding a lib target is mechanically small; the real costs are dependency
hygiene and upstream pace, not code:

- **Minimal `src/lib.rs`** (~40 lines): `pub mod cmds; pub mod core;
  pub mod discover; pub mod parser;` alongside the untouched `main.rs`.
  Module visibility is already `pub` throughout (doctests even reference
  `rtk::tracking::Tracker` as if a lib existed). What coco gains in-process:
  `discover::registry::rewrite_command` + lexer (rewrite decision),
  `core::toml_filter` (63 declarative text→text filters, incl. project
  `.rtk/filters.toml` loading), `core::filter` (`FilterLevel` code
  stripping), `core::guard` (never-worse), and the per-family **pure
  formatters** — `git::format_git_output(stdout, subcmd, verbose)`, the
  cargo/pytest/rake state machines — text in, text out; *execution stays on
  coco's side*. Library callers never touch `run_cli`, so telemetry pings
  and SQLite tracking simply don't execute (both live in CLI paths).
- **Dependency gating** (the real work, ~1–2 days): without `[features]`
  the crate drags `rusqlite` (bundled libsqlite3 — cold-build cost),
  `ureq`, `clap`, `colored` into coco's graph. A `core`-vs-`cli` feature
  split keeps the lib tier at regex/serde/toml/ignore. Deferrable (the cost
  is build-time, not runtime) but should land before the dep is
  load-bearing.

  What the split keeps vs drops, by module:

  | Module | What it provides | In `core`? |
  |---|---|---|
  | `discover/` (registry, lexer, rules) | `classify_command` (argv → family match — reused as the post-exec dispatch in §3.3), `rewrite_command`, unattestable-construct guard | **yes** — this is the brain |
  | `core/` toml_filter, filter, guard, truncate, utils | 63 declarative text→text filters, `FilterLevel` code stripping, never-worse guard, ANSI strip | **yes** |
  | `parser/` | shared output-parsing types consumed by the filters | **yes** (transitive) |
  | `cmds/` pure formatters | per-family text→text parsers (git stats extraction, cargo/pytest failure-focus, …); the `run()` exec wrappers around them are dead code for a lib caller | **yes** (formatters only) |
  | `core/tracking` | SQLite savings ledger (`rusqlite`) — called from CLI `run()` lifecycles, never from the formatters | no — coco has otel + cost tracking |
  | `core/tee` | raw-overflow recovery files | no — coco's Level-1 `<persisted-output>` persistence is strictly better |
  | `analytics/` | `rtk gain` / `cc-economics` / `session` reports over the SQLite ledger | no — CLI reporting UX; phase 1 reaches it through the binary (`rtk gain --format json`) when present |
  | `hooks/` (11.5 k LoC — the largest module) | host-agent installers (`rtk init`), per-agent stdin/stdout shim adapters (`rtk hook claude/cursor/gemini/copilot`), the `rtk rewrite` CLI wrapper, *foreign-host* permission readers (Claude/Cursor/Gemini settings files), custom-filter trust store, hook audit log | no — **except the small `trust` submodule**: `toml_filter::load()` hard-depends on `hooks::trust::check_trust_with_content` to gate custom filter files (toml_filter.rs:191/208, SA-2025-RTK-002), so `trust` moves into the core tier (§3.4). The rest is adapter plumbing for hosts rtk cannot modify |
  | `learn/` | mines *Claude Code* session JSONL for fail-then-succeed pairs → generates `.claude/rules/cli-corrections.md` | no — Claude-specific transcript format; coco has its own memory subsystem |
  | `core/telemetry` | opt-in anonymous ping (`ureq`) fired from `run_cli()` startup | no — must never execute in-process; gating it deletes the `ureq` dep |

  Two integration details this split surfaces: (a) the embedded tier must
  force-disable `colored` output (`colored::control::set_override(false)`,
  or `strip_ansi` afterwards) — formatters colorize for terminals, and ANSI
  must not leak into model-facing tool results; (b) custom filter files
  (`.rtk/filters.toml`) are consent-gated through `hooks::trust` — the
  embedded tier carries that submodule and **shares rtk's trust store**, so
  one approval covers both tools. Full shared-config design in §3.4.
- **Three delivery vehicles**, ranked:
  1. **Own fork + pinned git dependency** — `rtk = { git = "…/rtk-fork",
     rev = "…" }`. Fastest start, no publish ceremony, no repo bloat.
     No crates.io involvement at all: cargo clones the pinned commit into
     `~/.cargo/git/`, builds it from source like any workspace member, and
     `Cargo.lock` records the exact hash (reproducible; use `rev`, never
     `branch`). The fork also keeps upstream's package name `rtk` — the
     crates.io name-squatting problem doesn't exist off-registry. Two
     caveats: fresh clones need network/credentials at build time (cargo
     caches afterwards; `cargo vendor` covers air-gapped CI), and a crate
     with git deps cannot itself be published to crates.io — irrelevant
     while coco crates are unpublished, but it becomes the trigger to move
     to vehicle 2 if that ever changes. Rebase on *our* cadence: quarterly
     is fine — git/cargo filter semantics don't rot weekly even though
     upstream ships weekly.
  2. **Upstream the lib PR** (to rtk-ai/rtk directly, or via rr-rtk, whose
     author is already mid-flight on packaging PRs and would likely take a
     `[lib]` + `[features]` contribution) → switch to the published crate
     when it lands. End state: zero fork maintenance.
  3. **git subtree vendor** (`coco-rs/vendor/rtk/`, workspace-`exclude`d so
     workspace lints/fmt/clippy/`check-error-policy` don't fight upstream
     style, consumed as a path dep). Maximum control, offline builds — but
     +78 k LoC in the repo and recurring subtree-merge conflicts against
     local patches at every sync.

  **Recommendation: 1 now, 2 in parallel; 3 only if fork hosting is
  unacceptable.** Publishing a coco-owned *binary* crate remains pointless
  (rr-rtk already exists); the fork's sole purpose is the lib target.

**Fork delta & upkeep, quantified.** "Fork" here does not mean maintaining
78 k LoC — upstream keeps maintaining those; the fork maintains a *delta*:

| Fork stage | Delta | Upstream-merge conflict surface |
|---|---|---|
| v0 — lib target only | **one added file** (`src/lib.rs`, ~40 lines of `pub mod` re-exports); zero upstream files modified; full dep graph accepted (`rusqlite` bundled ≈ +30–60 s cold build, no runtime cost — tracking/telemetry live in CLI paths a lib caller never executes) | conflict-free by construction (additive file) |
| v1 — `core`/`cli` feature split | `Cargo.toml` `[features]` + `#[cfg(feature = "cli")]` on a handful of module roots (analytics, hooks, learn, telemetry); ~100-line delta, est. 1–2 days once | small, recurring on `Cargo.toml`/`mod.rs` lines |
| ongoing | quarterly `git merge upstream/master` + rev bump in coco's lockfile, est. 1–2 h including running upstream's test suite | — |

The residual risk is upstream refactors moving modules or changing pure
formatter signatures — absorbed by keeping all coco→rtk calls behind thin
per-family adapters in `exec/shell/src/rtk/` (one function per family), so
drift is localized to one file, never scattered through BashTool.

### 3.3 Phase-2 target: embedded core + post-exec filtering (zero install)

Embedding **inverts the architecture**. The binary needs the rewrite trick
because rtk sits *outside* the agent — hijacking the command is its only way
to see the output. coco sits *inside*: `BashTool` already captures
stdout/stderr. So the correct in-process integration is not "run the rewrite
engine in-process and still exec a binary we had to install" — it is a
**post-exec filter stage**:

```
spawn ORIGINAL command (no rewrite, no rtk process, no permission nuance)
  → capture stdout/stderr                      (existing path, unchanged)
  → rtk-core match on (argv, exit code, captured text)
       → family formatter (git / cargo / pytest / …) or TOML filter
       → never-worse guard (trivial port: compare estimated sizes)
       → catch_unwind around the pure filter call — a filter panic
         degrades to raw output, never takes down coco
  → truncate_output(30 k) → render_for_model   (existing choke points)
```

| | Phase 1: binary rewrite | Phase 2: embedded post-exec |
|---|---|---|
| Local install | required (rtk / rr-rtk) | **none** |
| Command executed | `rtk git status` | `git status`, unmodified |
| Rewrite trust model (§4.6) | applies | **moot** — command untouched |
| rr-rtk name fixup (§4.5) | needed | moot |
| Background tasks | skipped (buffering) | v0: **not filtered** — only completed foreground / TaskRuntime-`Terminal` output is (the `run_in_background` stream is captured raw); incremental stream filtering is future work |
| Sandbox | skipped (rtk's SQLite write) | **works** — no rtk process, no DB |
| Specialized text parsers (git, cargo/pytest failure-focus) | full | v0: **deferred** — `cmds` formatters not yet lib-exposed (§10-Q6); TOML long tail only |
| TOML long tail (63 tools) | full | **full** — engine is text→text by design |
| Compound / piped commands | per-segment rewrite | **not filtered** — a first-word filter can't safely apply to combined multi-segment output; passes through raw |
| JSON-mode filters (rubocop, golangci-lint, rspec…) | full (rtk injects `--format json`) | degraded to text parsing, until a small per-family argv flag-injection table is added pre-spawn (visible, auditable input tweak — far narrower than a rewrite) |
| Raw-output recovery | rtk tee files | v0: the filtered text **replaces** raw in both the inline view and the Level-1 `<persisted-output>` artifact (coco persists the model-facing stdout, not the pre-filter capture). Recover the raw output by re-running with the `RTK_DISABLED=1` prefix. Persisting the pre-filter capture for recovery is future work. |
| Rules freshness | `brew upgrade` | pinned rev, bumped on coco's cadence |

The dominant wins — the git family, cargo/pytest failure-focus, the TOML
long tail — are text-parsing filters that work post-hoc. The JSON-injection
families are a minority, deferred behind the flag table.

**Implementation seam.** The stage is one function —
`apply_rtk_filter(argv, exit_code, stdout) -> Option<String>` — owned by
`exec/shell/src/rtk/` alongside the phase-1 modules:

```
exec/shell/src/rtk/            (phase-2 additions)
  filter.rs     // post-exec stage: classify → family adapter / TOML engine
                //   → never-worse → catch_unwind wrapper
  families.rs   // thin per-family adapters over the vendored formatters
                //   (one fn per family; upstream drift localizes here)
  trust.rs      // consent-prompt glue over the vendored hooks::trust store
```

`BashTool` calls it at the single point where captured output exists and
truncation has not yet run — factored as one helper in front of the two
`truncate_output` call sites (`bash.rs:1108` task-runtime path, `bash.rs:1320`
foreground path) so both paths share the stage. The `RTK_DISABLED=1`
convention is honored here too: the dispatcher strips env-var prefixes
before classifying and skips filtering when it sees `RTK_DISABLED=` — the
same escape hatch in both tiers. With phase 2 the phase-1 facade name
generalizes (`RtkRewriter` → `RtkIntegration`); the `ToolUseContext` slot
and construction site are unchanged.

**Sequencing:** phase 1 (subprocess) still ships first — it is a week of
work and immediately serves users who already run rtk. Phase 2 supersedes it
as the default tier once the lib-target fork exists; the binary route stays
user-selectable via `rtk.engine` (§3.5), primarily for JSON-mode fidelity
until the flag-injection table reaches parity. Feature gate, config, metrics,
and the `Feature::Rtk` key are shared across both tiers — nothing user-facing
changes at the switch.

### 3.4 Shared filter & engine config — `.rtk/filters.toml`, `config.toml`

Requirement: embedded coco and a standalone rtk CLI on the same machine
read the **same configuration files** — no coco-side mirror, no divergence.
The embedded engine achieves this by calling the vendored loaders
(`TomlFilterRegistry::load()`, `Config::load()`) unchanged.

**Filter file format** — one `[filters.<name>]` section per filter, plus
inline tests:

```toml
# <project>/.rtk/filters.toml — project-local, committable
[filters.my-tool]
description   = "Strip noise from my-tool output"
match_command = "^my-tool\\b"        # regex against the full command string
strip_ansi    = true
filter_stderr = false                 # merge stderr before filtering (banner tools)
strip_lines_matching = ["^\\s*$", "^Progress: "]
keep_lines_matching  = []             # alternative: keep-only mode
replace       = [{ pattern = " in \\d+ms", replacement = "" }]
match_output  = [{ pattern = "already up to date", message = "ok" }]  # short-circuit
truncate_lines_at = 120
max_lines     = 40                    # head cap, applied after filtering
tail_lines    = 0                     # tail cap, applied last
on_empty      = "my-tool: ok"         # emitted when everything got filtered

[[tests.my-tool]]
name     = "strips progress noise"
input    = "Progress: 50%\nDone"
expected = "Done"
```

Fixed 8-stage pipeline: `strip_ansi → replace → match_output →
strip/keep_lines → truncate_lines_at → tail_lines → max_lines → on_empty`.
TOML filters strip noise lines — they don't reformat; reformatting is what
the Rust family formatters do.

**Lookup chain — identical in both tools, first match wins:**

1. `<project>/.rtk/filters.toml` (shadows same-name filters below, with a
   warning)
2. `~/.config/rtk/filters.toml` (user-global)
3. builtin blob — 63 filters embedded at the vendored rev
4. no match → raw output

coco resolves "project" against the Bash tool's effective cwd
(worktree-aware), matching rtk's process-cwd resolution. One accepted skew:
the builtin blob is frozen at the vendored rev, so a newer standalone rtk
may know more builtin filters than embedded coco until the rev is bumped —
user/project files never skew, they are read from disk by both.

**Trust model — one store, both tools.** Custom files are SHA-256-pinned:
`toml_filter::load()` calls `hooks::trust::check_trust_with_content` per
gated path and silently skips `Untrusted` / `ContentChanged` files
(SA-2025-RTK-002 — a malicious repo must not ship a filter that hides
output from the model). The design keeps that contract and makes consent
bidirectional:

- The fork's `core` tier carries the `hooks::trust` submodule.
- coco reads **and writes** the same store
  (`~/.local/share/rtk/trusted_filters.json`): a file trusted via
  `rtk trust` is honored by coco; a file approved inside coco is honored by
  the rtk CLI. One approval, both tools; coco never invents a second store
  format.
- Consent UX: in interactive sessions, first sight of an untrusted or
  changed project filter file raises a coco-native approval prompt (path +
  SHA-256; on `ContentChanged`, that it differs from the approved hash).
  Headless/SDK runs skip untrusted files silently — exactly rtk's behavior.

**Engine config `~/.config/rtk/config.toml`** is shared the same way: the
embedded engine honors the engine-relevant knobs — `[hooks]`
`exclude_commands` / `transparent_prefixes` (classification), `[filters]`
ignore rules, `[limits]` (grep/status caps used by family formatters) — and
ignores the subsystem knobs coco replaces (`[tracking]`, `[telemetry]`,
`[tee]`, `[display]`). Precedence is a **union of vetoes**: coco's own
`rtk.exclude_commands` (settings.json, §5.2) adds to the file's list;
either source can exclude a family, neither can un-exclude.

### 3.5 Tier selection — the `rtk.engine` policy

Once both tiers exist, one gate (`Feature::Rtk`) needs a selection policy.
Per repo convention this is a sub-toggle inside `RtkConfig` (§5.2), never a
second `Feature` variant:

```rust
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RtkEngine {
    /// Default. Embedded core filters post-exec; the external binary is
    /// consulted only for commands the core cannot handle at full fidelity.
    #[default]
    BuiltinFirst,
    /// Binary rewrite when available; embedded core as the fallback.
    ExternalFirst,
    /// Never spawn the binary (deterministic CI / air-gapped).
    BuiltinOnly,
    /// Never use the embedded core (parity debugging; `rtk gain` ledger).
    ExternalOnly,
}
```

**The asymmetry that shapes the whole policy:** the two tiers act at
different lifecycle points — external is a **pre-spawn rewrite**, builtin is
a **post-exec filter**. Therefore "first choice not satisfied → try the
other" must be decided **before spawn**, and "satisfied" must be defined as
a pre-spawn-checkable predicate:

| Tier | "satisfied" (all pre-spawn checkable) | Runtime failure (post-spawn) |
|---|---|---|
| builtin | `classify_command` matches a family **and** the family adapter is marked full-fidelity (pure formatter, or a TOML filter whose `match_command` hits) | filter panic / never-worse reject → **raw output**, no second chance |
| external | binary healthy **and** not background / sandboxed / excluded **and** `rtk rewrite` returns exit 0\|3 with output | rtk's own fail-safe passthrough inside the child |

Decision matrices:

```
builtin_first:                          external_first:
  classify(cmd)                            external satisfied?
    ├─ full-fidelity family                  ├─ yes → rewrite; POST-FILTER DISABLED
    │    → plan builtin post-filter          └─ no  → builtin family match?
    │      (no rewrite)                           ├─ yes → builtin post-filter
    ├─ degraded family (JSON-mode)                └─ no  → skip (raw + 30 k cap)
    │    → external satisfied? rewrite
    │      else builtin text-parse
    └─ no family
         → external satisfied? rewrite
           else skip (raw + 30 k cap)
```

**Three hard rules that make the scheme safe:**

1. **Fallback never re-executes.** All tier arbitration happens pre-spawn.
   If the builtin filter disappoints *after* the command ran (panic,
   never-worse reject), the result degrades to raw output — we never run
   the command a second time under the binary; shell commands have side
   effects.
2. **No double filtering.** When the external rewrite fires, the builtin
   post-filter stage is disabled for that call. rtk's already-compressed
   summary fed into a family formatter is semantic mangling (`git status`'s
   formatter parsing "3 files, +142/−89" produces garbage); never-worse
   bounds the size but not the meaning.
3. **Both miss → skip is the designed floor**, not an error: raw output
   through the existing 30 k truncation. Every arm of the matrix terminates
   in compressed-correct or raw-correct; there is no failure mode that
   loses output.

**Why `BuiltinFirst` is the default:**

- **Determinism** — output shape is fixed by the vendored rev, not by
  whatever rtk version happens to be on `$PATH` (external output shapes
  drift with upstream's weekly releases).
- **Coverage** — builtin works exactly where external is structurally
  skipped: sandboxed commands, background tasks (§4.3).
- **Observability** — builtin sees raw *and* filtered, feeding the §7.3
  A/B histograms; the external tier hides raw output from coco entirely.
- **Latency** — no per-call subprocess probe + rtk process startup.
- **Trust** — the executed command is never modified (§3.3 table).

**When `ExternalFirst` is legitimately better** (an honest trade, hence
user-selectable): full-fidelity JSON-injection families (rubocop,
golangci-lint, rspec) before the in-process flag table exists; always-latest
upstream rules without waiting for a rev bump; and keeping the `rtk gain`
ledger fed — the binary only tracks commands it proxied, so under
`BuiltinFirst` coco traffic disappears from `rtk gain` and coco's own
metrics become the source of truth.

Phase note: the knob ships with phase 2. During phase 1 only the external
tier exists — the setting is accepted but everything effectively runs
`ExternalOnly`.

**v0 collapse (honest caveat).** With `cmds` family formatters not yet exposed
(§10-Q6), the embedded tier has no *degraded-family* case, so `BuiltinFirst`
never falls back to a pre-spawn rewrite — it is byte-for-byte identical to
`BuiltinOnly` today. The two config values only diverge once family formatters
land (then `BuiltinFirst` must arbitrate *per command*, which the current static
per-session capability booleans cannot express — that step is a deliberate
signature change on `BashOutputRewriter`, not a fill-in-the-TODO). `ExternalFirst`
and `ExternalOnly` are already distinct (the post-exec fallback only fires under
`ExternalFirst`).

---

## 4. Architecture (phase 1)

This section is the subprocess tier only. Phase-2 architecture and its code
seam live in §3.3; tier arbitration between the two in §3.5.

### 4.1 Placement

New module `exec/shell/src/rtk/` (crate `coco-shell` — it already owns shell
semantics: read-only classification, compound analysis, providers):

```
exec/shell/src/rtk/
  mod.rs        // RtkRewriter facade + RewriteOutcome
  detect.rs     // binary probe: which("rtk") → which("rr-rtk"), version gate, OnceCell cache
  rewrite.rs    // `rtk rewrite` subprocess call, timeout, exit-code mapping, rr-rtk fixup
  *.test.rs     // companion tests (stub binary via utils/cargo-bin)
```

```rust
/// Session-wide, shared via Arc on ToolUseContext (mirrors `shell_provider`).
pub struct RtkRewriter {
    config: RtkConfig,
    binary: tokio::sync::OnceCell<Option<RtkBinary>>, // probe once per session
}

pub struct RtkBinary {
    path: PathBuf,             // config override, else which("rtk") then which("rr-rtk")
    flavor: RtkFlavor,         // Rtk | RrRtk — RrRtk triggers the §4.5 prefix fixup
    version: (u32, u32, u32),  // from `--version`; >= 0.23.0; pre-release tags
                               // (`0.42.3-rr.2`) compare by their base triple
}

pub enum RewriteOutcome {
    /// exit 0|3 with non-empty stdout — execute this instead.
    Rewritten(String),
    /// Execute the original command. Reason recorded for metrics/tracing.
    Passthrough(PassthroughReason),
}

pub enum PassthroughReason {
    BinaryMissing,
    VersionTooOld,
    Background,     // run_in_background=true: rtk buffers, would break TaskOutput streaming
    Sandboxed,      // sandbox active: rtk's SQLite write would fail under ReadOnly/Strict
    Excluded,       // coco-side exclude_commands first-word match
    NoEquivalent,   // exit 1
    HostDeny,       // exit 2 (rtk consulted foreign host rules — informational only)
    Timeout,        // rewrite probe exceeded rewrite_timeout_ms (default 500) — killed
    SpawnError,
    ShapeMismatch,  // rr-rtk fixup: a rewritten segment didn't start with the `rtk` token (§4.5)
}

/// Execution-site facts the skip conditions (§4.3) need.
pub struct RewriteSite {
    pub background: bool, // BashInput.run_in_background
    pub sandboxed: bool,  // sandbox snapshot decided it will wrap this command
}
```

The public API is **infallible** (`async fn rewrite(&self, command: &str, site: RewriteSite) -> RewriteOutcome`);
every failure maps to a `Passthrough` reason. No error type crosses the crate
boundary. The subprocess is invoked argv-style
(`Command::new(path).arg("rewrite").arg(command)`) — no shell interpolation of
the command string on the way in.

### 4.2 Insertion point and ordering — the load-bearing decision

Everything that *judges* the command runs on the **original** string; the
rewrite is applied last, at the single dispatch point in
`BashTool::execute` (`core/tools/src/tools/bash.rs`, before the
task-runtime / foreground fork):

```
model emits Bash{command: "git status && cargo test"}
  │
  ├─ check_permissions            on ORIGINAL   → Bash(git status:*) rules match unchanged
  ├─ read-only classification     on ORIGINAL   → is_read_only_command untouched
  ├─ security analysis            on ORIGINAL   → compound/destructive checks untouched
  ├─ sandbox snapshot decision    on ORIGINAL
  │
  ├─ rtk rewrite (feature-gated)  ──────────────→ "rtk git status && rtk cargo test"
  │      exit 0|3 → rewritten · else original
  ▼
spawn via ShellExecutor / TaskRuntime (quoting, snapshot, COCO_SHELL_PREFIX,
netns prefix, sandbox wrap — all downstream stages unchanged)
```

**No hook involvement — the hook layer is bypassed entirely.** rtk's hook
machinery (`rtk init`, the settings.json `PreToolUse` entry, `rtk hook
claude`, `rtk-rewrite.sh`, `RTK.md`) exists because rtk cannot modify the
hosts it targets — a stdin/stdout JSON shim is its only way in. coco *is*
the host: `RtkRewriter` calls the same underlying engine through its bare
CLI surface (`rtk rewrite "<cmd>"`, §1.4) directly at this insertion point.
Nothing is installed, no settings file is written, no hook event fires, and
users must **not** run `rtk init` for coco. If a user has independently
configured a coco PreToolUse hook that shells to rtk, the combination stays
idempotent — the engine returns already-`rtk`-prefixed commands unchanged.

Why here and not the alternatives the codebase offers:

- **Not `Allow{updated_input}` from `check_permissions`** — that path exists
  (`app/query/src/tool_call_preparer.rs::resolve_effective_input_from_permission`)
  and the evaluator does match rules on the pre-rewrite input, but returning
  `Allow` from the tool's opinion slot entangles a pure execution-layer
  concern with permission semantics, and the skip conditions
  (background/sandbox) are execution-time facts anyway.
- **Not a built-in PreToolUse hook** — protocol overhead, ordering vs user
  hooks, and the rewritten string would be re-validated for no benefit.
- **Not `BashProvider::build_exec_command`** (the `COCO_SHELL_PREFIX`
  precedent) — that wraps the *whole* compound string with one prefix; rtk
  needs per-segment rewriting, and the provider layer is too late for
  tracing/metrics attribution and too mixed with quoting.

Consequences of this ordering:

- The user's allow/deny rules (`core/permissions/src/shell_rules.rs::match_bash_rule`)
  keep matching `git status`, never `rtk git status`. No permission UX change,
  no new rules to teach, `strip_safe_wrappers` needs no `rtk` entry.
- The Ask flow shows the original command; approval happens before `execute`,
  so the rewrite always follows the user's decision.
- The model's tool_use block in history keeps the original command (the
  assistant message is never mutated).

### 4.3 Skip conditions

| Condition | Outcome | Rationale |
|---|---|---|
| `Feature::Rtk` disabled | rewriter not even constructed | subsystem gate at context assembly (see §5) |
| binary missing / < 0.23.0 | passthrough, one `info!` per session | silent degradation, same posture as rtk's own shims |
| `run_in_background: true` | passthrough | rtk captures-then-prints; would stall incremental `TaskOutput` streaming of dev servers |
| sandbox will wrap the command | passthrough | rtk writes its SQLite history under `~/.local/share/rtk` — blocked/EROFS under ReadOnly/Strict; skipping avoids a class of in-sandbox failures |
| first word ∈ `rtk.exclude_commands` | passthrough | cheap coco-side veto without spawning the probe |
| exit 1 / 2 / unknown / empty stdout | passthrough | protocol says no rewrite |
| probe exceeds `rewrite_timeout_ms` | kill probe, passthrough | a hung rewriter must never delay the real command |
| rr-rtk fixup shape check fails | passthrough | §4.5 — never execute a rewrite we can't fully account for |

rtk's own engine additionally honors `~/.config/rtk/config.toml`
`exclude_commands` / `transparent_prefixes`, heredoc/substitution guards, and
the `RTK_DISABLED=1` per-command prefix — we inherit all of that for free.

### 4.4 Runtime interplay

- **Timeouts / kill**: coco's Bash timeout kills the process group; rtk's
  child guard reaps its child. rtk itself has no timeout logic — coco's
  applies to the whole `rtk → tool` chain, unchanged.
- **Exit codes**: rtk preserves them; `render_for_model`'s exit-code
  interpretation (`bash.rs:740`) is unaffected.
- **Output cap**: the 30 k `truncate_output` still applies after rtk — now it
  almost never triggers, and when it does it truncates already-distilled text.
- **Tee recovery**: when rtk tees raw output to a temp file, the path appears
  in the compact output and the model can `Read` it — a natural "zoom in"
  affordance that head-truncation never had.

### 4.5 rr-rtk binary-name fixup

When the resolved binary is `rr-rtk` (no `rtk` on `$PATH`), the engine's
rewrite output still prefixes segments with the literal token `rtk` — the
prefixes are hardcoded in its registry (`"rtk git status"`, `"rtk read …"`),
unchanged by the fork. Executing that output verbatim would fail with
command-not-found.

Because prefix insertion at segment heads is the engine's **only** transform,
a bounded fixup is possible: split the rewritten string on the same top-level
separators the engine uses (`&&`, `||`, `;`, `|`, `&`), and for every segment
whose first token is exactly `rtk`, replace that token with `rr-rtk`. If any
rewritten segment fails this shape check, discard the rewrite and
passthrough (`ShapeMismatch`) — never execute a rewrite we can't fully
account for. The managed install (§3.1) avoids the fixup entirely by
symlinking `rtk` → `rr-rtk`.

Upstream issue worth filing on the fork: derive the rewrite prefix from
`current_exe()` / the crate name instead of a literal, which would delete
this whole subsection.

### 4.6 Trust model

Post-permission rewriting means the executed string differs from the approved
string. This is the same trust class as every other binary the agent invokes
(`git`, `cargo`): a malicious `rtk` on PATH is already game-over for a shell
agent. Mitigations kept deliberately simple: `binary_path` pinning in
settings, the probe requires the binary to self-identify (`rtk --version`),
and the rewrite is disabled inside sandboxes (§4.3). We do not re-run
security analysis on the rewritten string — rtk's unattestable-construct
guard refuses the shapes that would make prefixing unsound, and the rewrite
is a prefix insertion per segment, not free-form rewriting.

---

## 5. Feature gate, config, env

### 5.1 `Feature::Rtk` (`common/types/src/features.rs`)

```rust
/// Compress Bash dev-tool output (git / cargo / test runners / linters /
/// docker …) before it reaches the model — 60–90 % smaller tool results.
/// Two tiers behind this one gate, arbitrated by `RtkConfig.engine`
/// (design §3.5): the embedded rtk filter core (post-exec, zero install;
/// phase 2) and the external `rtk` binary (pre-spawn rewrite; >= 0.23.0
/// on PATH). Permission rules, read-only classification and the sandbox
/// decision always evaluate the ORIGINAL command. With neither tier
/// available the feature silently no-ops. Sub-toggles live in `RtkConfig`,
/// never as extra Feature variants.
Rtk,
```

```rust
FeatureSpec {
    id: Feature::Rtk,
    key: "rtk",
    stage: Stage::Stable,
    default_enabled: true,
},
```

`Stage::Stable` + `default_enabled: true`: RTK is **on by default**. What makes
default-on defensible is that the downside is bounded to zero — the never-worse
guard (§3.3) and the `catch_unwind` around every filter make a bad filter
degrade to raw output, never worse than today. The §7.3 metrics become
**post-hoc validation and a rollback signal**, not a promotion gate. Opt out any
time via `settings.json features.output_rewrite = false`, `COCO_FEATURE_OUTPUT_REWRITE=0`,
or `RuntimeOverrides` (the capability was renamed `Feature::Rtk` → `Feature::OutputRewrite`
during genericization; unknown feature keys are silently ignored, so the old
`features.rtk` / `COCO_FEATURE_RTK` names are inert no-ops); per-command via the
`RTK_DISABLED=1` prefix (§6). As
`Stable` (not `Experimental`) it has no `/experimental` menu entry and no
announcement — the model-facing note lives on the Bash tool description instead.

**Caveat while `cmds` is unexposed** (the `crate::Commands` coupling, §3.2; see
the upstream `refacto/vitest-decouple` branch): default-on delivers the
declarative TOML long-tail + rewrite/guard in-process, but the *specialized*
git / cargo / pytest formatters (the marquee failure-focus wins) arrive only via
the external `rtk` binary (phase 1) or once the embedded `cmds` decouple lands.
So default-on is immediately valuable for the external tier and the TOML
long-tail; embedded git/cargo/pytest compression turns on automatically at the
rev that exposes `cmds`. It never *blocks* those commands — they always run and
are captured; they simply fall back to head-truncation until compressed.

Gate placement follows the "subsystem entry point" rule: the
`ToolUseContext` builder constructs `Option<Arc<RtkRewriter>>` (phase 2:
`RtkIntegration`, same slot) only when `features.enabled(Feature::Rtk)` —
`BashTool` just consumes the context field. No `Tool::is_enabled`
involvement (Bash itself stays on).

### 5.2 `RtkConfig` (`common/config/src/sections.rs`)

Mirrors the `ShellConfig` pattern (partial → resolve(settings, env) →
`RuntimeConfig.rtk` → `ToolUseContext`):

```rust
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct PartialRtkSettings {
    #[serde(default)] pub engine: Option<RtkEngine>,
    #[serde(default)] pub binary_path: Option<String>,
    #[serde(default)] pub exclude_commands: Vec<String>,
    #[serde(default)] pub rewrite_timeout_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RtkConfig {
    /// Tier selection policy (§3.5). Default `BuiltinFirst` once phase 2
    /// ships; during phase 1 every value behaves as `ExternalOnly`.
    pub engine: RtkEngine,
    /// Explicit rtk binary; `None` → probe `$PATH` once per session.
    pub binary_path: Option<String>,
    /// coco-side skip list, matched on the first command word before the
    /// probe spawns. rtk's own `[hooks] exclude_commands` still applies
    /// inside the engine — union of vetoes (§3.4), NOT a mirror.
    pub exclude_commands: Vec<String>,
    /// Kill the `rtk rewrite` probe after this and fall back. Default 500.
    pub rewrite_timeout_ms: i64,
}
```

Deliberately **not** duplicated from rtk's own config: filter tuning, tee
mode, transparent prefixes, analytics — those stay in
`~/.config/rtk/config.toml`, owned by rtk. coco config only carries what coco
itself needs to make the spawn/skip decision.

### 5.3 Env keys (`common/config/src/env.rs`)

One new variant: `CocoRtkPath => "COCO_RTK_PATH"` (binary override, ranked
below the settings value inside `RtkConfig::resolve`). The feature toggle
needs no new key — `COCO_FEATURE_OUTPUT_REWRITE` rides the generic
`COCO_FEATURE_*` layer. `output_rewrite.rtk.mode` is deliberately settings-only
(no env key): flipping
execution tiers per-process via env invites unreproducible sessions. No
legacy aliases; backward compatibility is a non-goal.

---

## 6. Transparency & UX

- **Envelope**: the Bash result JSON keeps `command` = original and gains a
  typed `rtk: Option<RtkTier>` field naming which tier acted
  (`"builtin"` | `"external"`, absent when neither did), plus
  `rtkCommand: Option<String>` when the external rewrite ran. The TUI
  renderer may show a dim `rtk` badge; the SDK/NDJSON layer gets both
  fields for free.
- **Model awareness**: when the feature is active, one line is appended to
  the Bash tool description: *"Dev-tool command output is compressed by rtk;
  prefix a command with `RTK_DISABLED=1` to get raw output."* This gives the
  model a documented, per-command escape hatch instead of it fighting the
  filter. The prefix works in **both tiers** (external: the engine skips it;
  builtin: the §3.3 dispatcher checks it before classifying) and is stripped
  as a safe env-var by permission matching, so rules still match.
- **Tracing**: `debug!` per decision with fields `command_prefix`, `outcome`,
  `latency_ms` (field names per `common/otel/CLAUDE.md` conventions), one
  `info!` at session scope for detect results.

---

## 7. Expected gains & how we will measure them (统计数据)

### 7.1 rtk's published numbers (self-reported; tokens ≈ chars/4)

Per-ecosystem reductions: git 85–99 %, JS/TS 70–99 %, Python 70–90 %,
Go 75–90 %, Rust 60–99 %, cloud 60–80 %. Their 30-minute Claude Code session
model: **~118 k tokens of command output → ~23.9 k (−80 %)**, dominated by
`cat/read` (40 k→12 k), `cargo/npm test` (25 k→2.5 k), `grep` (16 k→3.2 k),
`git diff` (10 k→2.5 k). Overhead: 5–15 ms per command, ~4 MB binary.

Caveats we accept openly: numbers come from rtk's own README/architecture
docs, use the chars/4 heuristic, and include operations (cat/grep/find) that
in coco-rs flow through built-in tools and therefore **won't** see rtk. The
never-worse guard bounds the downside at ~0.

### 7.2 Projection for coco-rs (Bash-only surface)

Illustrative agentic coding session, 40 Bash calls:

| Class | Calls | Raw avg | With rtk | Saved |
|---|---|---|---|---|
| git status/diff/log | 15 | ~700 tok | ~150 tok | ~8 k |
| git add/commit/push | 8 | ~200 tok | ~15 tok | ~1.5 k |
| cargo test / pytest (hits 30 k cap) | 5 | ~7 500 tok | ~500 tok | ~35 k |
| cargo build / clippy / lint | 6 | ~1 500 tok | ~300 tok | ~7 k |
| docker / misc | 6 | ~400 tok | ~100 tok | ~1.8 k |
| **Total** | 40 | **~66 k** | **~13 k** | **~53 k (−80 %)** |

Second-order effects are likely worth as much as the direct savings:

- **Quality, not just size**: today the 30 k head cap silently deletes test
  *failures*; failure-focus keeps them. Fewer re-runs of the same command.
- **Compaction pressure**: smaller tool results → later auto-compact
  threshold crossings → fewer micro-compacts (`services/compact/src/micro.rs`
  clears old Bash results wholesale) and fewer prompt-cache-breaking history
  rewrites.
- **Level-1 persistence**: Bash results under the 30 k persistence bound stop
  being swapped out to `<persisted-output>` previews as often.

### 7.3 Native measurement (do not trust chars/4)

coco-rs measures its own gains, in real tokens-adjacent units:

- `coco.rtk.engine_total{tier="builtin"|"external"|"skip", reason}` counter —
  per-tier hit rate + skip/passthrough reasons (§3.5 arbitration outcomes).
- `coco.rtk.rewrite_latency_ms` histogram — proves the ≤ 15 ms claim.
- `coco.tool.bash.result_chars{rtk="builtin"|"external"|"miss"|"off"}`
  histogram — the actual A/B: median/percentile Bash result size per tier,
  from the existing `render_for_model` output length. Session-level input
  token deltas fall out of the existing cost tracking in `core/messages`.
- rtk's own ledger stays available: a follow-up `/rtk` command can render
  `rtk gain --project --format json` (per-command savings, top offenders)
  without coco re-implementing analytics.

Since RTK ships `Stable` / default-on (§5.1), these metrics are **post-hoc
validation and a rollback signal**, not a promotion gate: expect ≥ 4 weeks of
p50 Bash-result shrink ≥ 50 % with no regression in command-retry rate; if the
shrink underperforms or retries regress, flip `default_enabled` to `false`.

---

## 8. Testing

- `detect.test.rs` — version parsing, `which` fallback, config override,
  OnceCell single-probe behavior (no env mutation; inject a fake `$PATH` dir
  via the `utils/cargo-bin` harness stub binary).
- `rewrite.test.rs` — exit-code protocol mapping (0/1/2/3 → outcome), empty
  stdout, timeout kill, spawn failure; stub `rtk` is a tiny test binary that
  echoes canned responses per argv.
- `bash.rs` integration — skip matrix (background, sandbox, exclude list,
  feature off), envelope `rtkCommand` presence, permission rules still
  matching the original command (existing `shell_rules` tests extended with a
  rewritten-execution assertion).
- One `#[ignore]`-by-default e2e behind `which("rtk")` that runs
  `rtk rewrite "git status"` against a real install.

Phase 2 adds:

- `filter.test.rs` — post-exec stage: family dispatch, TOML filter
  application, never-worse rejection, `catch_unwind` degradation (a
  deliberately panicking fake adapter must yield raw output), ANSI-leak
  guard (assert no `\x1b[` in filtered output), `RTK_DISABLED=1` skip.
- `families.test.rs` — golden tests per family: captured fixture outputs
  (cargo-test failure, git status/diff/log, pytest) → expected compact
  text; regenerated on rev bumps.
- Parity harness (`#[ignore]`, needs a real binary): the same fixtures
  through the embedded core and through `rtk` — assert semantic
  equivalence, not byte equality (versions may legitimately drift).
- Trust flow — vendored store read/write round-trip against a temp data
  dir; consent-prompt glue unit-tested with a scripted approval; an
  untrusted project file must be skipped silently in headless mode.
- Tier arbitration — the §3.5 matrix as a table test over
  (engine policy × binary present × family fidelity × background/sandbox).

Companion-file layout per repo convention; no inline `mod tests`.

---

## 9. Phasing & future work

| Phase | Scope |
|---|---|
| **1 (subprocess tier)** | `Feature::Rtk`, `RtkConfig`, `exec/shell/src/rtk/` (incl. rr-rtk detection + fixup), BashTool insertion, metrics, tool-description line. File-level checklist: Appendix B. |
| 1.5 | `/rtk` slash command surfacing `rtk gain --project --format json`; `coco doctor` reports detect status; managed install (interim until phase 2 — `cargo install rr-rtk` or GitHub release download + SHA-256, LSP `Installer` precedent, incl. `rtk` symlink); **native formatting absorptions (§2.3/§2.4): grep grouped content + per-file cap, glob dir-grouping, files-mode counts, overflow markers** — independent of rtk, ship even if the feature never leaves Experimental. |
| **2 (the built-in tier, §3.3)** | Fork + `src/lib.rs` (+ `core`/`cli` feature split) consumed as a pinned git dep (§3.2 vehicle 1); post-exec filter stage in BashTool behind the same `Feature::Rtk`; `catch_unwind` + never-worse guard; shared filter/trust config (§3.4); `rtk.engine` tier arbitration defaulting to `builtin_first` (§3.5); upstream lib PR pursued in parallel (vehicle 2), switch to published crate when it lands. |
| 2.x (opt.) | Per-family argv flag-injection table (JSON-mode parity in-process); native `Read` "signatures-only" level via embedded `FilterLevel::Aggressive` (or reimplemented in `read_loader`); output-side secret redaction (`coco-secret-redact`) at the same post-exec seam — closes the gap flagged in §2.2. |
| Non-goals | PowerShell rewriting; routing Grep/Glob through rtk at runtime; mirroring rtk's filter config into coco settings (the embedded engine shares rtk's own files + trust store, §3.4); any `CLAUDE_*`/legacy env compatibility. |

## 10. Open questions

1. Does `rtk rewrite` need a `--` separator for commands starting with `-`?
   (Verify against clap's positional parsing during implementation.)
2. Exit 2 (`HostDeny`) fires off *Claude's* settings files if the user also
   has Claude Code installed — harmless (passthrough), but worth an upstream
   `--no-permission-check` flag so the verdict never consults a foreign host.
3. Should `TaskRuntime`'s running-task display show the rewritten command or
   the original + badge? (UX call at implementation time.)
4. Managed install default: prefer `cargo install rr-rtk` (compiles, slow,
   needs toolchain) or the GitHub release download (fast, needs checksum
   pinning maintenance)? Leaning release-download with `cargo` fallback.
5. File upstream (rr-rtk and/or rtk-ai/rtk): binary-name-aware rewrite
   prefixes (§4.5), a `--no-permission-check` flag for `rtk rewrite` (Q2),
   and the `[lib]` + `core`/`cli` feature-split PR (§3.2 vehicle 2).
6. **RESOLVED** (phase-2 v0). The fork's `src/lib.rs` exposes `core`
   (TOML filter registry + never-worse guard + ANSI/truncate/code-strip),
   `discover` (classify/rewrite), `hooks` (trust store) and `parser` — but
   **not `cmds`**: the family formatters stay coupled to `main.rs`'s binary-only
   `Commands` enum (`cmds::js::vitest_cmd::run_test`). So the git/cargo/pytest
   formatters are not `pub`-reachable yet; phase-2 v0 ships the TOML long-tail +
   guard, and the `cmds` decouple remains the trigger for the family formatters.
7. Where does the fork live — a `coco`-org repo, or contributed directly to
   rr-rtk's existing fork to avoid a third identity?

---

## Appendix A — source facts referenced

- coco-rs: `core/tools/src/tools/bash.rs` (`truncate_output` :1592,
  `render_for_model` :664, sandbox snapshot :829, dispatch :950/:1181);
  `exec/shell/src/provider/bash.rs` (`COCO_SHELL_PREFIX` wrap :189);
  `app/query/src/tool_call_preparer.rs` (permission-then-rewrite ordering
  :678/:1012); `core/permissions/src/shell_rules.rs` (`match_bash_rule`
  :388); `common/config/src/sections.rs` (`ShellConfig::resolve` pattern
  :532); `common/config/src/env.rs` (EnvKey pattern :194/:406);
  `services/lsp/src/config.rs::command_exists` :637 (which-probe precedent).
- rtk v0.42.4: `src/discover/registry.rs` (`rewrite_command` :561,
  `rewrite_compound` :599, segment rules :805), `src/discover/lexer.rs`
  (`contains_unattestable_construct` :279), `src/hooks/rewrite_cmd.rs`
  (exit-code contract :18), `src/hooks/hook_cmd.rs` (Claude payload :342),
  `src/core/guard.rs` (never-worse), `src/core/tracking.rs` (chars/4 :1284,
  schema :262), `src/cmds/system/search.rs` (grep grouping, `+N more`
  markers :488-562, 80-char truncation :686), `src/core/toml_filter.rs`
  (lookup chain :5-6, builtin blob :32, trust-gated load :188-221,
  SA-2025-RTK-002 project gate :676-683), `src/filters/README.md` (field
  table, 8-stage pipeline, lookup priority), `docs/contributing/ARCHITECTURE.md`
  (strategy taxonomy, overhead, savings tables), `CHANGELOG.md` (75 releases,
  ~weekly cadence 2026-03 → 2026-06).
- rr-rtk 0.42.3-rr.2 (published crate, inspected from the crates.io
  tarball): `Cargo.toml` (`[[bin]] name = "rr-rtk"`, no `[lib]` /
  `src/lib.rs` / `[features]`), `src/discover/registry.rs` (hardcoded
  `"rtk …"` rewrite prefixes :632/:818/:922), `src/hooks/constants.rs`
  (`CLAUDE_HOOK_COMMAND = "rtk hook claude"` :12).

## Appendix B — phase-1 implementation checklist

Every file phase 1 touches, in dependency order. Each row is independently
`just quick-check`-able; the whole set is one reviewable PR.

| # | Change | Where |
|---|---|---|
| 1 | `Feature::Rtk` variant + `FeatureSpec` row (§5.1) | `common/types/src/features.rs` + `features.test.rs` |
| 2 | `RtkEngine`, `PartialRtkSettings`, `RtkConfig` + `resolve(settings, env)` (§5.2, §3.5) | `common/config/src/sections.rs` + test |
| 3 | `RuntimeConfig.rtk` field + `RtkConfig::resolve` call in the builder | `common/config/src/runtime.rs` |
| 4 | `EnvKey::CocoRtkPath` (declaration + `as_str` arm) | `common/config/src/env.rs` |
| 5 | `rtk/` module: `RtkRewriter`, `RtkBinary`, detect probe, `rtk rewrite` call, exit-code map, rr-rtk fixup (§4.1, §4.5) | `exec/shell/src/rtk/{mod,detect,rewrite}.rs` + companion tests |
| 6 | `ToolUseContext.rtk: Option<Arc<RtkRewriter>>` + builder gate on `Feature::Rtk` (§5.1) | `core/tool-runtime/src/context.rs` |
| 7 | Rewrite call in `BashTool::execute` before the task/foreground dispatch fork; skip matrix; envelope `rtk` / `rtkCommand` fields (§4.2, §6) | `core/tools/src/tools/bash.rs` |
| 8 | Conditional Bash tool-description line (§6) | Bash tool description site, `core/tools` |
| 9 | `coco.rtk.*` metrics + tracing fields (§7.3) | `exec/shell/src/rtk/` emission, names per `common/otel` conventions |
| 10 | Register this doc in `docs/internal/CLAUDE.md` Document Map + File Index | docs |

## Appendix C — phase-2 (v0) implementation checklist

The embedded post-exec filter tier, as landed. Each row is independently
`just quick-check`-able.

| # | Change | Where |
|---|--------|-------|
| 1 | Coco-owned fork adds additive `src/lib.rs` (`pub mod core/discover/hooks/parser`); no upstream file touched | `collab-ai-dev/rtk` @ `8f2bc3d` |
| 2 | Fork delta: drop `serde_json` `preserve_order` so the lib does not force IndexMap key-ordering onto embedders (cargo unifies the feature graph-wide; coco relies on BTreeMap-sorted JSON) | `collab-ai-dev/rtk` @ `c788ea7` |
| 3 | Pin the fork as a git dep by `rev` (vehicle 1); bumped via `/rtk-sync` | workspace `Cargo.toml`, `exec/shell/Cargo.toml` |
| 4 | Post-exec stage: `apply_rtk_filter(command, exit_code, stdout) -> Option<String>`. Cheap command-string gate first (RTK_DISABLED strip → **skip compound/piped** commands → env-stripped `find_matching_filter` → **skip `filter_stderr` filters**), so the ~2 MB stdout is cloned only on a match; then `apply_filter` → self-contained byte-length never-worse guard (no coupling to rtk's return identity) → defensive `strip_ansi`, panic-isolated via the `spawn_blocking` join boundary; one decision metric | `exec/shell/src/rtk/filter.rs` + test |
| 5 | Extend `BashOutputRewriter` with backend-agnostic capability predicates (`does_pre_spawn_rewrite` / `does_post_exec_filter`) + `filter_output`; `RtkRewriter` projects `RtkMode` onto them | `exec/shell/src/rtk/mod.rs` + test |
| 6 | `RtkMode` becomes load-bearing (was inert): default `BuiltinFirst` now spawns unmodified + filters post-exec | `common/config/src/sections.rs` |
| 7 | Gate the pre-spawn rewrite on `does_pre_spawn_rewrite`; add `apply_post_exec_filter` (gated on capability + `!was_rewritten`, the no-double-filter rule) + `annotate_builtin_tier`; wire into BOTH exec paths before `decode_capped`, image-detection skipped when the filter fired | `core/tools/src/tools/bash_rtk.rs`, `bash.rs` + tests |
| 8 | `coco.output_rewrite.decision_total{engine=rtk,tier=builtin,reason}` for filtered / miss / never_worse / panic | `exec/shell/src/rtk/filter.rs` |

**Deferred (prerequisites not yet met), with the safe fallback each has today:**

- **`families.rs` (git/cargo/pytest formatters)** — blocked on the fork's `cmds`
  decouple (§10-Q6). Until then those commands post-exec-Miss and fall through to
  raw + head-truncation (today's behavior). No placeholder module is created
  (dead code); it lands with real content at the decoupling rev.
- **`trust.rs` (coco-native consent write flow)** — the registry loader already
  trust-gates project `.rtk/filters.toml` internally (silently skips untrusted =
  rtk's headless posture), so the read path is safe without it. The write flow is
  only meaningful once per-cwd project loading exists (the singleton reads the
  project file from process CWD once); deferred with the CWD-singleton skew.
- **JSON-mode arg(§2.x flag-injection), native `Read` `FilterLevel::Aggressive`,
  output-side secret redaction** — unchanged from §9.
