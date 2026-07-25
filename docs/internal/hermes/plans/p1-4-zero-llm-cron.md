# P1-4 — Zero-LLM Scheduled Jobs (Script-Only Cron)

Status: **IMPLEMENTED 2026-07-25** (`feat/hermes`) · Size: M · Owner crates:
`coco-tool-runtime` (job model), `coco-tools` (schema),
`coco-agent-host` (tick driver + execution), `coco-config` (knobs),
`coco-shell` (`remove_env` seam), `coco-tui` (delivery surface)

## As-built deltas from this plan

- **`CronPayload` lives in `coco-tool-runtime`, not `utils/coco-cron`.**
  `coco-cron` is timing-only by design (`CronTiming` carries no payload); the
  owning record is `coco_tool_runtime::CronTask`, so the payload went there.
  `ScheduleStore::add_cron_task` now takes `payload: CronPayload` instead of
  `prompt: &str`.
- **Migration-free via `#[serde(flatten)]` + `untagged`.** A record with
  `script` is a script job, one with `prompt` is a prompt job; pre-existing
  `{"prompt": …}` files load as `Prompt` with no upgrade pass. A hand-written
  script record without `onOutput` defaults to `Notify`, never `WakeAgent`.
- **Delivery surface = the `!command` transcript row.** `Notify` emits
  `TuiOnlyEvent::CronScriptResult`, and the TUI pushes
  `SystemPushKind::LocalCommand { command, output }` — the same durable
  transcript row bash-mode uses — plus a toast on failure. A toast alone would
  have lost the output, and writing history directly from the tick driver would
  race `commit_engine_turn_history` (which replaces the whole history).
- **Scripts run detached, with a per-job overlap guard.** Running inline would
  let one 120 s command stall the 1 s tick for every other job. A job still
  running when its next fire comes due skips that fire (warn-logged) instead of
  stacking a second process.
- **Creation-time gating rejects `Deny`-severity security checks and
  destructive commands only.** `Ask`-severity findings (pipes, redirects) are
  normal in monitoring scripts and there is no interactive prompt to route them
  to, so they are allowed.
- **Env scrub is data-driven**, not a hardcoded list: every configured
  provider's `ProviderConfig.env_key` plus `ANTHROPIC_API_KEY` /
  `ANTHROPIC_AUTH_TOKEN`, stripped through the new
  `coco_shell::ExecOptions.remove_env` (applied after `extra_env`, so a name in
  both is removed).
- **Two knobs, not one**: `scheduling.script_timeout_secs` (default 120) and
  `scheduling.script_output_max_bytes` (default 16000, char-boundary-safe
  truncation with a marker).
- **Deferred as planned**: hermes's in-band `wakeAgent` JSON gate on the last
  stdout line, pre-run script injection into a prompt job's context, and
  messaging-platform delivery.

## Problem

Every cron firing starts an agent turn. `CronCreateInput` has only
`cron` / `prompt` / `recurring` / `durable`
(`core/tools/src/tools/scheduling.rs:80-102`); the scheduler always
"enqueue[s] the task's prompt" (`utils/coco-cron/src/scheduler.rs:6,31`)
onto the session `CommandQueue` with `QueueOrigin::Cron`, waking the
agent driver (`app/cli/src/cron_tick.rs:5-10,177-191`). The common
"poll something, tell me only if it changed" monitoring job burns a full
LLM turn per tick even when nothing changed.

## Hermes evidence (hermes-agent @ `a7f65e3bc`)

Releases v2026.5.7 #19709/#19628, v2026.4.23 #12373, v2026.4.8 #5082,
v2026.5.16 #21881 (`watchers` skill built on this).

- **Dispatch** — `cron/scheduler.py:2523` `if job.get("no_agent"):`,
  short-circuit block :2506-2606. Semantics (comment :2516-2522):

  ```
  - script stdout (trimmed) → delivered verbatim as the final message
  - empty stdout            → silent run (no delivery, success=True)
  - non-zero exit / timeout → delivered as an error alert, success=False
  - wakeAgent=false gate    → treated like empty stdout (silent)
  ```

  "the script IS the job, no LLM involvement" (:2506); agent machinery
  is imported only *after* this block (:2608-2614) so `no_agent` ticks
  never pay agent construction.
- **wakeAgent gate** — `_parse_wake_gate` (:2135-2160): parses the last
  non-empty stdout line as JSON;
  `return gate.get("wakeAgent", True) is not False` (:2160). Also used
  by LLM jobs as a pre-run gate (:2629-2645: "run the pre-check script
  BEFORE building the prompt").
- **Pre-run script injection** —
  `prompt = _build_job_prompt(job, prerun_script=…)` (:2648): script
  output injected into the agent prompt (cheap data collection before
  spending tokens).
- **Job field** — `cron/jobs.py:977` `no_agent: bool = False`; doc
  :996-999: "With `no_agent=True` the script IS the job — its stdout is
  delivered verbatim. Without `no_agent`, its stdout is injected into
  the agent's prompt as context".
- Env hygiene (release v2026.6.19 #49207): job-script subprocesses run
  with sanitized env (no inherited provider credentials).

## Design

Enum over bool (repo rule) — a job is one of two kinds:

```rust
// utils/coco-cron job model
pub enum CronPayload {
    /// Today's behavior: enqueue prompt as an agent turn.
    Prompt { prompt: String },
    /// Zero-LLM: run the script; deliver stdout only when non-empty.
    Script { command: String, on_output: ScriptOutputAction },
}

pub enum ScriptOutputAction {
    /// Surface stdout as a TUI/system notification only (no agent turn).
    Notify,
    /// Enqueue an agent turn with stdout attached as context.
    WakeAgent,
}
```

Semantics (mirror hermes exactly):

- empty stdout (after trim) → silent success, recorded in the job log;
- non-empty stdout → per `on_output`: `Notify` = user-facing notice
  (TUI notification / headless log line), `WakeAgent` = enqueue via the
  existing `QueueOrigin::Cron` path with stdout as the prompt context;
- non-zero exit / timeout → always surfaced as an error notice
  (never silent), `success=false` in the job record.

Execution details:

- Run through the existing shell executor (`exec/shell`) with:
  sanitized env (strip provider credentials — reuse the env-scrub list
  from the coordinator spawn work), the session cwd, a hard timeout
  (config `scheduling.script_timeout_secs: i64`, default 120), and
  stdout capped via the standard output truncation before delivery.
- Tick driver (`cron_tick.rs`): branch on `CronPayload` **before** any
  agent/session wake — Script jobs must not touch the agent driver at
  all in the `Notify` + empty-stdout cases (hermes's "imported only
  after this block" property, translated).
- Tool schema (`scheduling.rs`): add `script` + `on_output` fields to
  `CronCreateInput`, mutually exclusive with `prompt` (validate: exactly
  one of `prompt` / `script`). Update the tool description so the model
  knows monitoring jobs should be scripts.
- Permissions: creating a Script job stores a shell command that will
  run unattended — route the command through the same shell security
  analysis used for Bash at creation time, and record it in the job for
  audit. Deferred v2: hermes's `wakeAgent` JSON gate on the last stdout
  line (start with the two-action enum; the in-band gate adds parsing
  ambiguity for little gain).

## Implementation steps

1. `CronPayload` in `utils/coco-cron` (serde `#[serde(tag = "type")]`),
   migration-free: existing persisted jobs deserialize as `Prompt` via
   an untagged fallback or a one-time load-time upgrade (check the
   persistence format first — no compat shims beyond deserialization).
2. Tick-driver branch + shell execution + env scrub + timeout.
3. Tool schema + validation + description.
4. Notification surface: reuse the existing TUI notification path
   (whatever `/cron` results and task notifications use today).
5. `just test-crate coco-cron` + `coco-tools`; an integration test with
   a script that alternates empty/non-empty stdout.

## Tests

- Script job, empty stdout → no agent wake, no notification, success
  recorded.
- Non-empty stdout + `Notify` → notification carries stdout verbatim
  (truncated per budget); no agent turn.
- Non-empty stdout + `WakeAgent` → one queued turn with stdout attached.
- Non-zero exit → error notice even with empty stdout.
- Timeout → killed, error notice, next tick unaffected.
- `prompt`+`script` both set → validation error at create time.

## Risks / non-goals

- Unattended shell execution is a real capability grant — mitigated by
  create-time security analysis + audit record; document in the tool
  description.
- Non-goals: hermes's messaging-platform delivery (coco surfaces are
  TUI/headless/SDK); cron `context_from` job chaining; blueprints.
