# Startup & Runtime Performance: jcode vs coco-rs

> Source-level comparison of two independent Rust coding-agent harnesses.
> All claims below were verified by reading the actual source on both
> sides; file:line citations are load-bearing. jcode lives at
> `/lyz/codespace/3rd/jcode`; coco-rs at `/lyz/codespace/codex/coco-rs`.
>
> **Framing reminder:** jcode is an independent, performance-obsessed
> lineage; coco-rs is a behavior-faithful port of Anthropic's Claude
> Code (TypeScript). A structural difference is *not* automatically a
> coco-rs deficiency — several jcode wins depend on a persistent-server
> architecture that coco-rs has explicitly deferred (the
> hub/connector/server "concurrent-app-server" work). Where a jcode
> mechanism conflicts with a documented coco-rs non-goal, that is stated
> plainly.

---

## jcode approach

**The headline numbers are an emergent property of a client/server
architecture, not of micro-optimized startup code.** The single most
important fact about jcode's runtime: the interactive TUI is a *thin
client that connects to a persistent, multi-session server over a Unix
socket*. The server (`Server` in `src/server.rs`) holds
`sessions: Arc<RwLock<HashMap<String, Arc<Mutex<Agent>>>>>`
(`src/server.rs:394`) — **N live agents inside ONE process** — plus a
"Shared MCP server pool (processes shared across sessions)"
(`mcp_pool: Arc<OnceCell<Arc<SharedMcpPool>>>`, `src/server.rs:429-430`).
Adding a session adds a HashMap entry plus a thin client, not a whole
new agent. This is the mechanism behind both the *14ms
time-to-first-frame* claim and the *~10MB extra RAM per added session*
claim.

**Startup data flow** (`src/main.rs` → `cli::startup::run` →
`dispatch::run_default_command` → `tui_launch::run_tui_client`):

1. **Allocator tuning runs as the literal first statement of `main()`.**
   `configure_system_allocator()` is called at `src/main.rs:50`, *before*
   the Tokio runtime is built at `src/main.rs:53-55` (correct ordering —
   it executes before any worker thread spawns). On glibc-without-jemalloc
   Linux it declares `extern fn mallopt` and calls
   `mallopt(M_ARENA_MAX, 4)` (`src/main.rs:30-44`, `M_ARENA_MAX = -8`,
   env-overridable via `JCODE_GLIBC_ARENA_MAX`), capping per-thread
   arenas to 4. With the optional `jemalloc` feature it instead exports
   `malloc_conf = "dirty_decay_ms:1000,muzzy_decay_ms:1000,narenas:4\0"`
   (`src/main.rs:18-19`). It then installs the rustls aws-lc provider and
   builds a multi-thread Tokio runtime.

2. **`cli::startup::run`** (`src/cli/startup.rs:12`) calls
   `startup_profile::init()` (`:13`) then sprinkles `startup_profile::mark(...)`
   checkpoints across the entire boot path: `panic_hook` (`:16`),
   `logging_init` (`:19`), `log_cleanup` (`:21`), `nofile_limit` (`:24`),
   `perm_harden` (`:27`), `perf_init` (`:30`), `telemetry_check` (`:34`),
   `args_parse` (`:49`). Two boot-path mechanisms stand out:
   - **`crate::platform::raise_nofile_limit_best_effort(8_192)`**
     (`src/cli/startup.rs:23`) raises the file-descriptor soft limit
     before the runtime — relevant under many-MCP / many-session fan-out.
   - **`perf::init_background()`** (`src/perf.rs:245-247`) spawns
     system-profile detection onto a *background `std::thread`* guarded by
     a `OnceLock`, so reading `/proc/loadavg`, `/proc/meminfo`,
     `/proc/cpuinfo`, and WSL/SSH/terminal detection never blocks boot.
     The update check is likewise spawned on a detached thread
     (`src/cli/startup.rs:80`, `:106`).

3. **Server presence check.** For the default (no-subcommand) path,
   `dispatch::run_default_command` (`src/cli/dispatch.rs:498`) calls
   `server_is_running()` (`:547`); if a server is already running it skips
   straight to `tui_launch::run_tui_client`. If not, it spawns one
   detached via `spawn_server` (`src/cli/dispatch.rs:585`). **A *cold*
   first launch is therefore not 14ms** — it must pay `spawn_server`.

4. **The thin client.** `tui_launch::run_tui_client`
   (`src/cli/tui_launch.rs:117`) enters the terminal via
   `init_tui_runtime()` (`:124`), constructs
   `App::new_for_remote_with_options` (`:162`), marks `pre_run_remote`
   then `startup_profile::report_to_log()` (`:173-174`), and calls
   `app.run_remote(terminal)` (`:176`). The expensive agent state
   (provider init, tools, memory graph, embeddings) lives in the
   already-warm *server*, not the client. The README's 14.0ms TTFF
   (vs Claude Code's 3436.9ms — "245.5× slower", `README.md:196`) is the
   thin client painting a "loading shell" against a warm server.

**`startup_profile`** (`src/startup_profile.rs`): a
`Mutex<Option<StartupProfile>>` storing `start: Instant` plus
`marks: Vec<(String, Instant)>`; `report()` (`:37-77`) renders a
per-phase table with from-start time, delta, % of total, and a `█` bar;
`report_to_log()` (`:79-84`) dumps it. Pure instrumentation, zero
runtime cost when not reported.

**`perf::PerformanceTier`** (`src/perf.rs`): `Full`/`Reduced`/`Minimal`
computed by `compute_tier` (`:306-356`) from load ratio, available
memory, SSH (forces `Minimal`, `:315`), WSL, and terminal family.
`TuiPerfPolicy` (`:75-86`) then derives `redraw_fps`, `animation_fps`,
`enable_decorative_animations`, `enable_focus_change`,
`enable_keyboard_enhancement`, `simplified_model_picker`, and
`linked_side_panel_refresh_interval` via `tui_policy_for` (`:180-243`).
Concrete caps verified in source: SSH → `Minimal` → 12fps
(`:220-225`); WSL → `redraw_fps.min(30)` + 500ms panel refresh
(`:197-200`); WSL + Windows-Terminal → `redraw_fps.min(20)`,
focus-change off, keyboard-enhancement off, simplified picker, 1000ms
panel (`:202-208`). Detection is environment-cheap (SSH via
`SSH_CONNECTION`/`SSH_TTY` `:271`; WSL via `/proc/version` `:364`;
terminal via `WT_SESSION`/`TERM_PROGRAM` `:374-393`). This makes the
renderer *degrade gracefully on weak hosts* rather than assuming a fast
terminal.

**Adaptive render loop**: the steady-state loop is event-driven and
recomputes its redraw cadence each iteration from the perf tier (deep
idle → ~1fps; active stream/spinner → `redraw_fps`). The **"1000+ fps"
README claim is single-frame *capability*** (a `render_frame` is
sub-millisecond), not a sustained loop rate — defensible but
marketing-flavored.

**Process identity** (`src/process_title.rs`): `set_title` (`:68-71`)
uses the `proctitle` crate AND `set_killall_process_name()` (`:73-82`),
which does `prctl(PR_SET_NAME, "jcode")` (15-char comm field) so
`killall jcode` works. Role-specific compact titles are real:
`jcode:s:<server>` (`:84-86`), `jcode:c:<session>` (`:101-104`),
`jcode:selfdev` (`:88-95`); `initial_title` picks per-subcommand
(`:119-130`). Set once during arg prep (`src/cli/startup.rs:66`).

**`process_memory`** (`src/process_memory.rs`): a full
self-observability layer — parses `/proc/self/status` +
`/proc/self/smaps_rollup` for RSS/PSS/anon/file/dirty/swap, keeps a ring
of snapshots, and (with jemalloc) exposes `purge_allocator()`
(`:200-231`, walks every initialized arena calling `arena.N.purge`) and
`set_allocator_decay_ms()` (`:233-273`) to *actively return idle RSS to
the OS* for long-lived sessions. This is what lets the README publish
accurate PSS figures.

**Lazy + idle-unloaded embedding** (`src/embedding.rs`): the ~87MB ONNX
model loads lazily on first `embed()`; `maybe_unload_if_idle`
(`:216-244`) drops it after idle AND calls `purge_allocator()`
(jemalloc, `:255`) or `malloc_trim(0)` (glibc, `:262-267`) to return
pages to the OS. The README's 27.8MB figure (`README.md:72`) is honestly
footnoted as the *embeddings-off* baseline; the full default build
(`default = ["pdf", "embeddings"]`, `Cargo.toml:227`) is much larger.

**`restart_snapshot`** (`src/restart_snapshot.rs`): persists
active/crashed session IDs and *re-launches them in new terminals* via
`spawn_resume_in_new_terminal` / `spawn_selfdev_in_new_terminal`
(`restore_snapshot`, `:184-203`). This is **session-continuity**, NOT a
process-memory snapshot/restore — it does not serialize/restore heap
state, so it gives no faster-than-cold process resume.

**Default build profile is tuned for the self-dev rebuild loop, not for
runtime.** `[profile.release]` is `opt-level=1, codegen-units=256,
incremental=true` (`Cargo.toml:255-259`); the optimized distribution
artifact is a *separate* `release-lto` profile (`:268-272`,
`lto="thin", codegen-units=16`).

---

## coco-rs approach

**Architecture: the default interactive TUI is a fully standalone,
single process.** Unlike jcode there is no persistent server the TUI
connects to — `tui_runner::run_tui` (`app/cli/src/tui_runner.rs:66`)
builds the *entire* agent runtime in-process before the first frame. The
hub (`hub/{protocol,connector,server}`) exists for event aggregation,
but the interactive path never opens a socket to it, and
`Commands::Daemon` prints `"Daemon mode is not yet fully implemented."`
(`app/cli/src/main.rs:157`). Each new session = a new `coco` process.

**Startup data flow** (`app/cli/src/main.rs`):

1. `#[tokio::main]` default **multi-thread** runtime
   (`app/cli/src/main.rs:28`); `Cli::parse()`; `tracing_init::install(&cli)`
   (`:35`). `tracing_init` is mode-aware: short subcommands resolve to
   `Mode::Skip` (no subscriber, clean stdout); TUI/Headless/SDK install a
   non-blocking rotating file appender. This is a genuine win — fast-exit
   subcommands pay nothing for logging. There is exactly one coarse
   anchor, `tracing::info!(target: "coco_cli::startup", … "coco entry")`
   (`:37-43`); **no per-phase timing.**

2. Default branch → `tui_runner::run_tui` (`main.rs:277`).

3. `run_tui` (`tui_runner.rs:66`) runs a **long synchronous bootstrap
   before any frame**:
   - `RuntimeReloader::spawn` (`:93`) — config hot-reload watcher; takes
     the initial `Arc<RuntimeConfig>` snapshot (settings.json layers +
     providers/models.json merged once).
   - `build_engine_resources` (`:129` → `session_bootstrap.rs:117`) —
     **synchronous and heavy**: `create_api_client`,
     `build_fallback_clients_for_role` (constructs the whole fallback
     client chain, `session_bootstrap.rs:131`), `ToolRegistry::new` +
     `register_all_tools` (`:134-135`), output-style resolution, and
     `build_system_prompt_for_model`, which calls
     `coco_context::discover_memory_files(cwd)` — a **synchronous
     filesystem walk from filesystem-root → cwd reading every
     CLAUDE.md/AGENTS.md plus recursive `@import` expansion**
     (`app/cli/src/headless.rs:286`; assembled in
     `core/context/src/prompt.rs:147`). It also builds the full command
     registry (builtins → extended → skills loaded from disk → plugin
     contributions).
   - `SessionManager::new` + `.create` (writes a session file,
     `tui_runner.rs:145-148`); a `spawn_blocking` cleanup of old sessions
     (correctly backgrounded, `:156-183`).
   - `SessionRuntime::build` (`:231`) — every per-session subsystem.
   - `install_session_late_binds` (`:273`) — task runtime, transcript
     store, agent-team wiring, fork dispatcher.
   - resume hydration (`:325`), **`fire_session_start_hooks("startup")`**
     (`:347`, can shell out to user hooks), git branch read
     (`coco_git::get_current_branch(&cwd)`, `:410`),
     `install_theme().await` (`:494`),
     `install_keybindings().await` (`:514`).
   - **Only then** `App::new(command_tx, notification_rx)` (`:364`).
     `App::new` takes *only the two channels* — it does NOT require the
     system prompt or registries (those are seeded post-construction at
     `tui_runner.rs:364-525`). `Tui::new()` is the first point that
     enters alt-screen + raw mode.
   - `app.run().await` (`:563`) → first `self.redraw()?`
     (`app/tui/src/app.rs:264`).

So coco-rs's *time-to-first-frame is gated behind config load + provider
client construction + a synchronous CLAUDE.md disk scan + skills/plugins/
commands disk loads + SessionStart hooks + theme/keybinding loads.* And
there is **no instrumentation**: a grep across `app/` for
`startup_profile`/`StartupProfile`/`time_to_first_frame`/`first_frame`/
`ttff` returns nothing.

**Render loop** (`app/tui/src/app.rs:257` + `frame_requester.rs`): a
`tokio::select!` loop multiplexing terminal events, `CoreEvent`s, file/
symbol search, hot-reload channels, and a coalesced `draw_rx`. Three
mechanisms make it idle-cheap:
- It **coalesces `CoreEvent`s** — after the first event it drains
  `notification_rx.try_recv()` in a loop (`app/tui/src/app.rs:298-303`),
  so a 100-token/sec stream produces one coalesced paint, not 100.
- `FrameRequester`/`FrameScheduler` (`frame_requester.rs:100-128`, ported
  from codex-rs) coalesces all redraw requests and `sleep`s for
  `ONE_YEAR` (`:101`) when nothing is pending — **idle frames cost
  literally nothing.**
- The 250ms tick is gated by `needs_tick()` (`app.rs:286`) so idle
  sessions don't wake every 250ms; the spinner self-schedules via
  `schedule_frame_in(SPINNER_TICK_INTERVAL)` only while a turn or stream
  is active (`app.rs:417-419`).

This part of coco-rs is already excellent and mechanism-equivalent to
jcode's adaptive loop.

**Allocator**: system allocator only — **no jemalloc/mimalloc, no
`mallopt`/arena tuning, no `malloc_trim`** anywhere in `app/`, `common/`,
or the workspace `Cargo.toml` (verified by grep — empty). On glibc Linux
this means default malloc behavior (up to `8×ncpu` arenas, page
retention) under the multi-thread Tokio runtime — a likely contributor
to higher steady RSS.

**Process identity**: no proctitle / `PR_SET_NAME` for the `coco`
binary. The only `prctl` uses in the workspace are `PR_SET_PDEATHSIG`
(`utils/pty`, `utils/sleep-inhibitor`) and `PR_SET_DUMPABLE`
(`exec/process-hardening`) — none sets the process name. `ps`/`kill` see
the generic `coco` exe name; a `coco ps` subcommand reads
session-registration JSON files instead (`app/cli/src/main.rs:160`).

**Memory observability**: no `/proc/self/smaps_rollup` PSS sampling and
no runtime memory side-panel — coco-rs cannot self-measure its own RSS/PSS.

**Release profile** (`Cargo.toml:453-457`): `lto="thin"`,
`codegen-units=1`, `strip="symbols"` — **runtime-optimized by default**,
stronger than jcode's build-speed-tuned default `release`.

---

## Head-to-head comparison

### 1. Time-to-first-frame — jcode genuinely wins, by architecture

jcode's 14ms is real but it is the *thin-client* number: the client
(`tui_launch.rs:117-176`) enters the terminal and paints a loading shell
while the heavy agent state lives in an already-warm server
(`server.rs:394`). coco-rs's first frame is gated behind the entire
in-process bootstrap in `run_tui` (config → `build_engine_resources` →
**synchronous `discover_memory_files` disk walk** (`headless.rs:286`) →
skills/plugins/commands → SessionStart hooks → theme/keybindings), and
only then enters alt-screen. The mechanism that makes jcode fast (a warm
shared server) is exactly what coco-rs's default path lacks. **This is
the single biggest legitimate gap** — but note it is partly structural
(requires the deferred concurrent-app-server). The *in-process* portion
(paint a loading shell first, defer the disk scans) is portable today;
see S2.

### 2. Per-session RAM scaling — jcode wins decisively

jcode keeps N agents as HashMap entries in one process with a *shared
MCP process pool* (`server.rs:394,429-430`). coco-rs spawns a full
standalone process per session, each rebuilding config, tools,
registries, system prompt, and runtime. Adding 10 sessions in coco-rs ≈
10× the per-process baseline; in jcode it is 1 server + 10 thin clients.
The README's ~10MB/session vs Claude Code's ~212.7MB/session
(`README.md:234`) is directionally believable given this design. **This
win requires the deferred concurrent-app-server and is therefore not
addressable without that work.**

### 3. Idle RAM baseline — partly architecture, partly allocator tuning

Two jcode mechanisms coco-rs lacks: (a) always-on glibc arena capping
`mallopt(M_ARENA_MAX,4)` (`main.rs:30-44`) — directly cuts per-thread
arena fragmentation under a multi-threaded runtime; (b) lazy +
idle-unloaded embedding with explicit `malloc_trim(0)` / jemalloc purge
on unload (`embedding.rs:216-267`). coco-rs uses the stock system
allocator with no arena cap under a multi-thread Tokio runtime — higher
steady RSS is expected. The arena cap (S1) is portable today; the
embedding-unload mechanism is not directly relevant (coco-rs ships no
local embedding model).

### 4. Render loop — effectively a tie

Both are event-driven, coalesced, idle-free, and FPS-clamped. jcode adds
a *host-aware perf tier* (`perf.rs:75-356`) that down-shifts fps and
disables animations on SSH/WSL/Windows-Terminal/low-mem; coco-rs's
`FrameRequester` (codex-derived) is a clean 120fps clamp with infinite
idle sleep but has **no host-tier degradation and no animation-richness
tiering whatsoever** — it assumes a capable terminal (S4). jcode's
"1000+ fps" is a capability claim, not a steady-state claim; both idle at
effectively 0fps. coco-rs's actor-style `FrameScheduler` is arguably
simpler to reason about.

### 5. Boot observability — jcode wins

jcode has `startup_profile` phase marks across the whole boot path
(`startup.rs:13-49`, `tui_launch.rs:123-174`) and `process_memory`
smaps/PSS sampling; coco-rs has neither (only one coarse `tracing::info`
anchor at `main.rs:37`). You currently *cannot* answer "what dominates
coco-rs's TTFF" from instrumentation — you would have to add it. This is
also a prerequisite for trustworthy measurement of S1/S2/S4 (S3).

### 6. Backgrounding of detection work — jcode slightly ahead

jcode pushes ALL heavy host detection off the boot thread
(`perf::init_background`, `perf.rs:245-247`) plus the update check
(`startup.rs:80`). coco-rs backgrounds the *right* things too (session
cleanup via `spawn_blocking` `tui_runner.rs:156`; `model_card_refresh`;
plugin/theme/keybinding/config watchers) — its async discipline is sound
— but it keeps the latency-critical system-prompt CLAUDE.md scan +
registry loads + git-branch read (`tui_runner.rs:410`) on the
synchronous pre-frame path.

### 7. File-descriptor limit — jcode raises it, coco-rs does not

jcode raises `RLIMIT_NOFILE` soft limit to 8192 at boot
(`startup.rs:23`); coco-rs never adjusts it. This can bite
many-MCP / many-session workloads.

---

## Where coco-rs already matches or wins

1. **Idle render loop is mechanism-equivalent and arguably cleaner.**
   `FrameRequester`/`FrameScheduler` (`frame_requester.rs:100-128`)
   coalesces all redraw requests, clamps to 120fps via `FrameRateLimiter`
   (`frame_rate_limiter.rs:12`), and sleeps `ONE_YEAR` when idle. It
   drains `notification_rx.try_recv()` after the first event
   (`app.rs:298-303`) so a 100-token/sec stream yields one coalesced
   paint. The 250ms tick is `needs_tick()`-gated. Neither this nor
   jcode's per-state `redraw_interval` is clearly superior.

2. **Default release profile is more runtime-optimized.**
   `[profile.release]` = `lto="thin"`, `codegen-units=1`,
   `strip="symbols"` (`Cargo.toml:453-457`). jcode's *default*
   `[profile.release]` is tuned for self-dev rebuild speed —
   `opt-level=1, codegen-units=256, incremental=true`
   (`jcode Cargo.toml:255-259`); its optimized artifact is the separate
   `release-lto` profile. An out-of-the-box `cargo build --release`
   yields a faster-running binary in coco-rs.

3. **Tracing is mode-aware and zero-overhead for fast-exit paths.**
   `tracing_init` returns `Mode::Skip` for status/doctor/etc., installing
   *no* subscriber, so short subcommands keep clean stdout and pay
   nothing. jcode always runs `logging::init()` + `cleanup_old_logs()` on
   the boot path (`startup.rs:18-21`).

4. **coco-rs already backgrounds expensive non-critical work
   correctly.** Session cleanup runs in `spawn_blocking`
   (`tui_runner.rs:156`); model-card refresh is `spawn_if_enabled`; and
   plugin/theme/keybinding/config watchers are all spawned tasks. The
   async infrastructure is in place; the gap is *which* items are still
   synchronous, not a lack of async plumbing.

5. **Layering keeps provider/auth/cache concerns out of the hot path by
   design.** The per-provider `vercel-ai-*` crates own betas / cache-break
   / rate-limit, so the inference layer stays thin. This is a deliberate,
   defensible structure, not a deficiency relative to jcode's monolith.

### jcode README claims to flag as marketing, not fully substantiated in source

- **"1000+ fps"** — true only as single-frame *capability*; the actual
  loop is adaptive and idles at ~1fps. Not a sustained 1000fps render.
- **"14ms / 245× faster than Claude Code"** — real but *conditional on a
  warm server already running*. A cold first launch must `spawn_server`
  (`dispatch.rs:585`) and is not 14ms. The benchmark is
  thin-client-vs-cold-monolith, which flatters jcode.
- **"27.8MB"** — explicitly the *embeddings-off* baseline
  (`README.md:72`); the full default build (`default = ["pdf",
  "embeddings"]`, `Cargo.toml:227`) is far larger. Honest in the README
  footnote, but the headline number is the stripped variant.
- **`restart_snapshot`** is session re-launch continuity
  (`restart_snapshot.rs:184-203`, spawns resume in new terminals), NOT
  heap snapshot/restore — it gives no faster-than-cold process resume.

---

## Optimization recommendations for coco-rs (adversarially verified)

Each recommendation below survived adversarial review (verdict
**confirmed** or **nuanced**). Suggestions are ordered to make the
high-value reorder (S2) safe and measurable: do **S3 first**.

### S3 — Add startup phase instrumentation (a coco_otel startup-profile) [DO FIRST]

*Verdict: confirmed.*

**Why.** jcode attributes boot latency phase-by-phase via
`startup_profile` (`src/startup_profile.rs`: `init`/`mark`/`report`/
`report_to_log`), wired across the real boot path (`startup.rs:13-49`,
`tui_launch.rs:123-174`, even `server_spawn` marks in `dispatch.rs`).
coco-rs has nothing — grep across `app/` for
`startup_profile`/`time_to_first_frame`/`first_frame`/`ttff` is empty; it
has only one coarse anchor `tracing::info!(target:"coco_cli::startup", …)`
(`main.rs:37`). You cannot currently tell what dominates TTFF, and the
payoff of S1/S2/S4 is unmeasurable without this.

**Concrete change.** Add a tiny startup-profile helper — a
`Mutex<Vec<(&'static str, Instant)>>`, or better, `tracing` spans with an
explicit `elapsed_ms` field per anchor — in `common/otel` or directly in
`app/cli`. Mark the boundaries already present in `run_tui` (reloader
spawned, engine_resources built, system_prompt built, registries built,
`App::new`, first `redraw`) and emit one summary at info level. Reuse the
seven canonical span anchors from `common/otel/CLAUDE.md` rather than
inventing targets.

**Impact: medium. Effort: low. Risk: very low** (pure instrumentation, no
behavior change). Prerequisite for trustworthy S1/S2/S4 measurement.
Respects all coco-rs non-goals.

### S1 — Cap glibc malloc arenas at startup (`mallopt(M_ARENA_MAX, …)`)

*Verdict: confirmed.*

**Why.** jcode caps per-thread allocator arenas as the very first thing
in `main()`: `configure_system_allocator()` calls
`mallopt(M_ARENA_MAX, 4)` (`src/main.rs:30-44`) before the Tokio runtime
is built (`:53-55`) — the correct ordering, before any worker thread
spawns. coco-rs runs the stock glibc allocator (up to `8×ncpu` arenas)
under `#[tokio::main]` multi-thread (`main.rs:28`) with no arena cap —
grep for `mallopt`/`jemalloc`/`mimalloc`/`global_allocator`/`malloc_conf`/
`M_ARENA_MAX` across `.rs` and `.toml` is empty.

**Concrete change.** In `app/cli/src/main.rs`, before building the
runtime, add a Linux-only `configure_system_allocator()` that calls
`libc::mallopt(M_ARENA_MAX, 4)` (or a value read from a
`COCO_GLIBC_ARENA_MAX` `EnvKey`, per the `COCO_*` env-var rule —
**do not** add an ad-hoc `std::env::var`). A ~10-line `cfg(target_os =
"linux")`-gated function, no new dependency beyond `libc`, no API surface.
Optionally evaluate `tikv-jemallocator` behind an off-by-default
`jemalloc` feature later, but the arena cap alone is the cheap, low-risk
win.

**Impact: medium. Effort: low. Risk: low** (process-global, well
understood; must be `cfg(target_os="linux")` and applied before threads
spawn). Respects all coco-rs non-goals.

### S2 — Draw a first frame before the heavy synchronous bootstrap

*Verdict: nuanced — correction folded in below.*

**Why.** jcode reaches first frame fast because the thin client paints a
loading shell while heavy agent state lives in the warm server
(`tui_launch.rs:117-176`, `server.rs:394`). coco-rs gates its first frame
behind `build_engine_resources` → `discover_memory_files` (synchronous
root→cwd disk walk, `headless.rs:286`) → `fire_session_start_hooks`
(`tui_runner.rs:347`) → `install_theme().await` (`:494`) →
`install_keybindings().await` (`:514`), ALL before `App::new`
(`tui_runner.rs:364`) and the first `redraw()` (`app.rs:264`).

**Correction (critical — do not mischaracterize the mechanism).** jcode's
14ms is a *client/server artifact* — a separate persistent `jcode serve`
process (`spawn_server`, `dispatch.rs:585`) that coco-rs's single-process
TUI does not have, and replicating it is the *separate, larger* deferred
concurrent-app-server effort. Do **not** frame S2 as "mimic jcode's
thin-client paint". The valid, *in-process* recommendation stands on its
own: coco-rs's own port reference (the TypeScript Claude Code boot flow,
documented at `docs/coco-rs/crate-coco-app.md:375` "Deferred prefetches
(after first render)") already prescribes this ordering.

**Concrete change.** Reorder `run_tui` (`app/cli/src/tui_runner.rs`):
construct `App` + enter alt-screen and emit one initial `redraw()` of a
lightweight "loading" shell (cwd/git header is cheap) **first** —
`App::new` only needs the two channels (`tui_runner.rs:364`), not the
system prompt or registries. Then move
`build_system_prompt_for_model`'s `discover_memory_files` scan,
skills/plugins discovery, SessionStart-hook firing, `install_theme`, and
`install_keybindings` into `tokio::spawn` / `spawn_blocking` that feed
results in via the existing `notification_tx`/`CoreEvent` channel the
loop already drains. The system prompt is only needed at the first model
call, not the first frame — **gate the `SubmitInput` dispatch (not the
paint) on system-prompt readiness.**

**Impact: high. Effort: high. Risk: medium-high** — must ensure no
first-turn prompt dispatches before the system prompt / registries are
ready (add a readiness gate), and resume-hydration + SessionStart-hook
ordering vs first user input must be preserved for TS parity. Largest
suggestion but the biggest TTFF win. **Pair with S3 first** so the win is
measurable. Respects all coco-rs non-goals.

### S4 — Add a host-aware TUI performance tier (degrade fps/animations on SSH/WSL/low-mem)

*Verdict: confirmed — two narrowings folded in.*

**Why.** jcode adapts render cadence and animation richness to the host:
`PerformanceTier` + `TuiPerfPolicy` (`perf.rs:75-356`) derive
`redraw_fps`/`animation_fps`/decorative-animation/focus-change/
keyboard-enhancement from `compute_tier` (`:306`) using load ratio,
available MB, SSH (forces `Minimal`, `:315`), WSL, terminal family —
e.g. SSH → 12fps, WSL → 30fps, WSL+Windows-Terminal → 20fps + focus/
keyboard-enhancement off (`:202-225`). Detection is cheap env reads, run
off-thread (`:245`). coco-rs hardcodes a single `FrameRateLimiter::default()`
= 120fps (`frame_rate_limiter.rs:12`, `frame_requester.rs:96`) with no
host detection and no config input.

**Narrowings.** (1) The gap is broader than fps: coco-rs has **no
animation-richness tiering at all** (no decorative/idle/focus-change/
keyboard-enhancement gating), not just a fixed fps. (2) `DisplaySettings`
exists (`app/tui/src/display_settings.rs:64`) but currently carries **no
`redraw_fps`/`animation_fps` fields** — so this is net-new config, not
wiring an existing knob.

**Concrete change.** In `coco-tui`, add a lightweight `PerfTier` /
`TuiPerfPolicy` analog: detect SSH via `SSH_CONNECTION`/`SSH_TTY`, WSL via
`/proc/version`, terminal via `WT_SESSION`/`TERM_PROGRAM` — all cheap env
reads done once at `App::new`. Feed the resolved max-fps into
`FrameRateLimiter` (which today ignores config) and an animations-enabled
flag into the spinner self-scheduling in `App::redraw`
(`app.rs:417-419`). Add `redraw_fps`/`animation_fps` fields to
`DisplaySettings` for user override. Keep it a TUI-local concern, **not** a
`Feature`. Add `insta` snapshot coverage per TUI conventions if any
visible animation changes.

**Impact: medium. Effort: medium. Risk: low** (worst case is too-conservative
fps on a fast SSH link; expose an override). Respects all coco-rs non-goals.

### S5 — Set a stable process title / `PR_SET_NAME`

*Verdict: confirmed.*

**Why.** jcode tags each process role so `killall jcode` and per-session
`ps` work: `set_title` uses the `proctitle` crate AND
`prctl(PR_SET_NAME, "jcode")` (`process_title.rs:68-82`), with
role-specific titles (`jcode:s:server`, `jcode:c:<session>`,
`jcode:selfdev`) chosen per subcommand (`initial_title`, `:119-130`), set
once during arg prep (`startup.rs:66`). coco-rs sets no proctitle /
`PR_SET_NAME` for the `coco` binary — grep shows only unrelated
`PR_SET_PDEATHSIG` (`utils/pty/src/process_group.rs:28`,
`utils/sleep-inhibitor`) and `PR_SET_DUMPABLE`
(`exec/process-hardening/src/lib.rs:58`). `coco ps`
(`app/cli/src/main.rs:160`) reads `sessions/<pid>.json` files, so OS-level
process identification is genuinely absent.

**Concrete change.** Add a small `process_title` helper in `app/cli`
(`cfg(target_os="linux")` `prctl(PR_SET_NAME, b"coco\0")`, optionally the
`proctitle` crate for the full argv-area title) and call it once in
`main()` after `Cli::parse()`, choosing a compact title from the
subcommand / session name (e.g. `coco:tui:<short-session>`). Use a
`COCO_*`-namespaced opt-out if desired.

**Impact: low. Effort: low. Risk: very low** (cosmetic OS-process
metadata; no functional path depends on it). Directly supports the
multi-session/daemon/hub direction coco-rs is already building
(`Commands::Ps/Daemon/Attach/Kill` exist as stubs). Respects all coco-rs
non-goals.

### S6 — Raise `RLIMIT_NOFILE` at boot (from verifier missed-findings)

*Verdict: confirmed missed-finding.*

**Why.** jcode raises the file-descriptor soft limit before the runtime:
`crate::platform::raise_nofile_limit_best_effort(8_192)`
(`src/cli/startup.rs:23`), marked `nofile_limit` in its startup profile.
coco-rs never adjusts `RLIMIT_NOFILE` (grep for
`RLIMIT_NOFILE`/`setrlimit`/`raise_nofile` across `app/`, `common/`,
`utils/` is empty). Under many-MCP / many-session fan-out (each MCP server
+ socket consumes descriptors) the default soft limit can be hit.

**Concrete change.** Add a Linux/macOS-only best-effort
`raise_nofile_limit(8192)` (via `libc::getrlimit`/`setrlimit` on
`RLIMIT_NOFILE`, clamped to the hard limit) called once in `main()` before
building the runtime. Best-effort: ignore failures (sandboxed/restricted
environments). A ~15-line `cfg`-gated helper, no new dependency beyond
`libc`.

**Impact: low–medium (workload-dependent). Effort: low. Risk: very low.**
Respects all coco-rs non-goals.

> **Recommendations intentionally NOT made (conflict with coco-rs
> non-goals / scope):**
> - *Persistent shared server + shared MCP pool* (jcode `server.rs:394,
>   429`): this is the root cause of jcode's ~10MB/session and warm-TTFF
>   wins, but it **is** the deferred concurrent-app-server work
>   (hub/connector/server) — a separate, larger initiative, not a
>   drop-in.
> - *Allocator introspection / runtime purge API* (jcode
>   `process_memory.rs:200-273` `purge_allocator`/`set_allocator_decay_ms`)
>   and *idle embedding unload with `malloc_trim`* (`embedding.rs:216-267`):
>   jcode's purge API is jemalloc-specific, and the unload hook exists to
>   reclaim its ~87MB ONNX model — coco-rs ships **no** local embedding
>   model, so the mechanism has no payload to reclaim. Revisit only if
>   coco-rs adopts jemalloc (S1's optional extension) AND a large
>   resident model.
> - *`/proc/self/smaps_rollup` PSS self-sampling* (jcode
>   `process_memory.rs`): genuinely useful for verifying footprint claims,
>   but lower priority than S3's boot timing; could be folded into the
>   same `common/otel` observability surface later.

---

## Rejected after adversarial review

No suggestion in this module's analyst set was **refuted** — all five
analyst suggestions (S1–S5) plus the strongest verifier missed-finding
(S6) survived. One (S2) was downgraded to **nuanced** rather than
rejected, and its correction is folded into the recommendation above:

- **S2 (nuanced, not rejected).** The literal analyst framing — "coco-rs
  should reach first frame fast like jcode's thin client" — was
  corrected. jcode's 14ms is a **client/server artifact** (a separate
  persistent `jcode serve` process, `dispatch.rs:585`), *not* an
  in-process paint-then-load reorder. Recommending coco-rs "mimic the
  thin-client paint" would conflate the cheap in-process reorder (valid,
  retained) with the expensive deferred concurrent-app-server (out of
  scope). The retained recommendation is purely the in-process reorder:
  paint a loading shell first (`App::new` needs only the two channels),
  then background `discover_memory_files` / skills+plugins / SessionStart
  hooks / theme / keybindings, gating `SubmitInput` (not the paint) on
  readiness.

For completeness, the items deliberately *excluded from the
recommendation list* (persistent shared server, jemalloc purge API,
idle-embedding `malloc_trim`, smaps PSS sampling) are not "refuted
suggestions" — they are correct descriptions of jcode mechanisms whose
adoption either conflicts with coco-rs's deferred-server scope or has no
payload in coco-rs (no local embedding model). They are listed in the
boxed note above so readers see what was checked and consciously set
aside.
