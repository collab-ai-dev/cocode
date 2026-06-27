# Agent-Teams Cross-Process E2E — Test Design (2026-06)

> **Status: DESIGN ONLY (not implemented).** This is the plan to retire the
> last open agent-teams item: the "E2E-unvalidated" debt for the cross-process
> boot→consume→turn→report→shutdown loop (gaps 1 / 4b / 6 / 8-cross). It is the
> validation counterpart to `agentteam-completeness-audit-2026-06.md`.

## 0. Coverage: in-process vs cross-process teammates

The two teammate kinds run DIFFERENT code paths and are tested separately:

| Concern | In-process (`coordinator::runner_loop`) | Cross-process (`app/cli::teammate_inbox_pump` + real process) |
|---|---|---|
| Mailbox priority scan (shared `scan_next_prompt`) | ✅ `select_mailbox_prompt_*` (6 tests) | ✅ same core; L1 `scan_*` against real mailbox |
| Control consumer (gap 8 ModeSet) | ✅ `in_process_drain_applies_mode_set_against_real_mailbox` (`drain_control_messages` → live `TeammateControlState`) | ✅ L1 `drain_applies_mode_set…` (`drain_control_tick` → `SetPermissionMode`) + `control_message_to_command` |
| team-permission rules push | applied via `TeammateControlState` (no constructor → no unit test, same as cross) | applied via shared live-rules `Arc` (no constructor → no unit test) |
| Runner registry / lifecycle | ✅ `runner.test.rs` (spawn/cancel/capacity/result) | n/a (it's a separate OS process) |
| Full turn loop (consume→turn→idle/shutdown) | ✅ `in_process_teammate_runs_initial_prompt_then_exits_on_shutdown` — `run_in_process_teammate` driven by a mock `AgentExecutionEngine`: runs the initial-prompt turn, exits cleanly on a seeded `ShutdownRequest` | ✅ **L2** real-binary PTY + wiremock |
| Process/pane spawn + teardown | n/a (runs in-process) | ✅ **L3** real tmux `create_teammate_pane` + `kill_pane` |

**Net:** in-process and cross-process now have **symmetric coverage of the full
mailbox → control → turn-loop path** — the cross-process side proves it with a
real binary (L2), the in-process side with a mock engine driving the real
`run_in_process_teammate`. The only remaining cross-process-exclusive layer is
**L3** (real tmux pane spawn/teardown), which has no in-process analog (an
in-process teammate runs in the leader's process, with no OS pane to manage).

## 1. What we're validating (and what we're not)

The cross-process teammate path has solid **unit** coverage of its pieces:
`scan_next_prompt` priority, `inject_and_wait` turn-id handshake,
`control_message_to_command`, protocol round-trips, shutdown constructors,
roster filter/cycle. What's missing is an **integration** proof that a *real*
`coco` teammate process, launched with `COCO_AGENT_*` env, actually:

1. **boots and consumes its file mailbox** (gap 1 — the pump),
2. **runs a real turn** off an injected prompt,
3. **reports back** to the `team-lead` mailbox (gap 4b),
4. **applies a leader control message** live (gap 8-cross: `ModeSetRequest`
   → `SetPermissionMode`; `TeamPermissionUpdate` → live-rules `Arc`),
5. **shuts down cleanly** on a `ShutdownRequest` → `ShutdownApproved` round
   trip, with the leader tearing down the pane (gap 6).

**Out of scope:** gap 10 (sandbox — structurally blocked); model *quality*
(we use a scripted mock, not a real LLM); the in-process teammate runner
(already regression-tested in `coordinator::runner_loop`).

## 2. Strategy: four layers, value front-loaded

A single monolithic "spawn two `coco` processes inside tmux and assert on an
LLM turn" test is maximally flaky (tmux availability, real process timing,
model nondeterminism). Instead, **decompose by what each layer needs**, so the
bulk of the validation is deterministic and CI-friendly and only the
genuinely-tmux-dependent slice is gated/ignored.

| Layer | Needs | Validates | CI |
|---|---|---|---|
| **L0 infra** | a `COCO_TEAMS_DIR` override | (enabler — hermetic teams/mailbox dir) | n/a |
| **L1 hermetic pump** | temp dir only (no process, no model) | `drain_control_tick` + `scan_tick` against a real on-disk mailbox: mode-set / team-permission / plain-msg framing / shutdown priority | always |
| **L2 single child + mock model** | `coco` binary + `wiremock` + temp dirs (NO tmux) | gap 1 + 4b end-to-end: a real teammate process consumes its mailbox, hits the mock model, writes a reply to `team-lead` | always (binary builds in CI) |
| **L3 full two-process tmux** | real `tmux` + 2 `coco` processes | pane spawn + leader surfacing + `ShutdownApproved` → `kill_pane` (gap 6) | `#[ignore]`, tmux lane only |

Most of the real risk (the pump's mailbox logic, the child actually booting +
consuming + reporting) is proven at **L1–L2 without tmux**. L3 only adds the
pane-lifecycle proof that can't be had without a real terminal multiplexer.

## 3. Layer 0 — hermetic base-dir infra (`COCO_TEAMS_DIR`)

**Problem.** `coordinator::team_file::teams_base_dir()` hardcodes
`dirs::home_dir().join(".claude/teams")`. Every mailbox / team-file read and
write funnels through it (`inbox_dir`, `get_team_dir`, …). A test cannot
isolate without polluting the developer's real `~/.claude/teams`.

**Fix (single change point).** Add `EnvKey::CocoTeamsDir` (`"COCO_TEAMS_DIR"`)
and consult it first in `teams_base_dir()`:

```rust
pub fn teams_base_dir() -> PathBuf {
    if let Some(dir) = coco_config::env::env_opt(coco_config::EnvKey::CocoTeamsDir) {
        return PathBuf::from(dir);
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("teams")
}
```

- Per project rule: register the variant on `coco_config::EnvKey`; read via
  `coco_config::env::env_opt` (never ad-hoc `std::env::var`). `coordinator`
  already depends on `coco-config`.
- This is the ONLY resolution site, so all mailbox/team-file callers inherit
  the override automatically (no API churn).
- **Bonus parity:** it's the same shape as the existing
  `COCO_REMOTE_MEMORY_DIR` override (swarm-leader / CCR), so it also gives the
  remote-leader case a way to relocate the teams dir later.

**Env-mutation caveat.** `cargo-nextest` runs **each test in its own process**,
so setting `COCO_TEAMS_DIR` in a test is isolated. Under bare `cargo test`
(shared process) parallel tests could collide. Mitigation: mark L1/L2 tests
`#[serial_test::serial(coco_teams_dir)]` (add `serial_test` dev-dep) OR document
"run under nextest" (the repo standard — `just test` uses nextest). Recommend
the `serial` guard for portability since the cost is trivial.

## 4. Layer 1 — hermetic pump integration test (no process, no model)

**Where:** `app/cli/src/teammate_inbox_pump.test.rs` (extends the existing
unit tests) or a new `app/cli/tests/teammate_pump_mailbox.rs` integration file.

**Setup (deterministic):**
1. `let teams = tempfile::tempdir()` ; set `COCO_TEAMS_DIR=teams.path()`.
2. Create `teams/<team>/inboxes/` and a `team.json` with the teammate + a
   `team_allowed_paths` entry (via `team_file::write_team_file`).
3. Build a `TeammateIdentity { agent_name, team_name, … }`.

**Cases (each asserts on real on-disk mailbox state):**
- **Mode-set:** `write_to_mailbox(<teammate>, ModeSetRequest{Plan, from:team-lead})`;
  run `drain_control_tick` with a captured `command_tx`; assert a
  `UserCommand::SetPermissionMode{Plan}` was sent **and** the message is
  marked read.
- **Team-permission:** write a `TeamPermissionUpdate`; pass a shared
  `Arc<RwLock<Vec<PermissionRule>>>`; assert it gained the derived rules.
- **Plain message:** write a non-structured message; run `scan_tick`; assert it
  returns the `<teammate_message …>`-framed prompt (not consumed by the control
  drain).
- **Shutdown priority:** write a normal message AND a `ShutdownRequest`; assert
  `scan_next_prompt` surfaces the shutdown first (already unit-tested in
  `coordinator`, re-asserted here against the real mailbox path).

This is the highest-ROI layer: it exercises the **real file IPC** the pump uses,
fully deterministic, no binary, no tmux, no model. It would have caught e.g. a
mailbox path-construction bug that the in-memory unit tests can't.

## 5. Layer 2 — single child process + mock model (no tmux)

**Goal:** prove a *real* `coco` teammate process boots from `COCO_AGENT_*` env,
the pump drives a turn off a seeded mailbox prompt, the turn hits a model, and
the teammate writes a reply to the `team-lead` mailbox. **No tmux** — the
teammate is just a child process; tmux only matters for the leader's *pane*
creation (L3).

**Where:** `app/cli/tests/teammate_e2e.rs` (new integration test crate).
**Dev-deps:** `coco-cargo-bin`, `wiremock`, `tempfile`, `serde_json`, `tokio`,
`serial_test`.

**Harness:**
1. **Mock model server (`wiremock`).** Mount a responder on the Anthropic
   Messages endpoint (`POST /v1/messages`) returning a scripted assistant
   response. Two useful scripts:
   - *Text reply* — minimal `{content:[{type:text,text:"done"}], stop_reason:"end_turn", …}` to prove a turn ran.
   - *Report-back* — a `tool_use` block calling `SendMessage` with
     `{to:"team-lead", message:"…", summary:"…"}` so the teammate's turn writes
     to the team-lead mailbox (proves gap 4b through real tool execution).
   wiremock records requests, so we can also assert the child actually called
   the model (request count ≥ 1).
2. **Temp config home (`COCO_CONFIG_DIR`).** Write a minimal provider+model
   config selecting an Anthropic-family model with `api_key:"test"` and
   `base_url` → the wiremock URI (or set `ANTHROPIC_BASE_URL` + `ANTHROPIC_API_KEY=test`
   env directly — simpler, fewer files). Pick the provider whose adapter does
   the least response-shape validation to keep the mock minimal.
3. **Temp teams dir (`COCO_TEAMS_DIR`).** Seed `team.json` (teammate member,
   `backend_type:in-process` or `tmux` with empty pane) + an `inboxes/` dir.
4. **Seed the teammate's mailbox** with one plain prompt (`write_to_mailbox`).
5. **Launch the child** via `coco_cargo_bin::cargo_bin("coco")`, env:
   `COCO_AGENT_ID=worker@team`, `COCO_AGENT_NAME=worker`, `COCO_TEAM_NAME=team`,
   `COCO_FEATURE_AGENT_TEAMS=1`, `COCO_CONFIG_DIR`, `COCO_TEAMS_DIR`,
   `ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY=test`, plus a non-interactive /
   headless flag so it doesn't open the TUI alt-screen in CI.
   - **Open question (§8):** the pump currently lives on the *TUI* agent-driver
     path (`tui_runner::run_agent_driver`). A headless (`-p`) teammate may not
     spawn the pump. Resolve before L2: either (a) ensure the pump also spawns
     on the headless teammate path, or (b) run the child in a PTY (via
     `utils/pty`) so the TUI path is active without a real terminal. Option (b)
     keeps the production path under test; preferred.
6. **Assert (poll-with-timeout, no sleeps):**
   - the `team-lead` inbox file gains a message from `worker` within N s
     (proves consume→turn→report), OR
   - wiremock received ≥1 `/v1/messages` request (proves the turn ran), and
   - the teammate's own inbox prompt is marked read.
7. **Teardown:** kill the child (drop guard / `Child::kill`); tempdirs auto-clean.

**Shutdown sub-case (still no tmux):** after the first turn, write a
`ShutdownRequest` to the teammate's mailbox; with the report-back script the
teammate's model reply calls `SendMessage{shutdown_response, approve:true}`;
assert a `ShutdownApproved` lands in the `team-lead` inbox. This proves the
gap-6 *worker producer* end-to-end without needing a pane to kill.

## 6. Layer 3 — full two-process tmux (gated, `#[ignore]`)

**Goal:** the only thing L1/L2 can't prove — real pane lifecycle.

**Gate:** `if !coco_coordinator::pane::is_tmux_available() { return; }` and
`#[ignore]` so it runs only in an explicit tmux lane (`cargo nextest run
--run-ignored all` on a tmux-equipped runner), never blocking normal CI.

**Flow:**
1. Launch a **leader** `coco` (same mock-model + temp dirs), drive it (via SDK
   stdin or a seeded prompt) to `TeamCreate` + spawn a teammate — the leader's
   tmux backend creates a real pane running a teammate `coco`.
2. Assert the pane exists (`tmux list-panes -t <session>` parsing) and the
   teammate consumed its initial prompt (team-lead inbox reply).
3. Drive the leader to request shutdown (TaskStop/SendMessage `shutdown_request`);
   the teammate approves; assert the leader's `ShutdownApproved` handler
   `kill_pane`s it (`tmux list-panes` no longer shows it) and removes the
   member from `team.json`.
4. Assert leader-exit cleanup kills any orphaned panes (gap 7).
**Teardown:** `tmux kill-session` on the test session; kill child processes.

This layer is deliberately last and optional: its unique value (pane create +
kill) is small relative to its flakiness.

## 7. Flakiness & isolation rules

- **No `sleep`-then-assert.** Always poll a mailbox file / wiremock request log
  with a bounded deadline (e.g. `for _ in 0..100 { check; sleep(50ms) }`).
- **Process-per-test (nextest)** isolates env; add `#[serial_test::serial]` for
  bare-`cargo test` safety on the env-mutating tests.
- **Always kill children + tmux sessions** in a teardown guard so a failed
  assert never leaks a `coco` process or a tmux pane.
- **Tempdirs for every dir** (`COCO_CONFIG_DIR`, `COCO_TEAMS_DIR`,
  `COCO_REMOTE_MEMORY_DIR` if needed) — zero writes to the real home.
- L3 skips cleanly (not fails) when tmux is absent.

## 8. Open questions

1. **Pump is TUI-path-only → L2 needs a PTY. [CONFIRMED 2026-06]**
   `teammate_inbox_pump::spawn` is called **only** from `tui_runner` (the TUI
   agent-driver path); `app/cli/src/headless.rs` has no teammate-identity
   branch. So a `-p`/headless child would NOT pump. **Decision: L2 runs the
   child under a PTY** (`utils/pty`) so the production TUI path activates
   without a real terminal — this keeps the real pump under test rather than
   adding a parallel headless pump just for testing.
2. **`COCO_TEAMS_DIR` fully isolates. [CONFIRMED 2026-06]** Every teams/mailbox
   path routes through `team_file::teams_base_dir()`: `inbox_dir →
   get_team_dir → teams_base_dir`, `get_team_file_path → get_team_dir`,
   `list_team_names`/`cleanup_*` → `teams_base_dir`. One override at
   `teams_base_dir()` covers all of them.
3. **Tasks dir is `config_home()`-based, not teams-based.** A team's task-list
   dir lives under `config_home()/tasks/…`, so `COCO_CONFIG_DIR` (not
   `COCO_TEAMS_DIR`) relocates it. L2/L3 must set BOTH temp overrides; confirm
   `cleanup_team_directories` honors the relocated tasks dir.
4. **Minimal mock-model fidelity (still open).** Which provider adapter
   validates the response shape the least (Anthropic vs OpenAI-compatible)?
   Pick that for the smallest wiremock stub; confirm it accepts a `base_url`
   override + a dummy `api_key`. (Anthropic is the likely pick given
   `ANTHROPIC_BASE_URL` is the cleanest single-env override.)

## 9. Build order / status

1. **L0 — DONE.** `EnvKey::CocoTeamsDir` (`"COCO_TEAMS_DIR"`) +
   `team_file::teams_base_dir()` override.
2. **L1 — DONE.** `app/cli/src/teammate_inbox_pump.test.rs` hermetic tests
   (`drain_applies_mode_set_against_real_mailbox`,
   `scan_frames_plain_peer_message_from_real_mailbox`,
   `scan_prioritizes_shutdown_over_plain_in_real_mailbox`) drive the pump
   against a real `COCO_TEAMS_DIR` tempdir. Env-mutating tests serialize via an
   async `ENV_LOCK` + a `TeamsDirGuard` Drop-restore; nextest isolates per
   process.
3. §8.1 resolved — **L2 runs the child under a PTY** (pump is TUI-path-only).
4. **L2 — DONE + PASSING.** `app/cli/tests/teammate_pty_e2e.rs::teammate_pty_consumes_mailbox_and_runs_turn`
   (`#[ignore]`). Spawns the REAL `coco` binary in a PTY (`coco-utils-pty`)
   with `COCO_AGENT_*` identity + a temp `COCO_CONFIG_DIR` (`providers.json`
   repointing `anthropic.base_url` at a `wiremock` SSE model) + a temp
   `COCO_TEAMS_DIR` seeded with one mailbox prompt; asserts the mock received a
   `POST /messages` carrying the framed prompt. **Verified passing** (real
   binary boots → pump consumes mailbox → runs a turn → hits the model).
5. **L3 — DONE + PASSING.** `coordinator/src/pane/tmux.test.rs::tmux_create_pane_spawns_real_pane`
   (`#[ignore]`, tmux-gated). `TmuxBackend::create_teammate_pane` creates a
   real pane on the PID-scoped swarm socket; asserts it appears in
   `tmux list-panes`. **Verified passing against tmux 3.4.**

### Finding surfaced while writing L3 — FIXED + regression-tested

`TmuxBackend` external mode (`is_native=false`, reachable when tmux is
available but the leader isn't inside it) had a **socket inconsistency**:
`create_teammate_pane_external` ran `tmux` on the PID-scoped swarm socket
(`-L claude-swarm-<pid>`), but `kill_pane` / `send_command_to_pane` /
`set_pane_*` / `hide` / `show` / `rebalance` ran `tmux` with NO `-L` (default
socket). So an external-session teammate's pane couldn't receive its command or
be torn down — a real gap-6/gap-1 teardown/spawn bug.

**Fix (done):** the socket decision is now a single backend property —
`TmuxBackend::socket()` (`None` native → inherited `$TMUX`; `Some(swarm)`
external) — and every op routes through one `TmuxBackend::run()` entry point.
Native behavior is unchanged (no `-L`); external now addresses the dedicated
server for ALL ops. The L3 test was upgraded to create→kill via the backend
and assert the pane is gone on the swarm socket — it fails pre-fix (kill missed
the right server) and passes post-fix (verified against tmux 3.4).

L0+L1 alone convert the pump's mailbox path from "unit-mocked" to
"integration-proven against real on-disk IPC" — the cheapest large step.
L2 is the headline "a real teammate process actually works" proof.
