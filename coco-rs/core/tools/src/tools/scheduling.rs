//! Scheduling tools: CronCreate, CronDelete, CronList, RemoteTrigger.
//!
//! Uses `ctx.schedules` (ScheduleStore trait) for persistence.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::Feature;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

/// Maximum simultaneous cron jobs allowed by `CronCreate`. `MAX_JOBS = 50`.
/// Enforced in `validate_input` so the model gets a clear error before the
/// store rejects.
const MAX_CRON_JOBS: usize = 50;

/// Recurring jobs auto-expire after this many days. `DEFAULT_MAX_AGE_DAYS = 7`.
/// Surfaced in the model-visible confirmation so the model can warn the user.
const DEFAULT_MAX_AGE_DAYS: u32 = 7;

/// Lightweight 5-field cron expression validator. Accepts standard cron
/// syntax minus extension features (no L/W/# qualifiers) and rejects
/// obviously malformed inputs at the tool boundary.
///
/// Returns `true` if the expression is well-formed (5 fields, each
/// matching the simple grammar). Returns `false` for any structural
/// or per-field violation.
///
/// Grammar per field:
///   - `*` (any value)
///   - `N` (literal number)
///   - `*/N` (every N units)
///   - `A-B` (range)
///   - `A,B,C` (list — each element follows the same rules above)
///
/// Errs on the side of accepting valid inputs and only catching obvious
/// typos like `*/5/*/*` or 4-field expressions.
fn is_valid_cron_expression(expr: &str) -> bool {
    // Faithful, range-aware parse via the shared `coco-cron` crate.
    // Stricter than the prior local validator — it also rejects
    // out-of-range values (e.g. `60 * * * *`).
    coco_cron::is_valid_cron_expression(expr)
}

/// Current wall-clock in epoch ms (for cron reachability checks). Sync so it
/// can be used from `validate_input`.
fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Typed input for [`CronCreateTool`].
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct CronCreateInput {
    /// Standard 5-field cron expression in local time: "M H DoM Mon
    /// DoW" (e.g. "*/5 * * * *" = every 5 minutes, "30 14 28 2 *" =
    /// Feb 28 at 2:30pm local once).
    ///
    /// REQUIRED — no `default:""`.
    pub cron: String,
    /// The prompt to enqueue at each fire time.
    ///
    /// REQUIRED.
    pub prompt: String,
    /// true (default) = fire on every cron match until deleted or
    /// auto-expired after 7 days. false = fire once at the next match,
    /// then auto-delete. Use false for "remind me at X" one-shot
    /// requests with pinned minute/hour/dom/month.
    #[serde(default = "default_true")]
    pub recurring: bool,
    /// true = persist to .coco/scheduled_tasks.json and survive
    /// restarts. false (default) = in-memory only, dies when this Claude
    /// session ends. Use true only when the user asks the task to
    /// survive across sessions.
    #[serde(default)]
    pub durable: bool,
}

fn default_true() -> bool {
    true
}

/// Typed output for [`CronCreateTool`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronCreateOutput {
    pub id: String,
    /// Human-readable cron schedule (currently echoes back the cron
    /// expression). Field name preserved as `humanSchedule` for
    /// wire compatibility.
    #[serde(rename = "humanSchedule")]
    pub human_schedule: String,
    pub recurring: bool,
    pub durable: bool,
    pub status: String,
}

pub struct CronCreateTool;

#[async_trait::async_trait]
impl Tool for CronCreateTool {
    type Input = CronCreateInput;
    coco_tool_runtime::impl_runtime_schema!(CronCreateInput);
    type Output = CronCreateOutput;

    fn to_auto_classifier_input(&self, input: &CronCreateInput) -> Option<String> {
        Some(format!("{}: {}", input.cron, input.prompt))
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::CronCreate)
    }
    fn name(&self) -> &str {
        ToolName::CronCreate.as_str()
    }
    /// Hidden from the model unless the cron feature is on — without a real
    /// [`ScheduleStore`](coco_tool_runtime::ScheduleStore) the tool only
    /// fails, so advertising it would waste model turns.
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::AgentTriggers)
    }
    /// Always exposes the durable path via `.coco/scheduled_tasks.json`.
    fn description(&self, _input: &CronCreateInput, _options: &DescriptionOptions) -> String {
        "Schedule a prompt to run at a future time — either recurring on a cron schedule, or once at a specific time. Pass durable: true to persist to .coco/scheduled_tasks.json; otherwise session-only.".into()
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("schedule a recurring or one-shot prompt")
    }

    /// Full model-facing description. Engine builds the tool
    /// def from `prompt()`, not `description()` (`engine_prompt.rs`), so
    /// this is what the model actually reads.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        format!(
            "Schedule a prompt to be enqueued at a future time. Use for both recurring schedules and one-shot reminders.\n\
\n\
Uses standard 5-field cron in the user's local timezone: minute hour day-of-month month day-of-week. \"0 9 * * *\" means 9am local — no timezone conversion needed.\n\
\n\
## One-shot tasks (recurring: false)\n\
\n\
For \"remind me at X\" or \"at <time>, do Y\" requests — fire once then auto-delete.\n\
Pin minute/hour/day-of-month/month to specific values:\n\
  \"remind me at 2:30pm today to check the deploy\" → cron: \"30 14 <today_dom> <today_month> *\", recurring: false\n\
  \"tomorrow morning, run the smoke test\" → cron: \"57 8 <tomorrow_dom> <tomorrow_month> *\", recurring: false\n\
\n\
## Recurring jobs (recurring: true, the default)\n\
\n\
For \"every N minutes\" / \"every hour\" / \"weekdays at 9am\" requests:\n\
  \"*/5 * * * *\" (every 5 min), \"0 * * * *\" (hourly), \"0 9 * * 1-5\" (weekdays at 9am local)\n\
\n\
## Avoid the :00 and :30 minute marks when the task allows it\n\
\n\
Every user who asks for \"9am\" gets `0 9`, and every user who asks for \"hourly\" gets `0 *` — which means requests from across the planet land on the API at the same instant. When the user's request is approximate, pick a minute that is NOT 0 or 30:\n\
  \"every morning around 9\" → \"57 8 * * *\" or \"3 9 * * *\" (not \"0 9 * * *\")\n\
  \"hourly\" → \"7 * * * *\" (not \"0 * * * *\")\n\
  \"in an hour or so, remind me to...\" → pick whatever minute you land on, don't round\n\
\n\
Only use minute 0 or 30 when the user names that exact time and clearly means it (\"at 9:00 sharp\", \"at half past\", coordinating with a meeting). When in doubt, nudge a few minutes early or late — the user will not notice, and the fleet will.\n\
\n\
## Durability\n\
\n\
By default (durable: false) the job lives only in this Claude session — nothing is written to disk, and the job is gone when Claude exits. Pass durable: true to write to .coco/scheduled_tasks.json so the job survives restarts. Only use durable: true when the user explicitly asks for the task to persist (\"keep doing this every day\", \"set this up permanently\"). Most \"remind me in 5 minutes\" / \"check back in an hour\" requests should stay session-only.\n\
\n\
## Runtime behavior\n\
\n\
Jobs only fire while the REPL is idle (not mid-query). Durable jobs persist to .coco/scheduled_tasks.json and survive session restarts — on next launch they resume automatically. One-shot durable tasks that were missed while the REPL was closed are surfaced for catch-up. Session-only jobs die with the process. The scheduler adds a small deterministic jitter on top of whatever you pick: recurring tasks fire up to 10% of their period late (max 15 min); one-shot tasks landing on :00 or :30 fire up to 90 s early. Picking an off-minute is still the bigger lever.\n\
\n\
Recurring tasks auto-expire after {DEFAULT_MAX_AGE_DAYS} days — they fire one final time, then are deleted. This bounds session lifetime. Tell the user about the {DEFAULT_MAX_AGE_DAYS}-day limit when scheduling recurring jobs.\n\
\n\
Returns a job ID you can pass to CronDelete."
        )
    }

    /// Render the create envelope as a single-line confirmation.
    fn render_for_model(&self, out: &CronCreateOutput) -> Vec<ToolResultContentPart> {
        let id = &out.id;
        let schedule = &out.human_schedule;
        // The scheduler (coco_cli::cron_tick) fires the prompt on schedule in
        // the interactive session; durable=true also persists to
        // .coco/scheduled_tasks.json so a later session picks it up.
        let where_str = if out.durable {
            "persisted to .coco/scheduled_tasks.json"
        } else {
            "session-only (not written to disk)"
        };
        let text = if out.recurring {
            format!(
                "Scheduled recurring job {id} ({schedule}), {where_str}. Auto-expires after {DEFAULT_MAX_AGE_DAYS} days; use CronDelete to cancel sooner."
            )
        } else {
            format!(
                "Scheduled one-shot task {id} ({schedule}), {where_str}. It fires once, then auto-deletes."
            )
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    /// Pre-flight checks for cron syntax, next-run reachability within the
    /// next year, and the global MAX_JOBS cap. Syntax + reachability use the
    /// shared `coco-cron` parser (`parse_cron_expression` / `next_cron_run_ms`);
    /// the MAX_JOBS check runs in `execute` against the schedule store.
    fn validate_input(&self, input: &CronCreateInput, ctx: &ToolUseContext) -> ValidationResult {
        if input.cron.is_empty() {
            return ValidationResult::invalid("cron parameter is required");
        }
        if input.prompt.is_empty() {
            return ValidationResult::invalid("prompt parameter is required");
        }
        if !is_valid_cron_expression(&input.cron) {
            return ValidationResult::invalid(format!(
                "Invalid cron expression '{cron}'. Expected 5 fields: M H DoM Mon DoW.",
                cron = input.cron
            ));
        }
        // Next-run reachability check (errorCode 2): reject syntactically-valid
        // but never-firing expressions like `30 14 30 2 *` (Feb 30) before any
        // side effect.
        if coco_cron::next_cron_run_ms(&input.cron, now_epoch_ms()).is_none() {
            return ValidationResult::invalid(format!(
                "Cron expression '{cron}' has no scheduled run within the next year.",
                cron = input.cron
            ));
        }
        // Durable crons persist to disk; an in-process teammate's agent id does
        // not survive the session, so a durable cron created by one would orphan.
        // Gates on `is_in_process_teammate`, not on `agent_id` (regular subagents
        // have that set).
        if input.durable && ctx.is_in_process_teammate {
            return ValidationResult::invalid(
                "durable crons are not supported for teammates (teammates do not persist across sessions)",
            );
        }
        ValidationResult::Valid
    }

    async fn execute(
        &self,
        input: CronCreateInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<CronCreateOutput>, ToolError> {
        // R7-T22: enforce the global cron-job cap. Rejects when there are
        // >= 50 active jobs; queries the schedule store for the current count.
        // The check happens in execute (not validate_input) because it's an
        // async store hit, and validate_input is sync. Failing here surfaces
        // as a tool error to the model.
        if let Ok(existing) = ctx.schedules.list_all_cron_tasks().await
            && existing.len() >= MAX_CRON_JOBS
        {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "Cron job limit reached ({MAX_CRON_JOBS}). Delete an existing job before creating a new one."
                ),
                display_data: None,
                source: None,
            });
        }

        // Persist via the store: durable → disk (.coco/scheduled_tasks.json),
        // else session-only. The scheduler tick (coco_cli::cron_tick) picks it
        // up and fires the prompt.
        match ctx
            .schedules
            .add_cron_task(
                &input.cron,
                &input.prompt,
                input.recurring,
                input.durable,
                // Only an in-process teammate stamps its agent id. A regular
                // subagent's session-only cron must not persist a stale agent id.
                if ctx.is_in_process_teammate {
                    ctx.agent_id.as_ref().map(coco_types::AgentId::as_str)
                } else {
                    None
                },
            )
            .await
        {
            Ok(task) => Ok(ToolResult {
                data: CronCreateOutput {
                    id: task.id,
                    human_schedule: coco_cron::cron_to_human(&task.cron),
                    recurring: input.recurring,
                    durable: input.durable,
                    status: "created".into(),
                },
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            }),
            Err(e) => Err(ToolError::ExecutionFailed {
                message: format!("Failed to create schedule: {e}"),
                display_data: None,
                source: None,
            }),
        }
    }
}

/// Typed input for [`CronDeleteTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CronDeleteInput {
    /// Job ID returned by CronCreate.
    ///
    /// Wire/schema key is `id` (REQUIRED); the Rust field stays `schedule_id`
    /// to avoid colliding with the `id()` tool-name method.
    #[serde(rename = "id")]
    pub schedule_id: String,
}

/// Typed output for [`CronDeleteTool`].
///
/// All fields marked `#[serde(default)]` so test fixtures and
/// transcript replay that provide partial envelopes (e.g.
/// `{"id": "x"}`) still deserialize into a valid struct for
/// `render_for_model`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronDeleteOutput {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub status: String,
}

pub struct CronDeleteTool;

#[async_trait::async_trait]
impl Tool for CronDeleteTool {
    type Input = CronDeleteInput;
    coco_tool_runtime::impl_runtime_schema!(CronDeleteInput);
    type Output = CronDeleteOutput;

    fn to_auto_classifier_input(&self, input: &CronDeleteInput) -> Option<String> {
        Some(input.schedule_id.clone())
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::CronDelete)
    }
    fn name(&self) -> &str {
        ToolName::CronDelete.as_str()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::AgentTriggers)
    }
    fn description(&self, _input: &CronDeleteInput, _options: &DescriptionOptions) -> String {
        "Cancel a scheduled cron job by ID".into()
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("cancel a scheduled cron job")
    }

    async fn prompt(&self, _options: &PromptOptions) -> String {
        "Cancel a cron job previously scheduled with CronCreate. Removes it from .coco/scheduled_tasks.json (durable jobs) or the in-memory session store (session-only jobs).".into()
    }

    fn render_for_model(&self, out: &CronDeleteOutput) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: format!("Cancelled job {id}.", id = out.id),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: CronDeleteInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<CronDeleteOutput>, ToolError> {
        if input.schedule_id.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "schedule_id is required".into(),
                error_code: None,
            });
        }
        match ctx.schedules.remove_cron_tasks(&[&input.schedule_id]).await {
            Ok(()) => Ok(ToolResult {
                data: CronDeleteOutput {
                    id: input.schedule_id,
                    status: "deleted".into(),
                },
                new_messages: vec![],
                app_state_patch: None,
                permission_updates: Vec::new(),
                display_data: None,
            }),
            Err(e) => Err(ToolError::ExecutionFailed {
                message: format!(
                    "Failed to delete schedule {id}: {e}",
                    id = input.schedule_id
                ),
                display_data: None,
                source: None,
            }),
        }
    }
}

/// Typed input for [`CronListTool`] — no parameters.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct CronListInput {}

/// One scheduled job entry in [`CronListOutput`].
///
/// Output schema: `{ id, cron, humanSchedule, prompt, recurring?, durable? }`
///
/// All fields marked `#[serde(default)]` so partial test fixtures /
/// transcript replay deserializes successfully into a struct the
/// renderer can iterate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronListJob {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub cron: String,
    /// Wire key is `humanSchedule` for compatibility.
    #[serde(default, rename = "humanSchedule")]
    pub human_schedule: String,
    #[serde(default)]
    pub prompt: String,
    /// Omitted when not set. `Option<bool>` lets us distinguish
    /// "explicitly false" from "absent" on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurring: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable: Option<bool>,
}

/// Typed output for [`CronListTool`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronListOutput {
    pub jobs: Vec<CronListJob>,
}

pub struct CronListTool;

#[async_trait::async_trait]
impl Tool for CronListTool {
    type Input = CronListInput;
    coco_tool_runtime::impl_runtime_schema!(CronListInput);
    type Output = CronListOutput;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::CronList)
    }
    fn name(&self) -> &str {
        ToolName::CronList.as_str()
    }
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::AgentTriggers)
    }
    fn description(&self, _input: &CronListInput, _options: &DescriptionOptions) -> String {
        "List scheduled cron jobs".into()
    }

    async fn prompt(&self, _options: &PromptOptions) -> String {
        "List all cron jobs scheduled via CronCreate, both durable (.coco/scheduled_tasks.json) and session-only.".into()
    }
    fn is_read_only(&self, _input: &CronListInput) -> bool {
        true
    }
    fn is_always_read_only(&self) -> bool {
        true
    }
    /// Listing schedules is a pure read of the schedule store. CronCreate/Delete
    /// stay non-safe because they mutate the store.
    fn is_concurrency_safe(&self, _input: &CronListInput) -> bool {
        true
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn search_hint(&self) -> Option<&str> {
        Some("list active cron jobs")
    }

    /// Render `{jobs: [...]}` as a human-readable summary list.
    /// Empty list → "No scheduled tasks." Otherwise: count + bulleted
    /// `- {id}: {humanSchedule} → {prompt}` per job.
    fn render_for_model(&self, out: &CronListOutput) -> Vec<ToolResultContentPart> {
        let text = if out.jobs.is_empty() {
            "No scheduled tasks.".to_string()
        } else {
            let n = out.jobs.len();
            let plural = if n == 1 { "task" } else { "tasks" };
            let mut buf = format!("{n} scheduled {plural}:");
            for job in &out.jobs {
                let schedule = if job.human_schedule.is_empty() {
                    job.cron.as_str()
                } else {
                    job.human_schedule.as_str()
                };
                buf.push_str(&format!(
                    "\n- {id}: {schedule} → {prompt}",
                    id = job.id,
                    prompt = job.prompt
                ));
            }
            buf
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        _input: CronListInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<CronListOutput>, ToolError> {
        match ctx.schedules.list_all_cron_tasks().await {
            Ok(tasks) => {
                let jobs: Vec<CronListJob> = tasks
                    .iter()
                    .map(|t| CronListJob {
                        id: t.id.clone(),
                        cron: t.cron.clone(),
                        // Human-readable schedule via the shared cron crate;
                        // falls back to the raw cron string.
                        human_schedule: coco_cron::cron_to_human(&t.cron),
                        prompt: t.prompt.clone(),
                        recurring: t.recurring,
                        // durable: None (file-backed) renders as durable; Some(false) = session.
                        durable: t.durable,
                    })
                    .collect();
                Ok(ToolResult {
                    data: CronListOutput { jobs },
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                })
            }
            Err(e) => Err(ToolError::ExecutionFailed {
                message: format!("Failed to list schedules: {e}"),
                display_data: None,
                source: None,
            }),
        }
    }
}

/// Action for [`RemoteTriggerTool`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RemoteTriggerAction {
    /// Default — list all configured triggers.
    #[default]
    List,
    /// Look up a single trigger by id.
    Get,
    /// Create a new trigger from `body`.
    Create,
    /// Update an existing trigger; merges `body` into the stored trigger.
    Update,
    /// Fire the trigger now and return its run result.
    Run,
}

impl RemoteTriggerAction {
    fn requires_trigger_id(self) -> bool {
        matches!(self, Self::Get | Self::Update | Self::Run)
    }
}

/// Typed input for [`RemoteTriggerTool`].
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct RemoteTriggerInput {
    /// The action to perform on triggers.
    ///
    /// REQUIRED — no default emitted.
    pub action: RemoteTriggerAction,
    /// Required for get, update, and run.
    ///
    /// Optional string matching `^[\w-]+$`.
    #[serde(default)]
    #[schemars(regex(pattern = "^[\\w-]+$"))]
    pub trigger_id: Option<String>,
    /// JSON body for create and update. Constrained to an object
    /// (string keys → arbitrary values), not arbitrary JSON.
    #[serde(default)]
    pub body: Option<serde_json::Map<String, Value>>,
}

/// Remote scheduled-agent triggers — **remote execution**, intentionally
/// DEFERRED (sanctioned non-goal).
///
/// Unlike the local `Cron*` tools (which fire a prompt into *this* session),
/// `RemoteTrigger` manages and runs agents on Anthropic's **CCR (Claude Code
/// Remote)** backend via authenticated HTTP (`list`/`get`/`create`/`update`/`run`)
/// against `{BASE_API_URL}/v1/code/triggers` using **claude.ai OAuth tokens**,
/// the org UUID, and the `ccr-triggers-2026-01-30` beta.
///
/// coco-rs does NOT port the transport: it requires claude.ai OAuth and
/// Anthropic-internal endpoints, which are explicit non-goals for the
/// multi-provider port (see root `CLAUDE.md` "Multi-Provider Boundaries" —
/// Anthropic cloud-credential / remote routes are out of scope). The tool
/// stays behind [`Feature::AgentTriggersRemote`] (default OFF), so it is hidden
/// from the model rather than registered-and-failing. If/when a remote-session
/// transport lands, the `ScheduleStore` trigger methods are the integration
/// seam.
pub struct RemoteTriggerTool;

#[async_trait::async_trait]
impl Tool for RemoteTriggerTool {
    type Input = RemoteTriggerInput;
    coco_tool_runtime::impl_runtime_schema!(RemoteTriggerInput);
    /// Output is `Value` — per-action shape (`list` returns array,
    /// `get`/`update` return a trigger object, `create` returns
    /// `{id, name, status}`, `run` returns `{trigger_id, status, result}`).
    /// Tagged-enum modeling deferred until backend types crystallize.
    type Output = Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::RemoteTrigger)
    }
    fn name(&self) -> &str {
        ToolName::RemoteTrigger.as_str()
    }
    /// Gated on [`Feature::AgentTriggersRemote`] (default OFF). The live
    /// transport (claude.ai OAuth + Anthropic-internal endpoints) is an
    /// explicit non-goal, so the feature stays off by default and the tool
    /// is hidden rather than registered-and-failing.
    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::AgentTriggersRemote)
    }
    fn description(&self, _input: &RemoteTriggerInput, _options: &DescriptionOptions) -> String {
        "Manage scheduled remote Claude Code agents (triggers) via the claude.ai CCR API. Auth is handled in-process — the token never reaches the shell.".into()
    }
    fn search_hint(&self) -> Option<&str> {
        Some("manage scheduled remote agent triggers")
    }

    /// Full model-facing description.
    async fn prompt(&self, _options: &PromptOptions) -> String {
        "Call the claude.ai remote-trigger API. Use this instead of curl — the OAuth token is added automatically in-process and never exposed.\n\
\n\
Actions:\n\
- list: GET /v1/code/triggers\n\
- get: GET /v1/code/triggers/{trigger_id}\n\
- create: POST /v1/code/triggers (requires body)\n\
- update: POST /v1/code/triggers/{trigger_id} (requires body, partial update)\n\
- run: POST /v1/code/triggers/{trigger_id}/run\n\
\n\
The response is the raw JSON from the API.".into()
    }
    fn should_defer(&self) -> bool {
        true
    }
    fn is_read_only(&self, input: &RemoteTriggerInput) -> bool {
        matches!(
            input.action,
            RemoteTriggerAction::List | RemoteTriggerAction::Get
        )
    }
    fn is_concurrency_safe(&self, _input: &RemoteTriggerInput) -> bool {
        true
    }

    /// Render trigger envelopes per-action. Most paths return a small
    /// `{id, name, status}` shape; `update` returns the full trigger.
    /// Pick a one-line confirmation for actions; fall back to JSON
    /// for unknown shapes.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let status = data.get("status").and_then(Value::as_str);
        let id = data
            .get("id")
            .or_else(|| data.get("trigger_id"))
            .and_then(Value::as_str);
        let text = match status {
            Some("created") => {
                let name = data
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unnamed");
                format!("Trigger created. id: {}. name: {name}.", id.unwrap_or("?"))
            }
            Some("triggered") => format!("Trigger {} fired.", id.unwrap_or("?")),
            Some("deleted") => format!("Trigger {} deleted.", id.unwrap_or("?")),
            _ => serde_json::to_string(data).unwrap_or_default(),
        };
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    fn validate_input(
        &self,
        input: &RemoteTriggerInput,
        _ctx: &ToolUseContext,
    ) -> coco_tool_runtime::ValidationResult {
        // `action` is now typed; unknown wire values fail at deserialize
        // time (one layer up). Only the "trigger_id required" gate
        // remains as a semantic check.
        if input.action.requires_trigger_id()
            && input.trigger_id.as_deref().unwrap_or("").is_empty()
        {
            return coco_tool_runtime::ValidationResult::invalid(
                "trigger_id is required for get, update, and run actions",
            );
        }
        coco_tool_runtime::ValidationResult::Valid
    }

    async fn execute(
        &self,
        input: RemoteTriggerInput,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let trigger_id = input.trigger_id.as_deref();
        let body = input.body.as_ref();

        match input.action {
            RemoteTriggerAction::List => match ctx.schedules.list_triggers().await {
                Ok(triggers) => Ok(ToolResult {
                    data: serde_json::to_value(&triggers).unwrap_or_default(),
                    new_messages: vec![],
                    app_state_patch: None,
                    permission_updates: Vec::new(),
                    display_data: None,
                }),
                Err(e) => Err(ToolError::ExecutionFailed {
                    message: format!("Failed to list triggers: {e}"),
                    display_data: None,
                    source: None,
                }),
            },
            RemoteTriggerAction::Get => {
                let id = trigger_id.unwrap_or_default();
                match ctx.schedules.get_trigger(id).await {
                    Ok(trigger) => Ok(ToolResult {
                        data: serde_json::to_value(&trigger).unwrap_or_default(),
                        new_messages: vec![],
                        app_state_patch: None,
                        permission_updates: Vec::new(),
                        display_data: None,
                    }),
                    Err(e) => Err(ToolError::ExecutionFailed {
                        message: format!("Failed to get trigger {id}: {e}"),
                        display_data: None,
                        source: None,
                    }),
                }
            }
            RemoteTriggerAction::Create => {
                let name = body
                    .and_then(|b| b.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unnamed");
                let description = body
                    .and_then(|b| b.get("description"))
                    .and_then(|v| v.as_str());
                match ctx.schedules.create_trigger(name, description).await {
                    Ok(trigger) => Ok(ToolResult {
                        data: serde_json::json!({
                            "id": trigger.id,
                            "name": trigger.name,
                            "status": "created",
                        }),
                        new_messages: vec![],
                        app_state_patch: None,
                        permission_updates: Vec::new(),
                        display_data: None,
                    }),
                    Err(e) => Err(ToolError::ExecutionFailed {
                        message: format!("Failed to create trigger: {e}"),
                        display_data: None,
                        source: None,
                    }),
                }
            }
            RemoteTriggerAction::Update => {
                let id = trigger_id.unwrap_or_default();
                match ctx
                    .schedules
                    .update_trigger(
                        id,
                        body.map(|m| Value::Object(m.clone()))
                            .unwrap_or(Value::Null),
                    )
                    .await
                {
                    Ok(trigger) => Ok(ToolResult {
                        data: serde_json::to_value(&trigger).unwrap_or_default(),
                        new_messages: vec![],
                        app_state_patch: None,
                        permission_updates: Vec::new(),
                        display_data: None,
                    }),
                    Err(e) => Err(ToolError::ExecutionFailed {
                        message: format!("Failed to update trigger {id}: {e}"),
                        display_data: None,
                        source: None,
                    }),
                }
            }
            RemoteTriggerAction::Run => {
                let id = trigger_id.unwrap_or_default();
                match ctx.schedules.run_trigger(id).await {
                    Ok(result) => Ok(ToolResult {
                        data: serde_json::json!({
                            "trigger_id": id,
                            "status": "triggered",
                            "result": result,
                        }),
                        new_messages: vec![],
                        app_state_patch: None,
                        permission_updates: Vec::new(),
                        display_data: None,
                    }),
                    Err(e) => Err(ToolError::ExecutionFailed {
                        message: format!("Failed to run trigger {id}: {e}"),
                        display_data: None,
                        source: None,
                    }),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "scheduling.test.rs"]
mod tests;
