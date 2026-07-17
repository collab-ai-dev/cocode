# Permission & Sandbox Hardening — Refactor Status & Handoff

Started: 2026-06-09 · Branches: `feat/tui` → `feat/sandbox` · Base: `1a0b37a32`
(squashed into #165).

**Status (2026-06-10): COMPLETE.** §4 baseline + §5 **T1–T6 all landed & gated**
on `feat/sandbox` (T1+T2 `d4efb65c84`; T3+T4 `34167a9a14`; T5 `7891454245`; T6
follow-on commit). The permission/sandbox hardening refactor is done — no §5
items remain. Full `just pre-commit` green at each step. Residual nice-to-haves
are noted inline (e.g. T1 per-subcommand path parity, T2 `allowManagedDomainsOnly`,
T3 modal-vs-count UX, T6 `insta` snapshot).

This is a **living handoff doc**: it carries the full context needed to resume
the permission/sandbox hardening refactor in a fresh session. It is a
fix-status / TODO doc, not an information owner — for crate internals see
[crate-coco-permissions.md](crate-coco-permissions.md) and
[crate-coco-sandbox.md](crate-coco-sandbox.md); historical audit rows live in
[audit-gaps.md](audit-gaps.md).

> **Resuming?** All §5 work (T1–T6) is ✅ done — this doc is now a historical
> record of what landed and why. §1–§3 hold the background/decisions/invariants;
> §5 documents each fix. Build/verify workflow is §6.

---

## 1. Background (重构背景)

A 12-dimension audit of coco-rs's `permission` + `sandbox` subsystems against
the `claude-code` TS reference (`/lyz/codespace/3rd/claude-code/src`) found one
dominant anti-pattern plus real decision bugs:

**"Ported-but-not-connected" (wired-but-dead).** Large amounts of correct logic
were ported from TS but **never invoked in production** (zero non-test callers).
Because the code *existed*, it looked done — but the behavior it was supposed to
produce never happened. The most severe instances:

- **Linux sandbox was 100% broken** — the bwrap inner stage re-execs
  `coco --apply-seccomp <mode> -- <cmd>`, but `main.rs` went straight to
  `Cli::parse()` with no arg0 intercept, so the inner process died on a clap
  "unexpected argument" error. Every sandboxed command on Linux failed.
- **The sandbox approval bridge was fully dead** — `check_path_async` /
  `check_network_async` / `set_approval_bridge` / `SdkSandboxApprovalBridge`
  had zero production callers; `SandboxApprovalRequired` was never emitted.
- **Two fail-open security holes** — a bash deny rule (`Bash(curl:*)`) was
  bypassable by compounding/wrapping/env-prefixing the command, and a dangerous
  classifier-bypass allow rule was never stripped on Auto-mode entry.

Plus fail-closed correctness bugs (central evaluator over-denying content-scoped
rules), unsafe read-only auto-approvals (`env`, `date -s`, `ps axe`, …), and
inert sandbox network filtering / violation reporting.

The work mirrors **observable TS behavior**, not a byte-for-byte port (Rust
idioms — typed enums, `Result`, snafu — are kept).

---

## 2. Audit & method

Process used (and the way to regenerate it): **audit → design → implement →
gate**, all via background workflows.

1. **Audit workflow** — 12 read-only dimension agents diffed coco vs TS, each
   adversarially verified; 14/14 verdicts confirmed. Output classified every
   gap by severity and `is_intentional_divergence`.
2. **Spec workflow** — 12 spec agents produced precise, TS-mirrored
   implementation specs (exact fns, algorithms, call-sites, tests). These specs
   are the source of truth for the remaining work.
3. **Implementation** — done **serially** (multiple groups touch the same hot
   files: `evaluate.rs`, `bash.rs`, `state.rs`, `session_runtime.rs`), verifying
   with `just quick-check` at each milestone.
4. **Gate** — `just pre-commit` (full nextest + clippy) before commit.

> **Spec files** were persisted under the session's workflow output. If gone,
> regenerate by re-running the audit + spec workflows (the audit prompt diffs
> each subsystem dimension vs TS; the spec prompt turns each gap into an exact
> change list). The gap IDs below (P1–P18, S1–S7) come from that audit.

---

## 3. Technical decisions & invariants (技术决策)

These are load-bearing — **do not "fix" or revert them** without re-reading the
rationale.

### 3.1 Intentional divergences (NOT bugs — don't flag/port)
Per the multi-provider port rules (see root `CLAUDE.md`):
- No `USER_TYPE='ant'` / GrowthBook gates (e.g. the auto-mode classifier is the
  ant-gated TS feature; coco keeps the seam but it's unreachable via the normal
  cycle — `auto_mode_available=false`).
- Anthropic cloud routes (Bedrock/Vertex/Foundry) are non-goals.
- Ultraplan paths skipped. Env vars renamed `CLAUDE_*` → `COCO_*`.
- coco's default `Tool::check_permissions` returns `Passthrough` (TS returns
  `allow`) — deliberate fail-secure choice.

### 3.2 "Force-ask that can't be overridden" = `ToolCheckResult::Ask` at step-1b
`PermissionEvaluator::evaluate_inner` consults `Tool::check_permissions` at
**step-1b** — after deny rules, but **before** allow rules (step 2), ask rules,
path-safety, and mode fallthrough. So an `Ask` returned from `check_permissions`
short-circuits with a `PermissionDecision::Ask` that **no allow rule and no
mode (incl. acceptEdits) can override** — exactly TS's "cannot be auto-allowed
by permission rules". Only `DontAsk` mode converts it to deny (TS parity). This
is how the bash path-safety gates (§4 [5]) are non-overridable.

### 3.3 Layering: `coco-permissions` now depends on `coco-shell` (Core→Exec)
Added for the bash rule decomposition (P2) — a **legal downward edge**
(Core depends on Exec; `core/tools` + `core/tool-runtime` already do). **`coco-shell`
must NOT depend on `coco-permissions`** (would cycle). Consequence: the bash
path-safety gates (P3/P4/P5/P15) live **in `coco-shell`** (pure helpers reusing
`coco-shell`'s own `check_dangerous_path`) and are orchestrated from
`core/tools/bash.rs` — NOT by having `coco-shell` call
`coco_permissions::is_dangerous_removal_path`.

### 3.4 Bash rule matching policy (P2)
`shell_rules::match_bash_rule(content, command, policy, case)`:
- `RuleMatchPolicy::DenyOrAsk` — strip ALL leading env vars (fixed point) +
  safe wrappers + redirections, then re-split per-subcommand (any segment
  match ⇒ match). A deny can't hide behind `FOO=1 …` / `timeout 5 …` / `a && …`.
- `RuleMatchPolicy::Allow` — compound guard: a prefix/wildcard allow rule never
  matches a compound command (can't widen an allow by chaining).
- `ShellCase::Insensitive` for PowerShell, `Sensitive` for Bash.

### 3.5 Auto-mode dangerous-rule strip (P1) — two mechanisms
- **(B) load-bearing security fix:** strip at **context-build** time —
  `tool_context.rs::build()` runs `strip_dangerous_allow_rules` when
  `live_mode == Auto`. Mandatory regardless of entry path.
- **(A) provenance:** Auto-entry stash in
  `apply_permission_mode_transition_to_app_state` (signature gained a
  `&PermissionRulesBySource` param — 6 callers updated) drives the
  "Exited Auto Mode" banner + restore.

### 3.6 acceptEdits no longer blanket-allows bash rm/mv/cp/sed
`ACCEPT_EDITS_COMMANDS` shrunk to `[mkdir, touch]`. Dangerous rm/sed are caught
by the force-ask gates (§3.2) which run *before* the acceptEdits check; a safe
`rm foo.txt` now defers to the rule pipeline rather than auto-allowing — more
TS-faithful (TS acceptEdits auto-accepts file-*edit* tools, not bash `rm`).

### 3.7 Sandbox bridge installed post-build via interior mutability
`SandboxState.approval_bridge: RwLock<Option<…>>`; installed AFTER
`SessionRuntime::build` via `sandbox_state().set_approval_bridge(...)` (SDK path
in `main.rs::run_sdk_mode`). This avoids churning `SessionRuntimeBuildOpts` /
`build_sandbox_state` signatures. `permission_checker()` propagates the bridge
so `check_path_async` is bridge-aware. Survives hot-reload (lives on the
persistent `Arc<SandboxState>`).

### 3.8 Sandbox is opt-in / default-off
`Feature::Sandbox` is default-off. The CRITICAL Linux break (S1) only bit users
who explicitly enabled the sandbox — but that's the security-conscious cohort
that most needs it.

---

## 4. Current status (当前状态) — committed in `1a0b37a32`

Full `just pre-commit` green (whole nextest suite + clippy). 59 files,
+2336/−313. Every item below has companion `.test.rs` coverage.

| Group | Gap | What landed | Severity |
|------|-----|-------------|----------|
| **[0]** | S1/S6 | `coco_sandbox::inner_stage::dispatch_or_continue` in `main.rs` before `Cli::parse`; `WindowsSandbox::available()=false`; Linux integ test | **CRITICAL** |
| **[4]** | P2 | `shell_rules::match_bash_rule` + `RuleMatchPolicy` + `strip_output_redirections`; coco-permissions→coco-shell dep | **HIGH (fail-open)** |
| **[7]** | P1 | `strip_dangerous_allow_rules` at `tool_context.rs` build + Auto-entry stash; `apply_permission_mode_transition_to_app_state` +rules param | **HIGH (fail-open)** |
| **[6]** | P13 | `evaluate.rs::central_rule_applies` (content denies defer to tool); `AgentTool` agentType deny enforcement | **HIGH (fail-closed)** |
| **[3]** | P6/P7/P11 | `read_only.rs` `ReadOnlyRule` table (env/date/ps/sort/pagers removed/validated) | HIGH |
| **[8]** | P9/P10 | `normalize_legacy_tool_name` in `rule_compiler.rs`; `ShellCase` PowerShell case-insensitive | HIGH |
| **[10]** | P14 | `PermissionEvaluationOptions.sandbox_auto_allow_bash` + `tool_call_preparer` wiring | MED |
| **[9]** | P12 | `filesystem.rs` `.claude/worktrees` component match, `/tmp` blanket-allow removed, `.claude/commands` | MED |
| **[1]** | S2/S3 | `SandboxState::start_network_proxy_with_bridge` (netns socat `BridgeManager`); `inner_command_prefix` in executor; `deps::socat_path`; ProxyRouted seccomp allows AF_UNIX; cross-platform `session_runtime` wiring | HIGH (fail-closed) |
| **[2]** | S4/S5/S7 | **CORE:** SDK `SdkSandboxApprovalBridge` installed on live state; `sandbox_preflight`→`check_path_async`; Linux executor records SIGSYS `seccomp_violation`; checker.rs doc fixed | partial — see §5 |
| **[5]** | P4/P5/P15 | **CORE:** force-ask gates in `bash.rs::check_permissions` (dangerous-removal, sed-danger, git-escape); `is_read_only` git-escape; `ACCEPT_EDITS_COMMANDS=[mkdir,touch]` | partial — see §5 |
| **[11]** | P8/P17/P18 | managed-rules enforcement (`typed_permission_rules` policy-only filter); `seed_session_additional_dirs` (--add-dir + settings); `is_running_as_root` (`geteuid`) | partial — see §5 |

**Note (S2 correction):** the audit's "proxy-env never injected" was imprecise —
injection already worked via `try_wrap_command_with_binds` (`snap.proxy_env`).
The real S2/S3 fix was starting the proxy on Linux + the netns bridge.

---

## 5. TODO — remaining work (prioritized)

Ordering reflects the directive: **finish internal implementation first; TUI/UI
surfaces are lowest priority.** Line numbers will drift — search by function name.

**Done (2026-06-10): all of T1–T6.** P0 internal (T1, T2), P1 internal (T3, T4),
and P2 TUI/UI (T5, T6) all landed and gated — see the ✅ sections below for what
each one did. Nothing in §5 remains open.

**Residual follow-ups — also resolved (2026-06-10).** The inline "nice-to-have"
notes were all closed in a follow-up pass:
- **T1 per-subcommand write-path validation** — a write command
  (`rm`/`rmdir`/`mv`/`cp`/`touch`/`mkdir`) targeting a path outside the allowed
  dirs now force-asks (new `coco_shell::extract_write_path_targets`, env+wrapper
  stripped; bash.rs checks each via `is_path_within_allowed_dirs` /
  `is_editable_internal_path`). Reads stay UNFENCED — a deliberate coco
  divergence (gating `ls ..`/`cat ../x` is too noisy; reads are non-destructive,
  fenced by the Read tool + kernel sandbox). Dead `coco_shell::check_redirect_paths`
  deleted.
- **T2 `allow_managed_domains_only`** — now gates the network ask-callback
  (`build_network_ask_callback` returns `None` under the policy, so denied hosts
  get a static refusal with no interactive widening). The host-blind, uncalled
  `check_network_async` was deleted (the proxy callback is the real surface).
- **T3 UX** — the per-burst blocking Error modal is replaced by a non-blocking
  toast (TS shows a count surface, not a modal).
- **T6** — `insta` snapshot of the open explainer panel added.

### P0 — Internal, completes a security/parity behavior

#### T1. Bash redirect / out-of-tree path-constraint gate ([5] P3) — ✅ DONE (2026-06)
Landed: a 4th force-ask gate in `bash.rs::check_permissions` (after
`check_git_escape`, before the acceptEdits check). Process substitution
(`<(…)` / `> >(…)`) → `Ask`; for each output-redirect target (skip
`/dev/null`), `has_shell_expansion(target)` → `Ask`, else
`!is_path_within_allowed_dirs(target, bash_gate_cwd(ctx), additional_dirs)`
→ `Ask`. Two new **pure** `coco-shell` helpers do the parsing (quote-aware,
mirroring TS `extractOutputRedirections` / `checkPathConstraints`):
`bash_permissions::extract_output_redirect_targets` and
`has_process_substitution`; the gate (in `core/tools`) calls them plus
`coco_permissions::{has_shell_expansion, is_path_within_allowed_dirs}`. Tests:
`bash.test.rs` (5 gate cases) + `bash_permissions.test.rs` (5 parser cases).
- **Scope note:** minimal P3 = redirects + process-sub only. TS
  `checkPathConstraints` is **broader** — it also validates per-subcommand path
  args (`cat /etc/shadow` outside tree → Ask) via `validateSinglePathCommand`.
  That broader per-command path-bound validation is **not** ported and is an
  untracked follow-up if full parity is wanted.
- **Dead code left in place:** `coco_shell::check_redirect_paths`
  (`path_validation.rs:76`) is unused + system-path-only — not wired here (the
  gate uses the new extractor). Delete when convenient.

#### T2. Sandbox network ask-callback ([2], `check_network_async`) — ✅ DONE (2026-06)
Landed: `NetworkAskCallback` (host-only, `proxy/mod.rs`) threaded as
`Option<NetworkAskCallback>` through `ProxyServer::start_with_ports` →
`run_http_proxy`/`run_socks_proxy` → `handle_http_connect`/`handle_socks5`. On
a denied host the handler `await`s the callback before the 403 / SOCKS5
REFUSED; an approving decision tunnels the connection.
`state.rs::build_network_ask_callback` constructs it from the installed
`approval_bridge` (`SandboxOperation::Network`, `request_approval == Approved`
→ `true`) and passes it at both `start_network_proxy` and
`start_network_proxy_with_bridge`. Tests: `server.test.rs` (HTTP approve/reject
+ SOCKS approve).
- **Follow-ups:** (a) `check_network_async` (checker.rs) is still host-blind +
  uncalled — the proxy path is now the real surface; (b) `allowManagedDomainsOnly`
  (TS hard-deny wrapper) has **no** coco config representation, so it is not
  gated; (c) the prompt only fires under the SDK today — interactive TUI needs
  T5 (`TuiSandboxApprovalBridge`) installed before a denied CONNECT can prompt.

### P1 — Internal, completes feedback/reporting

#### T3. Violation pipeline: monitor spawn + observer → event ([2] S5) — ✅ DONE (2026-06)
Landed, with a **lighter design than originally specced**: instead of a
`SandboxRuntimeBundle` + `SessionRuntime` field/Drop, the monitor is owned by
`SandboxState` itself (`monitor: Mutex<Option<ViolationMonitor>>`), so its
lifetime ties to the `Arc<SandboxState>` — no `SessionRuntime` changes.
- `state.rs::build` now uses `ViolationStore::with_observer()` and stores the
  `UnboundedReceiver<i32>` in `violation_observer` (take-once via
  `take_violation_observer`).
- `monitor.rs` gained a sync `cancel()` + a `Drop` that cancels (winds down the
  macOS `log stream` child via `kill_on_drop`).
- `state.rs::start_violation_monitor()` (idempotent; no-op unless
  `platform_active`) spawns the platform monitor and retains it.
- `tui_runner.rs` (after the sandbox hot-reload block) calls both and spawns a
  **cross-platform** drain task that forwards the observer count as
  `CoreEvent::Protocol(ServerNotification::SandboxViolationsDetected { count })`.
  The drain self-terminates when the `SandboxState` (observer sender) drops.
  Producer is **TUI-only** — the SDK path surfaces violations to the model via
  `<sandbox_violations>`, and the consumer lives in coco-tui. Tests:
  `state.test.rs` (observer take-once + count delivery; inactive no-op guard).
- **Follow-up (UX):** the TUI consumer (`protocol.rs:713`) shows a blocking
  Error modal per violation burst — possibly noisy vs TS's count surface.

#### T4. LLM permission-explainer logic ([11] P16, internal half) — ✅ DONE (2026-06)
Landed the **internal half** (see the DECISION note below for the UI half):
- `PermissionsConfig` gained `permission_explainer_enabled: Option<bool>` +
  `explainer_enabled()` (default-on, `!= Some(false)`; TS `permissionExplainerEnabled`).
- `TuiPermissionBridge::explain_risk(params) -> Option<PermissionExplanation>`
  upgrades the `Weak<SessionRuntime>`, gates on the flag, runs
  `generate_permission_explanation` via `runtime.side_query()` (the
  already-wired handle) under an 8s timeout, graceful-degrades to `None`.
- **No production caller yet** — `explain_risk` is `pub` (no dead-code warning),
  awaiting the **T6 Ctrl+E lazy panel** caller. Deliberate internal/UI split.
  Test: `tui_permission_bridge.test.rs` (no-runtime → `None`).
> **DECISION (2026-06):** the explainer (T4 internal + T6 UI) will mirror TS's
> **Ctrl+E lazy panel** (`confirm:toggleExplanation`, fetched on first toggle),
> NOT the current always-on title badge — TS only shows risk in the on-demand
> panel and fetches lazily to avoid an LLM call per prompt. So T6's render path
> (already-built always-on badge) is to be **reworked into the panel**, and the
> explainer is fetched lazily, not eagerly on every `ApprovalRequired`.
> **Also corrected:** the doc's "highest uncertainty" SideQuery-handle concern
> is already solved — `SessionRuntime::side_query()` is real + production-wired
> (used by `session_rename.rs`, `tui_runner.rs`); source the handle there, NOT
> from `ToolUseContext.side_query` (NoOp by default).

### P2 — TUI / UI surfaces (landed last; both ✅ done)

#### T5. Interactive TUI sandbox approval bridge ([2]) — ✅ DONE (2026-06)
Landed: `app/cli/src/sandbox_approval_bridge_tui.rs::TuiSandboxApprovalBridge`
installed in `tui_runner` (alongside the violation monitor). It registers a
fresh-`Uuid` entry in the SAME `PendingApprovals` map the tool-permission bridge
uses and emits `TuiOnlyEvent::SandboxApprovalRequired`, so the existing
`UserCommand::ApprovalResponse` arm resolves it with no tui_runner change
(sandbox prompts carry no `permission_updates`). Translates the resolved
`ToolPermissionDecision` → `SandboxApprovalDecision`; fail-closed (Rejected) on a
closed notification/response channel. Unblocks T2's network ask-callback in
interactive mode (was SDK-only). Tests: `sandbox_approval_bridge_tui.test.rs`
(approve / deny / fail-closed).

#### T6. Risk explainer — Ctrl+E lazy panel ([11] P16, UI half) — ✅ DONE (2026-06)
Implemented as the **TS Ctrl+E lazy panel** (per the §5-T4 DECISION), NOT the
always-on title badge. Flow: `ConfirmToggleExplanation` (Ctrl+E, Confirmation
context) → repointed to `TuiCommand::TogglePermissionExplanation` → toggles
`PermissionPromptState.explanation_visible`; on first open sends
`UserCommand::RequestPermissionExplanation { request_id, tool_name, tool_input }`
→ `tui_runner` spawns `SessionRuntime::explain_permission_risk` (the new single
home for the explainer call; the T4 bridge `explain_risk` now delegates here) →
emits `TuiOnlyEvent::PermissionExplanationReady { request_id, explanation }` →
`tui_only.rs` lands it on the active prompt's `ExplainerFetch`
(`NotFetched`/`Loading`/`Ready`/`Unavailable`). `presentation/request.rs` renders
the panel only when open (default body byte-unchanged → no test churn) with the
risk reflected in both the panel text and the border; new i18n keys in
`en.yaml`/`zh-CN.yaml`. **This is T4's first production caller** — the explainer
is no longer dead code. The legacy always-on `risk_level` title badge stays
inert (never populated in prod). Tests: render (3), event handler (3), toggle
dispatch (2). UI-only `insta` snapshot of the open panel is a nice-to-have
follow-up (current tests assert body substrings + border).

---

## 6. Build / verify / commit workflow

Run from `coco-rs/`.
- Iterate: `just quick-check` (fmt + seam guards + incremental clippy; ~30s–2m).
  **Do not run mid-edit spam** — run at milestones.
- Final gate (REQUIRED before commit, run ONCE): `just pre-commit` (full nextest
  + clippy). It is orders of magnitude slower; run it long-running/background and
  **check the real exit code** — a `| tail` pipe masks failures (this bit us
  once; the suite had 2 failures behind a `tail` exit 0).
- Conventional Commits; end the message with
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Never auto-commit; wait for explicit user "commit".

**Test-only gotchas seen:** making a sync fn async breaks its companion test
(`#[test]`→`#[tokio::test]` + `.await`); a naive whitespace splitter shreds
quoted sed scripts (use a quote-aware split). Both already fixed.

---

## 7. References

- TS reference source: `/Users/linyuzhi/codespace/myagent/agents/claude-code-kim/src`
  (the `/lyz/codespace/3rd/claude-code/src` path in older notes is stale).
- Crate internals: [crate-coco-permissions.md](crate-coco-permissions.md),
  [crate-coco-sandbox.md](crate-coco-sandbox.md),
  [crate-coco-shell.md](crate-coco-shell.md),
  [bypass-permissions.md](bypass-permissions.md).
- Historical audit log: [audit-gaps.md](audit-gaps.md);
  fix-ordering: [current-gap-fix-plan.md](current-gap-fix-plan.md).
- Auto-memory: `project_permission_sandbox_dead_wiring.md` (the audit findings +
  fix status, cross-session).
