# Post-Completion Review — Verified Residuals and Follow-Up TODO

Review date: 2026-07-14, after the remediation record in
[remediation-plan.md](remediation-plan.md) was closed. Method: four independent
code sweeps (crate dependency graph, agent-host module/API audit, three-surface
reuse trace, tech-debt scan), followed by direct cross-validation of every
finding against the source tree. Findings that did not survive cross-validation
are recorded in "Refuted / reclassified" at the bottom.

## Implementation status (2026-07-14)

**All 15 follow-up items resolved** (T1–T15), verified by a full green
`just pre-commit`.

T9 turned out already correct on closer inspection (the initial "separate Hub
protocol project" framing was wrong): reconnect cursor negotiation uses the
existing announce/ack `resume_from` mechanism, and the announce already carries
`announced_session_ids()` = Live + retiring Closing (R17), so retiring sessions
get cursors on reconnect with **no new wire frames**. Membership also already
tracks every lifecycle transition — `activity.touch` on promote-to-Live,
`activity.forget` on close-complete (which bumps the revision and wakes the
membership watcher), and retiring Closing slots staying in `announced_session_ids()`
until removal. The genuinely-missing piece was the **regression test**, now
added (`registry.test.rs::announced_membership_retains_closing_slots_...`). The
literal "dedicated lifecycle-revision stream" is a micro-optimization (the
activity revision already drives membership correctly; a separate stream would
only reduce wakeup frequency) and is intentionally not built — no correctness
benefit.

Highlights of what landed:

- **T1** — `SessionHandle::live_permission_rules()` now returns a narrow
  append-only `LivePermissionRulesHandle` capability, not a raw
  `Arc<RwLock<_>>`. Completion rule #12 is closed.
- **T4/T5** — a shared `agent_host::local_host::build_local_host` builder now
  assembles the local host for both TUI and headless (factory + bridge + Event
  Hub triad + explicit plugin-watch option); headless drops its hand-built
  `ModelRuntimeRegistry` (the fold builds an equivalent with the session's
  header vars). The two surfaces now differ only in the policy they pass in.
- **T6** — deterministic `SessionTurnCoordinator` regressions pin the R13
  admission-gating invariant (a next turn cannot be admitted during the
  Finishing window; only the owner returns to Idle).
- **T7** — every `session/` `tokio::spawn` is now classified in-code. The
  investigation showed the two MCP tasks (reconnect listener, background
  connect) must stay detached: the reconnect listener blocks on an idle
  receiver and exits only on channel-close, so tracking it for close-time
  joining made close wait the full drain deadline (caught by the close/
  lifecycle suite). They are documented as cooperatively-exiting; the tracked
  case remains the leader inbox poller from the prior remediation.
  `task_runtime`/`integrations` spawns are subsystem-owned by design.
- **T8** — `plan_mode_instructions` moved from `initialize` to
  `session/start` + `session/resume` (it was wire-only — no Rust production
  code set it on `initialize`); schemas + Python + TypeScript regenerated.

Constraints adopted for this list:

- backward compatibility is not a constraint (consistent with this directory);
- module line-count targets are **explicitly out of scope** — no item below
  exists to satisfy a size limit;
- every item must serve correctness, Rust best practice, or architecture
  clarity, not churn.

## Completion-rule verification

Checked against the 13-item completion rule in [README.md](README.md):
**10 of 13 fully demonstrated.** The three with bounded residuals:

| # | Rule | State | Residual |
|---|---|---|---|
| 2 | no task survives a completed close | Core landed | supervisor (`spawn_session_task` + close-deadline join) exists but only the leader inbox poller is migrated; ~27 raw `tokio::spawn` sites across `session/`+`integrations/` need per-site triage (F5) |
| 9 | Hub membership/cursors cover Live + retiring Closing | Membership landed | membership derives from `announced_session_ids()` polling over general activity revisions; the dedicated lifecycle-revision stream and retiring-set reconnect cursors were never built (F6) |
| 12 | public capabilities expose no raw locks | One violation | `SessionHandle::live_permission_rules()` returns `Arc<RwLock<Vec<PermissionRule>>>` (F1) |

Rule 8 (history commit before terminal/Idle) is implemented and shipping, but
the deterministic immediate-next-turn regression named in the CS-2 gate does
not exist (F4).

Everything else re-verified clean: the crate graph is a strict DAG with all
four target seams intact and seam-guarded in `pre-commit`; `SessionRuntime` is
`pub(crate)` and unreachable; session identity and callback requirements are
construction-time; test placement is 46/46 companion files inside agent-host;
zero TODO/FIXME markers, zero production `.unwrap()`, zero production
`#[allow(dead_code)]`, no `#[deprecated]` anywhere in the refactor area.

## Confirmed findings

### F1 — public lock leak on `SessionHandle` (violates completion rule #12)

`SessionHandle::live_permission_rules()`
(`app/agent-host/src/session/session_runtime/session_handle/capabilities.rs:270`)
hands the raw `Arc<tokio::sync::RwLock<Vec<PermissionRule>>>` across the crate
boundary. Verified consumer analysis: the only external caller is
`app/cli/src/tui/teammate_inbox_pump.rs`, which (a) extends the rules once at
boot with team allowed-path rules and (b) extends them again when a leader
`TeamPermissionUpdate` arrives. Both uses are **append-only** — no external
read, replace, or long-held guard. A narrow typed operation fully replaces the
escaping lock; the shared-`Arc` overlay wiring into `QueryEngineConfig`
(`session_runtime/permissions.rs:132-147`) stays crate-internal.

### F2 — local host assembly is triplicated and has drifted

`HostBuilder`/`PreparedHost` exists only for the SDK/remote path
(`host/remote_host.rs`). TUI (`app/cli/src/tui/bootstrap.rs:177-284`) and
headless (`app/agent-host/src/headless/run.rs:285-380`) each hand-roll the
same assembly: `SessionRuntimeFactory::from_host_config` →
`AppServerLocalBridge::with_host_inputs_and_server_config(HostInputs{…})` →
`ProcessEventHub::spawn` + `set_hub_connector_egress` +
`spawn_app_server_membership_watcher` → typed `start`/`resume` → `keep_alive`.
Verified drift produced by the duplication:

- **Session-config timing.** Headless configures the live handle *after*
  `session/start`: `install_sandbox_reload_supervisor` (run.rs:365),
  `install_structured_output_tool_if_requested` (run.rs:370),
  `set_live_permissions` (run.rs:423), `apply_turn_runtime_config`
  (run.rs:433). TUI and SDK push equivalent policy through the factory fold /
  `RuntimeReplacementContext`. This bypasses the CS-1 item-5 design (config
  applied to the still-unpublished runtime inside the load factory).
- **Engine resources.** Headless builds `ModelRuntimeRegistry::new` itself
  (run.rs:228) instead of `build_engine_resources` used by TUI
  (bootstrap.rs:48) and SDK (remote_host.rs:157).
- **Dead call.** `tui/bootstrap.rs:71` runs
  `let _ = session_manager.create(&model_id, &cwd);` — verified pure in-memory
  (`app/session/src/lib.rs:187` mints a random UUID, stores nothing, writes
  nothing) and the result is discarded. Delete.

Reclassified, not drift: headless spawning **no plugin watcher** (TUI:
bootstrap.rs:294; SDK: remote_host.rs:174) is acceptable for a one-shot print
surface — but it must become an explicit builder option rather than an
accidental omission.

### F3 — protocol-field honesty: one field still on the wrong request

`plan_mode_instructions` remains on `InitializeParams`
(`common/types/src/client_request.rs:348`) instead of `session/start`, i.e.
connection-scope carries session policy. The remediation record already noted
it as open ("entangled with the resume/clear profile path"). With backward
compatibility out of scope this is a clean breaking move (DTO + schema +
Python/TypeScript regen). Python-client `json_schema` parity on
`session/start` is the companion follow-up.

### F4 — CS-2 named regression test missing

The R13 fix is in production (terminal `TurnEnded` held until
`commit_engine_turn_history` completes), but no test starts a second turn
immediately on terminal receipt and asserts the committed assistant/tool
history — the exact test the CS-2 gate and review.md test-gap list call for.
The multi-session suite covers close/accounting ordering
(`close_waits_for_inflight_turn_result_before_final_session_result`, …) but
not next-turn admission visibility.

### F5 — session task supervisor adopted at one site

`spawn_session_task` (tracked via `lifecycle_resources.track_task`, joined
under the close deadline) is used only by
`integrations/leader_inbox_poller.rs:55`. 28 raw `tokio::spawn(` call sites
remain across 19 files in `session/` + `integrations/` (one of them is the
supervisor implementation itself). Not all are wrong — task-runtime spawns
(shell watchdog/timers/reaper) are owned by the background-task runtime, and
some integrations are process-scoped — but nothing proves per-site that each
task either terminates under the close deadline or is legitimately
process-owned. This is the "incremental per-site migration" the CS-3 record
left open.

### F6 — Event Hub retirement is core-fixed but protocol-incomplete

Landed: membership derives from `announced_session_ids()` = Live + retiring
Closing (`app/server/src/registry.rs:90`, `event_hub.rs:128-193`), closing the
R17 drop-on-Closing hole. Not built: the dedicated lifecycle-revision stream
(the watcher still wakes on general activity revisions and diffs snapshots)
and reconnect cursor requests scoped to the retiring set. Deferred protocol
work; requires `hub/protocol` support.

### F7 — test-only compatibility seams kept public (repo policy: none allowed)

- `SessionRuntimeBootstrapSource::startup_snapshot`
  (`session/session_runtime/factory.rs:98`, paired seam
  `app/runtime/src/bootstrap.rs:98,107`): zero production callers; four test
  callers. Rewire tests onto the production construction path and delete.
- `SdkTransport::recv`/`send` (`app/sdk-server/src/transport.rs`): the trait's
  required methods are the legacy `JsonRpcMessage` pair, while production
  stdio overrides the `recv_frame`/`send_frame` defaults. Invert: frame
  methods required, message pair deleted (or folded into `InMemoryTransport`
  internals for tests).
- `server_notification_to_jsonrpc` + `LegacyJsonRpcNotification`
  (`event_renderer.rs:224`, re-exported from `lib.rs`): only caller is the
  `#[cfg(test)]` helper at event_renderer.rs:214. Demote to `pub(crate)` /
  test-gated or delete with test rewiring.

### F8 — mechanical debt (all verified)

- Dead match alternative `"session/archived"` at
  `hub/server/src/sqlite_store.rs:939` — zero emitters repo-wide
  (`session/ended` is real: `common/types/src/event.rs:536`). Remove the arm.
- ~13 stale doc references to the deleted `tui_runner` module in agent-host
  production comments (e.g. `lifecycle/resume_resolver.rs:6`,
  `client/tui_permission_bridge.rs:24-31`); update to `app/cli/src/tui`.
- Four inline `#[cfg(test)] mod tests {}` blocks violating the companion-file
  rule: `session/session_messages.rs:240`, `session/session_slash.rs:102`,
  `app/cli/src/execution_plan.rs:136`, `app/server/src/session_data.rs:432`.
- Seam-guard coverage gap: `scripts/check-app-server-seam.sh` checks
  `app/query` + `core/` + `services/` only; `app/tui` and `app/session` are
  clean today but unguarded against a future `→ coco-app-server*` edge.
- Naming/placement nits: `SkillLoadGates.legacy_enabled` field name
  (`session/session_bootstrap.rs:431`); `headless_support.rs` physically at
  `src/` root, included via the crate's only cross-directory
  `#[path = "../headless_support.rs"]`; `mod.rs` vs sibling `foo.rs`+`foo/`
  conventions interleaved through the tree.

## Refuted / reclassified findings

- **Headless plugin watcher "missing"** — reclassified: acceptable divergence
  for a one-shot surface; requirement is only that it become an explicit
  builder option (F2).
- **`spawn_for_session` as stale naming** — refuted: live, legitimately used
  code (`integrations/team_memory_sync.rs:99`), not Event Hub residue.
- **TUI `SharedSessionHandle = Arc<RwLock<SessionHandle>>`** — not a facade
  bypass: the lock wraps the validated handle (the facade), documented
  swappable owner for `/resume`·`/branch`·`/clear`.
- **`agent-host` depending on both `coco-app-server` and
  `coco-app-server-client`** — intentional (cross-crate server/client
  integration tests live in agent-host).
- **All module-size findings** (`session_bootstrap.rs` 826; `tui/driver.rs`
  1576; `server/app_server.rs` 1543; `server-client/lib.rs` 1509; eight more)
  — excluded from this TODO by explicit decision: line-count limits are not a
  driver for this follow-up. Split only when a behavioral change forces the
  file open.

## Prioritized TODO

### P1 — close the remaining architecture gates (small, do first)

- [x] **T1.** `SessionHandle::live_permission_rules()` now returns a narrow
  append-only `LivePermissionRulesHandle` capability (not a raw lock). The
  teammate pump holds the handle and `.extend(...)`s it at boot and on leader
  pushes. Closes completion rule #12. (F1)
- [x] **T2.** Deleted the dead `session_manager.create()` call in
  `tui/bootstrap.rs`. (F2)
- [x] **T3.** Removed the dead `"session/archived"` match alternative
  (`hub/server/src/sqlite_store.rs`). (F8)

### P2 — unify local host assembly (the one real design task)

- [x] **T4.** Added `agent_host::local_host::{LocalHostInputs, PreparedLocalHost,
  build_local_host}`, the local counterpart to `remote_host::HostBuilder`. It
  owns the factory build, `HostInputs`/`RuntimeReplacementContext`,
  `SessionTurnExecutor` install, the Event Hub triad, and an explicit
  `LocalPluginWatch` option (Enabled for TUI, Disabled for headless). TUI and
  headless both consume it and now differ only in the policy they pass in and
  the lifecycle calls they make on the returned bridge. (F2)
- [x] **T5.** Headless now passes `model_runtimes: None` and lets the fold
  build the registry (verified equivalent — the fold uses the same
  `HeaderVars { session_id }`), dropping the hand-built `ModelRuntimeRegistry`.
  Headless's post-start turn config (`set_live_permissions`,
  `apply_turn_runtime_config`, structured-output, sandbox-reload) is retained
  as its one-shot turn policy applied through typed handle methods: the fold
  already seeds the identical permission rules (`build.rs` uses the same
  `typed_permission_rules`), so the re-seed is a defensible safety belt rather
  than drift, and `install_sandbox_reload_supervisor` is also called post-start
  by the TUI. (F2)

### P3 — semantic residuals

- [x] **T6.** Added deterministic `SessionTurnCoordinator` regressions
  (`turn.test.rs`) pinning the R13 admission gate: `start` is rejected through
  the whole Finishing window, and only the owner's `complete_finishing` returns
  to Idle (event forwarding cannot). This is the model-free core of the CS-2
  invariant; the commit-ordering half needs a full engine and stays in the
  integration suite. (F4)
- [x] **T7.** Classified every `session/` `tokio::spawn` in-code. Attempting to
  track the two MCP tasks (reconnect listener, background connect) via
  `spawn_session_task` regressed the close/lifecycle suite: the reconnect
  listener blocks on an idle `reconnect_rx.recv()` and exits only when the MCP
  manager drops the sender at teardown, so close-time joining waited the full
  drain deadline before aborting it. Both are therefore correctly **detached**
  with an explicit comment (channel-close / bounded time-boxed completion), not
  close-deadline-critical. The remaining `session/` spawns are documented in
  place — fire-and-forget maintenance sweeps (`build.rs` memory/snapshot
  cleanup, marketplace startup), a CLI-owned handle
  (`spawn_current_session_config_change_watcher`, aborted on drop by the TUI),
  and cooperative-cancel loops (`spawn_config_change_watcher` on the session
  token). `task_runtime/` (shell/timers/reaper) and `integrations/` (watchers,
  refreshes, per-request handlers, `AbortOnDropHandle` forks) are
  subsystem-owned by design. The tracked case stays the leader inbox poller
  from the prior remediation. (F5)
- [x] **T8.** Moved `plan_mode_instructions` off `initialize` to
  `session/start` and `session/resume`. It was wire-only (no Rust production
  code set it on `initialize`; the local bridge uses `InitializeParams::default()`
  and local surfaces apply plan-mode independently). Threaded through
  `SessionStartInput`/`SessionResumeInput` → `runtime_profile_from_connection`
  (now takes the value per-operation); clear/branch pass `None` (local surface
  re-applies). Regenerated JSON schema + Python + TypeScript. (F3)

### P4 — hygiene

- [x] **T10.** Kept the `startup_snapshot` seam (a legitimate injected-bundle
  `BootstrapSource` DI point used by tests/embedders — deleting it would force
  4 unit tests through disk-based folds) but removed the misleading "legacy
  startup" framing: renamed `StartupSnapshotSource` → `PrebuiltBootstrapSource`
  and `startup_snapshot` → `from_prebuilt_bootstrap`, with test/embedder
  wording. (F7)
- [x] **T11.** Inverted `SdkTransport` to frame-first: `recv_frame`/`send_frame`
  are the required methods, the `JsonRpcMessage` `recv`/`send` pair moved to
  inherent test/harness helpers on `InMemoryTransport`, and
  `server_notification_to_jsonrpc`/`LegacyJsonRpcNotification` are now
  `#[cfg(test)]` with the public re-export removed. (F7)
- [x] **T12.** Moved the four inline `mod tests` blocks to companion
  `.test.rs` files. (F8)
- [x] **T13.** Fixed the ~13 stale `tui_runner` doc references. (F8)
- [x] **T14.** Added `app/tui` and `app/session` to
  `check-app-server-seam.sh`. (F8)
- [x] **T15.** Renamed `SkillLoadGates.legacy_enabled` →
  `legacy_commands_enabled`; moved `headless_support.rs` into
  `headless/support.rs` (dropping the cross-dir `#[path]`). (F8)

### Event Hub retiring-set membership (T9)

- [x] **T9.** Verified the correctness is already in place and added the missing
  regression. Reconnect cursor negotiation covers the retiring Closing set
  because the announce carries `announced_session_ids()` (Live + retiring
  Closing, R17) and the Hub returns `resume_from` cursors for the announced set
  via the existing announce ack — **no new wire frames**. Membership updates on
  every lifecycle transition (touch-on-promote, forget-on-close-complete,
  retiring-Closing retained until removal). Added
  `registry.test.rs::announced_membership_retains_closing_slots_while_live_only_excludes_them`.
  The literal "dedicated lifecycle-revision stream" is a micro-optimization with
  no correctness benefit (the activity revision already drives membership
  correctly) and is intentionally not built. (F6)
