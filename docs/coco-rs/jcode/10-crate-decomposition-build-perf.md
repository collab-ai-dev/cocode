# Crate Decomposition, Modularity & Build Performance: jcode vs coco-rs

This module compares the two harnesses on **how the code is physically
partitioned into compilation units**, what mechanically enforces those
boundaries, and what each project does to keep warm rebuilds fast. Build
*economy* — what recompiles when you edit a file — is a structural property of
the crate graph, so decomposition and build performance are one topic.

All claims below were re-verified against source on both sides; file:line
references are to the actual trees (`/lyz/codespace/3rd/jcode` and
`/lyz/codespace/codex/coco-rs`). Marketing framing from either README was
treated as unverified until checked.

---

## jcode approach

### A modular *monolith* with a workspace shell

jcode declares 50 workspace members plus the root `jcode` package
(`Cargo.toml:8-61`), but the decomposition is heavily skewed toward the root
crate:

- Root `src/` = **336,355 LoC across 646 files** (`find src -name '*.rs'`),
  with `src/lib.rs` declaring **76 `mod` items** all compiled in one crate.
- All `crates/*` combined = **114,188 LoC**, of which `jcode-desktop` alone is
  **65,994** (a separate desktop product surface). Excluding desktop, only
  ~48K LoC has been extracted into crates vs 336K in the root — **~87% of
  non-desktop code lives in a single crate.**

jcode's own `MODULAR_ARCHITECTURE_RFC.md:31-36` is candid: "Today, jcode is
best described as a **modular monolith with a growing workspace shell**... The
root `jcode` crate still owns most runtime orchestration."
`MODULAR_ARCHITECTURE_RFC.md:84-96` enumerates what the root still owns: CLI,
server orchestration, session state, agent turn loop, provider impl
composition, protocol/message/config types, tool registry, TUI app + rendering,
auth, memory, safety, ambient. The largest root files all recompile together
on any root edit (verified via `wc -l`):

| File | LoC |
|---|---|
| `src/server/client_lifecycle.rs` | 2837 |
| `src/tui/app/commands.rs` | 2823 |
| `src/tui/app/input.rs` | 2677 |
| `src/tui/app/inline_interactive.rs` | 2621 |
| `src/tui/ui.rs` | 2519 |
| `src/provider/anthropic.rs` | 2478 |
| `src/auth/lifecycle_driver.rs` | 2262 |

### Type-seam crates (the good part)

jcode HAS extracted a family of clean `*-types` data-contract crates. Verified
dependency edges:

- `jcode-message-types` → serde, serde_json, chrono only.
- `jcode-session-types` → serde, chrono, jcode-message-types only.
- `jcode-config-types`, `jcode-side-panel-types` → serde only.
- `jcode-core` → chrono, rand, libc, serde only — genuinely cheap.

`jcode-tool-core` deliberately pulls `tokio` (sync feature) because it carries
the `Tool` trait + execution context — a runtime-contract crate, not a pure
type crate. This matches the RFC's contract-vs-runtime distinction. The verifier
also found jcode actively *practices* DTO-seam extraction to shrink the root's
rebuild surface — there are 14 `*-types` crates
(config/session/message/tool/task/side-panel/usage/auth/memory/ambient/batch/gateway/selfdev/background),
with documented seam moves (e.g. `ToolDefinition`→`jcode-message-types`,
Provider trait→`jcode-provider-core`) validated by warm benchmark.

### Dependency-boundary guard — present but NOT in CI

`scripts/check_dependency_boundaries.py` parses `cargo metadata --no-deps` and
blocks `jcode-*-types` crates from depending on root/runtime/UI/provider crates
(`FORBIDDEN_INTERNAL_DEPS`, lines 28-51); it even forbids `jcode-core` as a
"backdoor catch-all," and separates hard errors from warnings (lines 95-99).
**But `grep -rn check_dependency_boundaries .github/` returns nothing** — it is
not invoked by any workflow, only referenced in
`docs/CRATE_OWNERSHIP_BOUNDARIES.md`. It is an advisory, manually-run guard
scoped to the ~14 `*-types` crates. Critically, its scope is **narrow**: it
checks only that DTO crates don't depend upward — it is *not* a general
all-layers rank check.

### Compile-perf machinery (a serious, measured program)

`COMPILE_PERFORMANCE_PLAN.md` is a real engineering program, not a wish list:

- `scripts/dev_cargo.sh` — sccache-if-available, clang+lld linker, a dedicated
  `selfdev` profile (`Cargo.toml:257-259`, `opt-level=0` inheriting release).
- `scripts/bench_compile.sh` — verified flags: `--touch <path>` (touch a file
  before each timed run to *simulate an edit*), `--edit <path>` (toggle a
  restored edit), `--runs <n>` (min/median/avg/max), `--json`, and
  `-- <extra cargo args>` (lines 20-32). This produced the per-file warm
  rebuild table at `COMPILE_PERFORMANCE_PLAN.md:212-222`.
- `scripts/bench_selfdev_checkpoints.sh` — bundles cold + warm check+build
  checkpoints.
- The root `.cargo/config.toml` is intentionally minimal (`jobs = 6`, no
  hardcoded wrapper/linker); sccache/linker are opt-in via the wrapper.

### Critical: jcode CANNOT use sccache on its hot path

`COMPILE_PERFORMANCE_PLAN.md:170-178` documents that the adaptive low-memory
selfdev path "**disables `sccache` by default because `sccache` rejects Cargo
incremental builds**" and relies on `CARGO_INCREMENTAL=1` +
`CARGO_PROFILE_SELFDEV_CODEGEN_UNITS=256`. So jcode's fast path is Rust's
**intra-crate** incremental compilation (per-crate-internal, fragile) with 256
codegen units to limit recompilation *within* the 336K-LoC monolith — and the
docs record earlyoom killing `rustc` at 2.7-3.3 GiB RSS on a 16 GiB box
precisely because the root crate is so large. This is fundamentally weaker than
a clean crate boundary, and jcode's own plan
(`COMPILE_PERFORMANCE_PLAN.md:43-46`) ranks "Workspace / crate boundaries" #1
("Rust caches best at the crate boundary").

The `dev_cargo.sh` path is memory-adaptive: it auto-sets `CARGO_INCREMENTAL=1`
+ `codegen-units=256` + disables sccache under earlyoom / <24 GiB RAM. coco-rs
by contrast has a single static `incremental=false` config with no
memory-adaptive fallback.

### Heavy deps are NOT default-off

`Cargo.toml:227` ships `default = ["pdf", "embeddings"]`.
`COMPILE_PERFORMANCE_PLAN.md:279-286` shows they made embeddings opt-in
(2026-05-05) then **reverted** (2026-05-23) so the ONNX/tokenizer memory recall
"works out of the box." Every default build therefore compiles the heavy
`jcode-embedding` (ONNX) stack. They mitigate with feature profiles for probes
(below), but the *default* remains heavy.

### Ergonomic dev feature-profile selector

`JCODE_DEV_FEATURE_PROFILE={default,minimal,pdf,embeddings,full}` is injected by
`dev_cargo.sh` (validation at lines 126-131), cutting the root dep tree from
~3740 lines (defaults) to ~1106 (none). Dev/probe binaries are gated:
`Cargo.toml:6` sets `autobins = false`, lines 82/87/92 put
`required-features = ["dev-bins"]` on `tui_bench` / `session_memory_bench` /
`mermaid_side_panel_probe`, and line 228 declares `dev-bins = []` — so
`--all-targets` inner loops skip probe binaries.

### CI ratchets (a real strength — a whole budget family)

`.github/workflows/ci.yml:55-69` runs five ratcheting guards on every PR
(verified):

- `check_warning_budget.sh` — zero-warning ratchet (`warning_budget.txt` = `0`).
- `check_code_size_budget.py` — oversized **production** file ratchet
  (`DEFAULT_THRESHOLD = 1200`, baseline `code_size_budget.json` tracks 55
  oversized files, **46 in root `src/`**).
- `check_test_size_budget.py` — oversized **test** file ratchet
  (`threshold_loc: 1200`).
- `check_panic_budget.py` — unwrap/panic ratchet (`panic_budget.json`
  total = 21).
- `check_swallowed_error_budget.py` — swallowed-error ratchet
  (`swallowed_error_budget.json` total = 2289, broken out by
  `dot_ok`/`let_underscore`/`unwrap_or_default`).

There is also `scripts/check_startup_budget.sh`, a runtime startup-time ratchet
(runs `bench_startup.py --check`) tied to the TTFF perf claims. These guards
mechanically prevent the monolith — and the panic/error/warning debt — from
getting worse.

### README build-speed claim verification

The "blazing-fast" framing is *forward-looking* for builds. `README.md:554`
states plainly: "An incremental debug cargo build with cache enabled takes
about **1 minute** on my machine. **The goal is 5-20 seconds.** Refactors and
crates seams should be able to make this happen." So the build-speed claim is an
**explicitly-unachieved aspiration**, not a measured result. Warm touched-file
`cargo check` measured 7-14s and warm selfdev builds 12-31s
(`COMPILE_PERFORMANCE_PLAN.md:212-222`), with a no-op/warmup check at ~65s. (The
14ms-TTFF / 1000fps claims are *runtime*, out of scope for this module; the
*build-speed* marketing is candidly unmet in jcode's own docs.)

---

## coco-rs approach

### An already-decomposed layered workspace

`Cargo.toml:1-110` declares ~80 first-party crates (the verified member list
includes 8 strict layers plus `coordinator`, `hub/*`, `bridge`, `retrieval`,
`tests/*`). There is no monolith. Total non-test production code is large
(`find ... ! -name '*.test.rs' ...` ≈ 471K LoC including build artifacts), but
the **largest single source crate is `app/tui` at 39,361 LoC (~9%)**, then
`retrieval` 31,673, `app/cli` 28,303, `core/tools` 21,168, `app/query` 21,030.
**No crate dominates the compile graph.**

### Enforced layering

The root `CLAUDE.md` codifies "Lower layers depend on nothing above them."
Verified concretely:

- `common/types/Cargo.toml` (foundation) depends only on `coco-llm-types` (the
  DTO seam) — nothing from core/app.
- No `utils/*` crate depends on any core/app crate (scanned all utils
  Cargo.toml; zero upward violations).
- `app/cli/src/main.rs` is the composition root wiring dozens of internal
  crates.

This is exactly the architecture jcode's RFC is *planning*. jcode's
`MODULAR_ARCHITECTURE_RFC.md` Phases 2-6 (extract jcode-core, then
jcode-provider/agent/server/session, then jcode-tui, then shrink root to a
composition shell) are all future work; coco-rs's `app/cli` is *already* the
thin composition shell, provider runtime is *already* `services/inference` +
`vercel-ai/*`, the agent loop is `app/query`, session is `app/session`, TUI is
`app/tui`. **The RFC's "target architecture" is roughly coco-rs's *current*
architecture.**

### The dual-seam discipline (broader than jcode's, and in CI)

coco-rs isolates the *entire Vercel AI SDK* behind two crates.
`scripts/check-vercel-ai-seam.sh` enforces that ONLY `common/llm-types` (DTO
seam) and `services/inference` (runtime seam) may declare a `vercel-ai*`
dependency; every other crate reaches SDK types via `coco_llm_types::*` /
`coco_inference::*` aliases. SDK version bumps edit exactly two crates. **This
guard IS wired in** (`justfile:93,106,122` — `check-seam` runs in both
`quick-check` and `pre-commit`) plus the git pre-commit hook
(`scripts/git-hooks/pre-commit`) and the Stop hook.

### Error-tier seam

`scripts/check-error-policy.sh` enforces a 3-tier error policy across all
crates (Tier 2 libraries `utils/*` + `vercel-ai/*` may not return
`anyhow::Result` or depend on `coco-error`/`snafu`; Tier 3 trunk must use
`coco-error`). It uses a *shrinking* `error-policy-allowlist.txt` —
grandfathered violations that fail if they grow OR if a no-longer-violation
stays listed. Wired into `justfile:100,106,122` (`quick-check` / `pre-commit`).

### Test organization keeps prod compile units lean

Policy mandates companion `.test.rs` files via
`#[path="x.test.rs"] mod tests;`. Verified: **1028 `.test.rs` companion files**,
only **6** inline `#[cfg(test)] mod tests {` stragglers. Combined with the
`[profile.ci-test]` / `profile.test` separation, dev-dep-linked test binaries
don't pollute the `cargo check` / clippy `.rmeta` path — and CLAUDE.md
explicitly reasons about `.rmeta` vs test-cfg-ELF cache non-sharing.

### Incremental strategy: crate-boundary cache via sccache

`.cargo/config.toml` sets `rustc-wrapper = "sccache"` (line 2,
`SCCACHE_CACHE_SIZE = "40G"`, `SCCACHE_IDLE_TIMEOUT = "0"`, lines 5-7) AND
`incremental = false` for dev/test/ci/ci-test profiles (lines 9,12,15,18) —
because sccache rejects incremental. This works *precisely because* coco-rs has
fine-grained crates: the **crate is the cache unit**, so an unchanged crate is a
clean sccache hit. `mold` is also configured as the Linux linker (lines 34-38).
(Note: root `CLAUDE.md:49-53` documents only mold and never mentions sccache or
why incremental is off — a real doc drift; see recommendation M10-S6.)

### Incremental lint closure

`.claude/scripts/clippy.sh` (`just clippy-affected`) lints only
`{changed crates ∪ transitive reverse-dep closure}`, falling back to full only
when Cargo.toml/Cargo.lock/toolchain change or affected ≥70% of workspace. A
leaf-crate edit re-lints just that crate and its dependents. Wired into
quick-check, the Stop hook, and the git pre-commit hook.

### Heavy deps default-off (mostly)

`retrieval/Cargo.toml`: `fastembed` (ONNX runtime) is `optional = true`
(line 85); `[features] default = []` (line 111) with
`neural-reranker = ["fastembed"]` (113) / `local-embeddings = ["fastembed"]`
(114) / `local` (117) gating it. The whole BM25/tree-sitter/petgraph/tiktoken
retrieval stack lives in ONE isolated leaf crate (`coco-retrieval`) depended on
only by the workspace root, and the heaviest piece (ONNX) is further off by
default. coco-rs thus *wins* the heavy-leaf-isolation goal jcode wrote down and
then walked back. **Caveat (verified):** the chunking deps — `tree-sitter` + 5
grammars (rust/go/python/java/typescript, lines 37-47), `tiktoken-rs` (line 34),
and `rusqlite` (line 26) — are **unconditional** in `coco-retrieval`. They are
still crate-isolated (only the root depends on retrieval), but they are *not*
feature-gated within that crate; see recommendation M10-S4.

### Build profiles

`Cargo.toml` sets `profile.dev.package."*"` opt-level=1 (deps optimized, local
crates fast), release `lto=thin` + `codegen-units=1` + strip, plus dedicated
`ci` / `ci-test` profiles. Workspace-wide clippy lints are denied in
`[workspace.lints.clippy]`.

### The one structural hole: no live quality CI for coco-rs

The single most important decomposition-discipline gap is *not* in the crate
graph — it's in CI coverage. **coco-rs has NO live GitHub quality CI.** Verified
by scanning `.github/workflows/`:

- `ci.yml` verifies **codex-rs** Cargo manifests and the codex npm package.
- `rust-ci.yml` gates on `codex-rs/*` paths and runs `working-directory:
  codex-rs`.
- `rust-ci-full.yml`, `rust-ci-full-nextest-platform.yml`, `cargo-deny.yml`,
  `blob-size-policy.yml` all target `codex-rs/`.
- **Only `coco-release.yml` touches `coco-rs/`** — and it only builds release
  artifacts.

So coco-rs's architectural invariants (vercel-ai seam, error tiers,
clippy-affected) are enforced by `just quick-check`, the git pre-commit hook,
and the Stop hook — i.e. **locally, on the honor system**, not by a blocking PR
gate. jcode by contrast gates every PR via `ci.yml:55-69`. This is the
precondition for the budget-guard recommendations below: every "wire into CI"
item must target `just`/hooks/`.claude/scripts`, *not* a (nonexistent) coco
GitHub workflow.

---

## Head-to-head comparison

The decisive structural difference is the **crate-boundary cache unit, and
coco-rs has already won it.** Both projects' design docs agree on the principle
(jcode's `COMPILE_PERFORMANCE_PLAN.md:43-46` and `CRATE_OWNERSHIP_BOUNDARIES.md`
are built around "Rust caches best at the crate boundary"); the difference is
execution state.

| Dimension | jcode | coco-rs |
|---|---|---|
| Largest compile unit | root `src/` **336,355 LoC / 646 files / 76 modules** | `app/tui` **39,361 LoC** (~9%) |
| % code in one crate | ~87% of non-desktop in root | none dominant; ~80 crates |
| Incremental mechanism | intra-crate `incremental=true` + 256 codegen-units; sccache **impossible** on the monolith hot path | crate-boundary **sccache**, `incremental=false` |
| Edit blast radius | touching any of 76 root modules invalidates the whole root crate's front-end + codegen | leaf edit → that crate + reverse-dep closure (sccache-hit on the rest) |
| Heavy ML dep (ONNX) | `default = ["pdf","embeddings"]` — compiled in **every** default build | `fastembed` optional, `default = []` — off by default |
| Layering enforcement | DTO-only guard, **advisory** (not in CI) | vercel-ai seam + 3-tier error policy, **enforced** locally (quick-check / pre-commit / git hook / Stop hook) |
| Quality CI gate | YES — size/test/panic/swallowed-error/warning/startup ratchets on every PR | **NO coco quality CI** — invariants enforced only locally |
| Compile benchmark harness | `bench_compile.sh --touch/--edit/--runs/--json` + checkpoint bench | none |
| Memory-adaptive build | YES — auto-falls back to incremental + 256 CGU + sccache-off under low RAM | static `incremental=false` only |
| Test/prod compile separation | inline `#[cfg(test)]` (225 modules, 0 companions) | **1028 `.test.rs` companions**, 6 inline stragglers |

**Resource implication.** jcode's monolith forces a worse-of-both-worlds
compile economy on its hot path: it can't use sccache (incremental conflict), so
it depends on Rust's intra-crate incremental — which recompiles large fractions
of a 336K-LoC crate whenever a widely-used type or any of the 76 modules
changes, and can OOM `rustc` at 2.7-3.3 GiB
(`COMPILE_PERFORMANCE_PLAN.md:170-178`). coco-rs's fine-grained crates keep each
compile unit small enough that sccache hits are clean and per-crate rebuilds are
cheap — at the cost of more `Cargo.toml`s to maintain and cross-crate
re-export plumbing (the `coco_llm_types` / `coco_inference` aliases).

**Where jcode is genuinely better (mechanisms coco-rs lacks):**

1. **A whole *family* of ratcheting CI budgets** — size, test-size, panic,
   swallowed-error, warning, startup. These are portable, proven mechanisms.
   coco-rs has zero automated budgets.
2. **A touched-file compile-benchmark harness** (`bench_compile.sh`) that
   produces repeatable, edit-simulating warm-rebuild numbers — so a refactor can
   be *measured*, not asserted.
3. **An ergonomic dev feature-profile selector** + dev-bins gating, so inner
   loops skip heavy features/probe binaries with one env var.
4. **A memory-adaptive build path** that degrades gracefully on small machines.

None of these conflict with any coco-rs documented non-goal — they are tooling
and discipline, not architecture changes, and several are directly portable.

---

## Where coco-rs already matches or wins

1. **The decomposition jcode's RFCs are still *planning*, coco-rs has
   *shipped*.** jcode's "target architecture" (thin composition root + extracted
   provider/agent/server/session/tui crates) is precisely coco-rs's *current*
   layout (`app/cli` composition root; `services/inference` + `vercel-ai/*`;
   `app/query`; `app/session`; `app/tui`). jcode's own Executive Summary calls
   its present state a "modular monolith."

2. **Seam enforcement is broader AND actually run.** jcode's
   `check_dependency_boundaries.py` guards ~14 `*-types` crates and is **not
   wired into CI** (`grep` of `.github/` finds nothing; only a doc references
   it). coco-rs's `check-vercel-ai-seam.sh` + `check-error-policy.sh` guard
   architecture-wide invariants and run in `quick-check`, `pre-commit`, the git
   pre-commit hook, and the Stop hook. coco-rs isolates an *entire third-party
   SDK* behind 2 seam crates; jcode isolates only its own DTOs. (Caveat: jcode's
   guards run in a *blocking PR gate*; coco-rs's run only locally — see the CI
   gap above.)

3. **Heavy ML dependency is default-off in coco-rs, default-ON in jcode.** This
   is the exact optimization jcode attempted
   (`COMPILE_PERFORMANCE_PLAN.md:279-286`, embeddings opt-in) and then
   **reverted** (2026-05-23). jcode ships `default = ["pdf","embeddings"]`;
   coco-rs ships `default = []` with `fastembed` optional and the whole
   retrieval stack isolated in one leaf crate.

4. **No 336K-LoC invalidation hotspot.** Editing `app/tui` (coco-rs's largest,
   39K) doesn't touch `core/tools`, `services/inference`, or `vercel-ai/*`;
   editing any of jcode's 76 root modules invalidates the entire root front-end
   + codegen unit.

5. **Test/prod compile-unit separation.** coco-rs's 1028 `.test.rs` companion
   files (vs jcode's inline `#[cfg(test)]` modules, 0 companions) mean
   `cargo check`/clippy on production code doesn't drag dev-dependencies into
   the `.rmeta` path — and CLAUDE.md explicitly structures the workflow around
   this cache non-sharing.

**jcode claims that do not hold in source:**

- **"Blazing-fast build" is explicitly unmet in jcode's own README**
  (`README.md:554`: ~1 minute now, "goal is 5-20 seconds"). The build-speed
  marketing is aspirational.
- **The "50 workspace crates" headline overstates decomposition.** 50 members
  exist, but excluding the 65,994-LoC `jcode-desktop` surface, only ~48K LoC is
  actually in crates vs 336K in the root.
- **The dependency-boundary guard is not a CI gate** — drift can land
  unblocked.

---

## Optimization recommendations for coco-rs (adversarially verified)

Only suggestions whose adversarial verdict was **confirmed** or **nuanced**
appear here. For nuanced items the correction is folded in. All respect
coco-rs's documented non-goals (none of these touch compaction strategy,
provider-crate boundaries, or the dropped TS gates).

A precondition applies to every "enforce X" item: **coco-rs has no live
GitHub quality CI** (verified above — all rust quality workflows target
`codex-rs/`). Therefore every guard must be wired into `just quick-check`, the
git pre-commit hook (`scripts/git-hooks/pre-commit`), and/or the Stop hook /
`.claude/scripts` — *not* into a coco GitHub workflow, which does not exist.

### M10-S1 — Add a ratcheting file-size budget guard (port jcode's `check_code_size_budget.py`) — `[nuanced]`

**Why.** jcode mechanically prevents oversized-file regression: a baseline of 55
files >1200 LoC, existing tracked files may not grow, new oversized production
files are blocked, and `--update` only after intentional cleanup
(`scripts/check_code_size_budget.py:26-27,124-160`), wired into CI at
`ci.yml:55-57`. coco-rs has only an advisory CLAUDE.md guideline ("target <800
LoC; files >~1600 LoC: create a new module") with **no enforcement** — and
**36** production src files exceed 1200 LoC (verified, excluding `target/` and
test files), including `app/cli/src/tui_runner.rs` (5035),
`hooks/src/orchestration.rs` (3453), `app/cli/src/session_runtime.rs` (3240),
`app/query/src/engine.rs` (2905).

**Concrete change.** Add `coco-rs/scripts/check_code_size_budget.py` + a baseline
JSON, scoped to all crate `src/` dirs. Three corrections from adversarial
review:

- **(a) Wire into `just quick-check` + the Stop/git-pre-commit hooks, NOT
  "rust-ci.yml"** — that workflow is codex-rs's. There is no coco GitHub CI to
  target.
- **(b) Copy jcode's *full* production-file filter**
  (`check_code_size_budget.py:44-58`): exclude `*.test.rs` **and** `tests.rs`
  **and** `*/tests/*` **and** `*_test.rs` / `*_tests.rs` / `tests_*`. The naive
  "just exclude `*.test.rs`" filter miscounts the **3923-LoC**
  `app/cli/src/sdk_server/handlers/tests.rs` as production (it is *not* a
  companion file). With the naive filter the count is 37; with the full filter
  it is 36.
- **(c) Seed the baseline at the current 36 files** and allowlist
  `services/mcp-types/src/lib.rs` (1591, **auto-generated**) so a regenerate
  doesn't trip it.

**Impact: medium. Effort: low. Risk: low** (pure additive guard; the only
false-positive surface is generated files, mitigated by the allowlist).
**Non-goals: respected.**

### M10-S2 — Wire a cross-layer dependency-direction guard (rank-based, generalizing jcode's `check_dependency_boundaries.py`) — `[nuanced]`

**Why.** coco-rs's core invariant ("Lower layers depend on nothing above them")
is enforced only by the two narrow seam scripts (vercel-ai isolation + error
tiers). There is **no general guard** that, e.g., a `utils/*` crate didn't start
depending on a `core/*` crate, or that `common/types` stays free of upward deps.
The layering is currently clean (hand-verified: utils/* have zero upward deps;
`common/types` depends only on peer `coco-llm-types`) but nothing mechanically
prevents regression. jcode's `check_dependency_boundaries.py` demonstrates the
`cargo metadata`-driven boundary-check pattern (lines 54-62; error/warning split
95-99) — though note jcode's guard is *narrow* (DTO-crates-don't-depend-upward
only), so this is a generalization of its pattern, not a copy of its scope.

**Concrete change.** Add `coco-rs/scripts/check-layering.sh` (or `.py`) that
reads `cargo metadata`, assigns each crate a layer rank from its path, and fails
on any dependency to a strictly higher rank. Corrections from adversarial
review:

- **(a) Reuse the in-repo template.** `/lyz/codespace/codex/.github/scripts/verify_tui_core_boundary.py`
  already parses Cargo.toml + `cargo metadata` to enforce a `codex-tui!→codex-core`
  boundary — a ready, in-repo pattern (jcode's is an external reference).
- **(b) Explicitly rank the crates missing from CLAUDE.md's layer table.**
  `coco-coordinator` (verified) depends on `coco-tool-runtime`,
  `coco-permissions`, `coco-tasks`, `coco-hooks`, `coco-memory`, `coco-context`,
  `coco-compact`, `coco-messages` and is consumed by `app/cli` — so it must sit
  between root-modules (rank 4) and app (rank 5). Same for `hub/{protocol,
  connector,server}` and `standalone/{bridge,retrieval}`. Encoding these
  exceptions (plus same-layer allowances and `app/cli` composing everything) is
  the bulk of the effort.
- **(c) Wire into `just quick-check`** next to `check-seam`; start in warn-only
  mode (mirroring jcode's error/warning split) then promote to hard-fail.

**Impact: medium. Effort: low. Risk: low-medium** (the exception encoding is the
risk). **Non-goals: respected** — this *enforces* the documented layering, it
doesn't change it.

### M10-S3 — Add a touched-file compile-benchmark harness (port `bench_compile.sh --touch/--runs/--json`) — `[confirmed]`

**Why.** jcode can answer "did this refactor improve warm rebuild time for
editing crate X?" via `scripts/bench_compile.sh` (`--touch <path>`, `--edit`,
`--runs <n>` with min/median/avg/max, `--json`, `-- <extra args>`) +
`bench_selfdev_checkpoints.sh`; this produced the per-file table at
`COMPILE_PERFORMANCE_PLAN.md:212-222` and the steady-state caveat at ~line 389
("treat the rerun as the comparable steady-state datapoint"). coco-rs has
`just quick-check`/`pre-commit`/`test-crate` but **no touched-file timing
harness** — its compile-economy notes in CLAUDE.md are reasoned about, never
measured.

**Concrete change.** Add `coco-rs/scripts/bench-compile.sh` wrapping
`cargo check -p <crate>` and `cargo build` with `--touch` (touch a file, then
time), `--runs` with summary stats, and `--json`; add a `just bench-compile
<crate>` recipe. Because coco-rs uses sccache, **also report `sccache
--show-stats` hit/miss deltas per run** to validate crate-boundary cache
effectiveness — and discard the first post-clean run as warm-up noise (jcode hit
the same issue).

**Impact: medium. Effort: low. Risk: low** (pure tooling; only caveat is
sccache warm-up noise). **Non-goals: respected.**

### M10-S4 — Gate the always-on retrieval chunking deps + add an ergonomic lean-build recipe — `[nuanced]`

**Why.** The analyst's headline framing ("fastembed already isolated, mostly
ergonomics") is only half right. Verified: `retrieval/Cargo.toml` has
`fastembed` optional + `default = []` (lines 85,111) — so the *neural-reranker /
local-embeddings* ONNX path is correctly off by default, and "developers
iterating on app/tui pay for the neural-reranker" is **wrong**. **But** the
chunking stack is unconditional: `tree-sitter` + 5 grammars
(rust/go/python/java/typescript, lines 37-47), `tiktoken-rs` (line 34), and
`rusqlite` (line 26) are non-optional deps of `coco-retrieval`. They are
crate-isolated (only the root depends on retrieval), yet anyone whose build
graph pulls in `coco-retrieval` compiles all of them.

**Concrete change (reframed per adversarial review).** The real lever, not the
recipe:

- **(a) Audit whether `coco-retrieval`'s tree-sitter grammars + `tiktoken-rs` +
  `rusqlite` can move behind a feature** so a build graph that doesn't exercise
  retrieval chunking skips them — and confirm Cargo feature unification doesn't
  already force them via another path. jcode's model is the template: heavy deps
  behind features + dev-bins gated via `autobins = false` +
  `required-features = ["dev-bins"]` (`Cargo.toml:6,82,87,92,228`).
- **(b) A `just check-min` / `just check-crate <name>` ergonomic recipe**
  (passing `--no-default-features` / scoping to one crate) is a nice-to-have but
  *low* value, since the default build already doesn't pull `fastembed`. If
  added, keep the `--no-default-features` path tested so it can't silently break
  (jcode hit exactly this, `COMPILE_PERFORMANCE_PLAN.md:291-294`).

**Impact: low. Effort: low. Risk: low.** **Non-goals: respected** — coco-rs's
heavy-dep isolation is *already better* than jcode's; this is incremental.

### M10-S5 — Split the largest files (>2500 LoC), but for the right reasons — `[nuanced]`

**Why.** coco-rs's own CLAUDE.md says "Files > ~1600 LoC: create a new module,"
yet `app/cli/src/tui_runner.rs` (5035), `hooks/src/orchestration.rs` (3453),
`app/cli/src/session_runtime.rs` (3240), and `app/query/src/engine.rs` (2905)
all violate it (sizes verified). jcode treats oversized files as invalidation
units (`COMPILE_PERFORMANCE_PLAN.md`; the size-budget ratchet operationalizes
it; the RFC split-readiness checklist `MODULAR_ARCHITECTURE_RFC.md:444-455` is
the method).

**Critical correction from adversarial review — the *mechanism* claim is wrong
for coco-rs's config.** The analyst said "an unrelated edit forces recompiling
thousands of lines in one **codegen unit**." With `.cargo/config.toml`
`incremental = false` (lines 9,12,15,18), editing **any** line in a crate
recompiles the **entire crate's front-end** (parse / typeck / MIR) regardless of
module split — module boundaries do **not** bound front-end invalidation when
incremental is off. And dev `codegen-units` default to 256 (only release pins
`codegen-units = 1`), so back-end codegen is already split 256 ways; an
intra-crate module split buys little there either.

**Concrete change.** Split the >2500-LoC offenders for the *documented* reasons —
**the project's own CLAUDE.md size rule, readability, and merge-conflict surface
on hot files** — NOT for incremental-rebuild wins, which intra-crate splits do
not deliver under `incremental=false`. **If compile time is the actual goal,
extract a true sub-*crate*** (e.g. pull a cohesive slice of
`app/cli/tui_runner.rs` into a `coco-tui-runtime` crate) so it becomes a
separate compilation/sccache unit. Use jcode's split-readiness checklist
(`MODULAR_ARCHITECTURE_RFC.md:444-455`) to choose the seam, and pair with
M10-S1's ratchet to lock the gains. (Note: the analyst's prioritization "because
app/cli is what many CI jobs build" is also weakened — there is no coco CI
building it.)

**Impact: low. Effort: medium. Risk: medium** (mechanical churn for modest
compile payoff; prioritize readability/conflict-surface, not speed). **Non-goals:
respected** — but heed CLAUDE.md's own warning against single-use helper
extraction; don't over-split.

### M10-S6 — Reconcile the mold-vs-sccache documentation drift in root CLAUDE.md — `[confirmed]`

**Why.** Confirmed exactly: `CLAUDE.md:49-53` ("Linker speedup (already
configured)") documents **only** `mold` ("sets mold as the Linux linker...
Install once with `apt install mold`") and never mentions sccache or
incremental (grep returns nothing). Meanwhile `.cargo/config.toml:2` sets
`rustc-wrapper = "sccache"`, lines 5-7 set `SCCACHE_CACHE_SIZE = "40G"` /
`SCCACHE_IDLE_TIMEOUT = "0"`, and lines 8-18 set `incremental = false` for
dev/test/ci/ci-test. A contributor reading CLAUDE.md installs mold but **not**
sccache — missing the dominant cross-crate cache — and has no explanation for
why incremental is disabled (sccache rejects incremental, the same constraint
jcode documents at `COMPILE_PERFORMANCE_PLAN.md:170-178`).

**Concrete change.** Edit `/lyz/codespace/codex/CLAUDE.md:49-53` (there is no
separate `coco-rs/CLAUDE.md`). Rename "Linker speedup" → "Build cache + linker"
and document **both**: sccache as `rustc-wrapper` (install `sccache`, 40G cache)
as the **primary** cross-crate cache and the **reason** `incremental = false`
(sccache rejects incremental — same tradeoff jcode documents), then mold as the
linker. One paragraph, mirroring jcode's clear explanation.

**Impact: low. Effort: low. Risk: none** (docs-only; prevents accidentally
re-enabling incremental, which would silently disable sccache). **Non-goals:
respected.**

### M10-S7 — (from verifier missed findings) Port the rest of jcode's ratcheting budget family — `[additional, confirmed-by-verifier]`

**Why.** Beyond file-size (M10-S1), jcode runs four more ratchets coco-rs lacks
(`ci.yml:55-69`, verified):

- **`check_test_size_budget.py`** (oversized *test* files, threshold 1200) —
  directly relevant: coco-rs has a **4436-LoC** `app/query/src/engine.test.rs`
  and a **3923-LoC** non-companion `app/cli/src/sdk_server/handlers/tests.rs`,
  plus ~13 other test files >1200 LoC (verified).
- **`check_panic_budget.py`** — coco-rs *bans* `.unwrap()` in non-test code via
  CLAUDE.md but has **no automated count**; jcode tracks it (`panic_budget.json`
  total = 21).
- **`check_swallowed_error_budget.py`** — tracks `dot_ok` / `let_underscore` /
  `unwrap_or_default` (jcode total = 2289).
- **`check_warning_budget.sh`** — zero-warning ratchet (`warning_budget.txt`).

**Concrete change.** Port the test-size and panic ratchets first (highest signal
for coco-rs given the test-file outliers and the unwrap ban), wired into
`just quick-check` + hooks. Seed each baseline at current state so they only
ratchet down. The warning ratchet is partly covered already by
`[workspace.lints.clippy]` deny-list + `clippy-affected`, so it is lower
priority.

**Impact: medium. Effort: low-medium. Risk: low** (additive guards; same
generated-file allowlist caveat as M10-S1). **Non-goals: respected.**

> A **startup-time budget** (jcode's `check_startup_budget.sh`) is intentionally
> *not* recommended here: it is tied to jcode's TTFF perf claims, which are a
> runtime concern out of this module's scope, and coco-rs has not made a
> startup-latency commitment. If coco-rs later sets such a target, the pattern is
> available to copy.

---

## Rejected after adversarial review

No suggestion in the analyst set received a **refuted** verdict — all six were
**confirmed** or **nuanced**, and the nuanced corrections have been folded into
the recommendations above. For transparency, the *specific analyst sub-claims
that were checked and overturned* (and corrected in-place) are:

- **M10-S1 — "wire into rust-ci.yml" — overturned.** There is no coco-rs quality
  CI; `rust-ci.yml`/`ci.yml` target `codex-rs/`. Corrected to `just quick-check`
  + hooks.
- **M10-S1 — "just exclude `*.test.rs`" filter — overturned.** Misses the
  3923-LoC non-companion `tests.rs`; corrected to jcode's full filter
  (`tests.rs` / `*_test(s).rs` / `tests_*` / `*/tests/*`).
- **M10-S2 — "generalize to coco-rs's 8 layers" framing — narrowed.** jcode's
  guard is DTO-only, not an all-layers rank check; and the in-repo
  `verify_tui_core_boundary.py` is the better template than jcode's external
  script. Also added the missing ranks for `coco-coordinator`, `hub/*`,
  `standalone/*`.
- **M10-S4 — "fastembed already isolated, mostly ergonomics" headline —
  overturned.** The neural-reranker *is* off by default (analyst's worry was
  unfounded there), but `tree-sitter`+grammars / `tiktoken-rs` / `rusqlite` are
  *unconditional* in `coco-retrieval` — that gating is the real lever, not the
  recipe.
- **M10-S5 — "recompiling thousands of lines in one codegen unit" mechanism —
  overturned.** Under `incremental=false`, the whole crate front-end recompiles
  regardless of module split, and dev codegen is already 256-way. Reframed:
  split for readability / conflict-surface / the project's own rule, and extract
  a *sub-crate* if compile time is the goal.

Nothing here recommends reversing a coco-rs documented non-goal. The two
explicitly *not*-recommended items are a startup-time budget (runtime scope, no
stated commitment) and any change to the deliberate `coco_llm_types` /
`coco_inference` dual-seam (which is the very design that *gives* coco-rs its
clean crate-boundary cache and is therefore a strength, not a target).
