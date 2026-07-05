//! Dynamic Workflow tool entry point.
//!
//! Resolves + validates the workflow source, registers a background
//! `LocalWorkflow` task, and launches the JS runtime
//! ([`coco_workflow_runtime::WorkflowEngine`]) on a dedicated thread via
//! [`workflow_host`]. Fire-and-forget: returns an `async_launched` result; the
//! engine's `agent()`/progress bridge back to the live subagent system.

use std::path::PathBuf;

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_tool_runtime::WorkflowTaskRequest;
use coco_types::Feature;
use coco_types::PermissionBehavior;
use coco_types::PermissionRule;
use coco_types::PermissionRuleSource;
use coco_types::PermissionRuleValue;
use coco_types::PermissionUpdate;
use coco_types::PermissionUpdateDestination;
use coco_types::TaskType;
use coco_types::ToolCheckResult;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use super::workflow_host;
use super::workflow_journal;

/// Typed validation-failure code threaded into [`ValidationResult::invalid_with_code`].
/// Mirrors CC's numeric `errorCode` taxonomy (source / parse / determinism /
/// resume-still-running) without leaking a magic string at each call site.
/// `.as_str()` returns the stable wire code the UI / classifier consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowValidationCode {
    /// Missing or oversized source: no script/name/scriptPath, or an inline
    /// script that exceeds the byte cap. CC `errorCode 1`.
    SourceError,
    /// `export const meta` / script parse failure. CC `errorCode 2`.
    MetaParse,
    /// Non-deterministic source (`Date.now()` / `Math.random()` / `new Date()`).
    /// CC `errorCode 4`.
    Determinism,
    /// Resume rejected because a workflow with the same run id is still
    /// running and must be stopped first. CC `errorCode 3`. Used by the
    /// resume work; defined now for completeness.
    ResumeRunning,
}

impl WorkflowValidationCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SourceError => "source_error",
            Self::MetaParse => "meta_parse",
            Self::Determinism => "determinism",
            Self::ResumeRunning => "resume_running",
        }
    }
}

/// Model-facing tool prompt: the workflow DSL contract. Lookup directories are
/// interpolated from [`coco_workflow::workflow_dirs_hint`] so the config-dir
/// namespace is never hardcoded.
fn workflow_prompt() -> String {
    format!(
        "Run a local dynamic workflow script that orchestrates multiple subagents.\n\n\
The script is plain JavaScript that MUST begin with `export const meta = {{ name, description, phases }}` (a pure object literal) and MUST be deterministic — no `Date.now()`, `Math.random()`, or `new Date()`.\n\n\
Globals available to the script:\n\
- `agent(prompt, opts?)` — spawn one subagent and `await` its result text. opts: {{ label, phase, agentType, model, effort, isolation, schema }}. isolation may be \"worktree\"; \"remote\" is not available.\n\
- `parallel(thunks)` — run `[() => agent(...), ...]` concurrently (a barrier); a failed call resolves to `null`.\n\
- `pipeline(items, ...stages)` — flow each item independently through all stages; a stage gets `(prev, item, index)`.\n\
- `phase(title)` / `log(message)` — emit progress.\n\
- `args` — the value passed as the `args` parameter; `budget` — the token budget.\n\n\
Provide the source via `scriptPath`, `name` (loaded from {dirs}), or an inline `script`. Pass `resumeFromRunId` to resume a prior (stopped or finished) run in this session: completed `agent()` results replay from the prior run's journal instead of re-spawning. The prior run's on-disk script is authoritative on resume.\n\
The workflow runs in the BACKGROUND: this returns immediately with a taskId, and progress + the final result arrive via task notifications.",
        dirs = coco_workflow::workflow_dirs_hint()
    )
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowInput {
    #[serde(default)]
    pub script: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Accepted for TS compatibility; metadata description comes from
    /// `export const meta`.
    #[serde(default)]
    pub description: Option<String>,
    /// Accepted for TS compatibility; metadata title comes from
    /// `export const meta`.
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub args: Value,
    #[serde(default)]
    pub script_path: Option<String>,
    #[serde(default)]
    pub resume_from_run_id: Option<String>,
}

/// Result of launching a workflow in the background (fire-and-forget). The
/// engine runs on a dedicated thread; progress + the final result arrive via
/// task notifications.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowLaunchResult {
    pub status: String,
    pub task_type: String,
    pub task_id: String,
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workflow_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_file: Option<String>,
}

pub struct WorkflowTool;

#[async_trait::async_trait]
impl Tool for WorkflowTool {
    type Input = WorkflowInput;
    type Output = WorkflowLaunchResult;

    #[allow(clippy::expect_used)]
    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        static SCHEMA: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
            std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            coco_tool_runtime::ToolInputSchema::from_static_value(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "Inline workflow script source."
                    },
                    "name": {
                        "type": "string",
                        "description": format!(
                            "Workflow name to load from {}.",
                            coco_workflow::workflow_dirs_hint()
                        )
                    },
                    "description": {
                        "type": "string",
                        "description": "Ignored compatibility field."
                    },
                    "title": {
                        "type": "string",
                        "description": "Ignored compatibility field."
                    },
                    "args": {
                        "description": "JSON value exposed to the workflow as args."
                    },
                    "scriptPath": {
                        "type": "string",
                        "description": "Path to a local workflow script."
                    },
                    "resumeFromRunId": {
                        "type": "string",
                        "description": "Existing workflow run id to resume."
                    }
                }
            }))
        })
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::Workflow)
    }

    fn name(&self) -> &str {
        ToolName::Workflow.as_str()
    }

    fn aliases(&self) -> &[&str] {
        &["RunWorkflow"]
    }

    fn description(&self, input: &Self::Input, _options: &DescriptionOptions) -> String {
        input
            .name
            .as_deref()
            .or(input.script_path.as_deref())
            .map(|target| format!("Run workflow: {target}"))
            .unwrap_or_else(|| "Run workflow".to_string())
    }

    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        workflow_prompt()
    }

    fn is_enabled(&self, ctx: &ToolUseContext) -> bool {
        ctx.features.enabled(Feature::Workflow)
    }

    fn is_read_only(&self, _input: &Self::Input) -> bool {
        false
    }

    fn validate_input(&self, input: &Self::Input, _ctx: &ToolUseContext) -> ValidationResult {
        let has_script = input.script.as_ref().is_some_and(|s| !s.is_empty());
        let has_name = input.name.as_ref().is_some_and(|s| !s.trim().is_empty());
        let has_script_path = input
            .script_path
            .as_ref()
            .is_some_and(|s| !s.trim().is_empty());
        if !has_script && !has_name && !has_script_path {
            return ValidationResult::invalid_with_code(
                "Workflow requires one of script, name, or scriptPath.",
                WorkflowValidationCode::SourceError.as_str(),
            );
        }
        if let Some(script) = input.script.as_ref()
            && script.len() > coco_workflow::MAX_WORKFLOW_SOURCE_BYTES
        {
            return ValidationResult::invalid_with_code(
                format!(
                    "Workflow inline script exceeds {} bytes.",
                    coco_workflow::MAX_WORKFLOW_SOURCE_BYTES
                ),
                WorkflowValidationCode::SourceError.as_str(),
            );
        }
        if let Some(run_id) = input.resume_from_run_id.as_deref()
            && !run_id.trim().is_empty()
            && !valid_workflow_run_id(run_id)
        {
            return ValidationResult::invalid_with_code(
                "resumeFromRunId must match ^wf_[a-z0-9-]{6,}$.",
                WorkflowValidationCode::SourceError.as_str(),
            );
        }
        ValidationResult::Valid
    }

    async fn check_permissions(
        &self,
        input: &WorkflowInput,
        ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        // Rule key: only a *named* workflow with no `scriptPath` has a stable
        // identity. A `scriptPath` (with or without a name) and a bare inline
        // `script` have none, so they always fall through to Ask.
        // `ruleKey = input.scriptPath ? undefined : input.name`. This closes
        // the bypass where a `{scriptPath, name}` call could be auto-allowed by
        // an unrelated `Workflow(name)` rule while actually running on-disk code.
        let rule_key = workflow_rule_key(input);

        let Some(name) = rule_key else {
            return ToolCheckResult::Ask {
                message: "Allow Workflow to run this local script?".to_string(),
                suggestions: Vec::new(),
                choices: None,
                detail: None,
            };
        };

        if matching_workflow_rule(&ctx.permission_context.deny_rules, name).is_some() {
            return ToolCheckResult::Deny {
                message: format!("Workflow {name} blocked by permission rules."),
            };
        }
        if matching_workflow_rule(&ctx.permission_context.ask_rules, name).is_some() {
            return workflow_ask(name, resolved_script_preview(input, ctx));
        }
        if matching_workflow_rule(&ctx.permission_context.allow_rules, name).is_some() {
            return ToolCheckResult::Allow {
                updated_input: resolve_for_permission(input, ctx),
                feedback: None,
            };
        }
        workflow_ask(name, resolved_script_preview(input, ctx))
    }

    fn prepare_permission_matcher(&self, input: &WorkflowInput) -> String {
        // Mirror the rule key: a `scriptPath` call carries no stable name, so
        // it must not match a `Workflow(name)` rule.
        match workflow_rule_key(input) {
            Some(name) => format!("Workflow({name})"),
            None => self.name().to_string(),
        }
    }

    fn to_auto_classifier_input(&self, input: &WorkflowInput) -> Option<String> {
        Some(
            input
                .script
                .clone()
                .or_else(|| input.name.clone())
                .unwrap_or_default(),
        )
    }

    async fn execute(
        &self,
        input: Self::Input,
        ctx: &ToolUseContext,
    ) -> Result<ToolResult<Self::Output>, ToolError> {
        let resume_run_id = input
            .resume_from_run_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty());

        // Resolve the run to resume (if any): map the prior `wf_…` run id to its
        // `local_workflow` task row. The on-disk script + journal hang off that
        // row's output path. Resume is same-session only, so this only consults
        // the in-session task registry.
        let resume = match resume_run_id {
            Some(run_id) => Some(self.resolve_resume(run_id, ctx).await?),
            None => None,
        };

        // Source resolution differs for resume vs fresh:
        // - Resume: the prior run's ON-DISK script is authoritative; any inbound
        //   `script`/`name`/`scriptPath` is ignored (replay must run identical
        //   source or the cache keys diverge).
        // - Fresh: resolve + parse the inbound source. A parse/source failure is
        //   a real error (TS errorCodes 1/2/4), surfaced before any launch.
        let (script, persisted_source, source_path, run_id, resume_journal_source) = match resume {
            Some(resumed) => {
                let script = coco_workflow::parse_workflow_script(&resumed.source, true).map_err(
                    |error| {
                        ToolError::execution_failed(format!(
                            "Workflow resume was not launched: {error}"
                        ))
                    },
                )?;
                (
                    script,
                    resumed.source,
                    None,
                    // Reuse the resumed run's identity so the new run's journal
                    // continues the prior run's replay lineage.
                    resumed.run_id,
                    Some(resumed.journal_path),
                )
            }
            None => {
                let source_input = coco_workflow::WorkflowSourceInput {
                    script_path: input.script_path.as_ref().map(PathBuf::from),
                    name: input.name.clone(),
                    script: input.script.clone(),
                    cwd: ctx.cwd_override.clone(),
                };
                let (spec, script) = coco_workflow::resolve_workflow_source(source_input)
                    .and_then(|spec| {
                        let check_determinism =
                            matches!(spec.kind, coco_workflow::WorkflowSourceKind::Inline)
                                || (input.script.is_some() && input.script_path.is_some());
                        coco_workflow::parse_workflow_script(&spec.source, check_determinism)
                            .map(|script| (spec, script))
                    })
                    .map_err(|error| {
                        ToolError::execution_failed(format!("Workflow was not launched: {error}"))
                    })?;
                (
                    script,
                    spec.source,
                    spec.source_path,
                    generate_workflow_run_id(),
                    None,
                )
            }
        };

        // Launch requires a background runtime. Checked after source resolution
        // so a source/parse error (TS errorCodes 1/2/4) still surfaces first.
        let Some(task_handle) = ctx.task_handle.clone() else {
            return Err(ToolError::execution_failed(
                "Background tasks are unavailable; cannot launch workflow.",
            ));
        };

        let task_id = coco_types::generate_task_id(TaskType::LocalWorkflow);
        let workflow_name = Some(script.meta.name.clone());
        let cancel = tokio_util::sync::CancellationToken::new();

        // Create the Running row + persist the script (the seam keeps the full
        // source on disk for review/resume).
        let task_id = task_handle
            .register_workflow_task(
                WorkflowTaskRequest {
                    task_id: task_id.clone(),
                    run_id: run_id.clone(),
                    workflow_name: workflow_name.clone(),
                    prompt: None,
                    tool_use_id: ctx.tool_use_id.clone(),
                    script: persisted_source,
                    source_path,
                },
                cancel.clone(),
            )
            .await;

        let output_path = task_handle.output_file_path(&task_id).await;
        let output_file = output_path.as_ref().map(|path| path.display().to_string());

        // Resume journal: results are recorded alongside the run's `.output`
        // file (`<task_id>.journal.jsonl`) so a future `resumeFromRunId` can
        // replay completed `agent()` results. The cache is hydrated up front:
        // - Cross-run resume hydrates from the PRIOR run's journal
        //   (`resume_journal_source`) and appends to this new run's journal.
        // - A fresh run hydrates from its own (not-yet-existing) journal: empty
        //   cache, identical to a fresh journal, with same-task crash recovery
        //   for free.
        let journal_path = output_path
            .as_deref()
            .and_then(workflow_journal::journal_path_for_output);
        let hydrate_source = resume_journal_source.or_else(|| journal_path.clone());
        let journal = std::sync::Arc::new(match hydrate_source {
            Some(source) => workflow_journal::WorkflowJournal::resumed(&source, journal_path),
            None => workflow_journal::WorkflowJournal::new(journal_path),
        });

        // Launch the engine on a dedicated thread (it is `!Send`); run the
        // body (meta stripped), exposing `args` from the tool input.
        workflow_host::spawn_workflow_engine(
            script.script_body,
            input.args.clone(),
            ctx.agent.clone(),
            task_handle,
            task_id.clone(),
            cancel.clone(),
            workflow_host::WorkflowSpawnContext {
                session_id: ctx.session_id_for_history.clone().unwrap_or_default(),
                invoking_agent_id: ctx.agent_id.as_ref().map(|id| id.as_str().to_string()),
                tool_use_id: ctx.tool_use_id.clone(),
                features: ctx.features.clone(),
                skill_overrides: ctx.skill_overrides.clone(),
                tool_overrides: ctx.tool_overrides.clone(),
                parent_tool_filter: ctx.tool_filter.clone(),
                active_shell_tool: ctx.active_shell_tool,
                log_assistant_responses: ctx.log_assistant_responses,
                parent_mode: ctx.permission_context.mode,
                agent_catalog: ctx.agent_catalog.clone(),
                total_token_budget: ctx.total_token_budget,
                workflow_abort: coco_tool_runtime::TurnAbortSignal::from_token(cancel.clone()),
                cwd: ctx.cwd_override.clone(),
            },
            tokio::runtime::Handle::current(),
            journal,
        );

        Ok(ToolResult::data(WorkflowLaunchResult {
            status: "async_launched".to_string(),
            task_type: "local_workflow".to_string(),
            task_id,
            run_id,
            workflow_name,
            output_file,
        }))
    }

    fn render_for_model(&self, output: &Self::Output) -> Vec<ToolResultContentPart> {
        let mut text = format!(
            "Workflow launched in background.\ntaskType: {}\ntaskId: {}\nrunId: {}",
            output.task_type, output.task_id, output.run_id
        );
        if let Some(name) = &output.workflow_name {
            text.push_str(&format!("\nworkflow: {name}"));
        }
        if let Some(output_file) = &output.output_file {
            text.push_str(&format!("\noutputFile: {output_file}"));
        }
        text.push_str(
            "\n\nBriefly tell the user what you launched, then continue with any non-overlapping work if useful. \
Progress and the final result will arrive in a subsequent task notification; \
you can also tail the output file to check progress.",
        );
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }
}

/// A prior workflow run located for `resumeFromRunId`. Carries the AUTHORITATIVE
/// on-disk source and the prior run's journal path so the replay cache hydrates
/// from completed `agent()` results.
struct ResolvedResume {
    /// Reused run id (`wf_…`) so the new run continues the prior replay lineage.
    run_id: String,
    /// The prior run's persisted script source (`<task_id>.workflow.js`).
    source: String,
    /// The prior run's `journal.jsonl`, hydrated into the new run's cache.
    journal_path: PathBuf,
}

impl WorkflowTool {
    /// Resolve a `resumeFromRunId` to its prior run's on-disk script + journal,
    /// enforcing the errorCode-3 precedence: a still-running run must be stopped
    /// first. Same-session only — the lookup consults the in-session registry.
    async fn resolve_resume(
        &self,
        run_id: &str,
        ctx: &ToolUseContext,
    ) -> Result<ResolvedResume, ToolError> {
        let Some(task_handle) = ctx.task_handle.as_ref() else {
            return Err(ToolError::execution_failed(
                "Background tasks are unavailable; cannot resume workflow.",
            ));
        };
        let Some(prior) = task_handle.find_workflow_task_by_run_id(run_id).await else {
            // No matching run in this session: resume is same-session only, so
            // there is nothing to replay. Surface a clear, typed source error.
            return Err(ToolError::InvalidInput {
                message: format!(
                    "Workflow {run_id} was not found in this session; resume is same-session only."
                ),
                error_code: Some(WorkflowValidationCode::SourceError.as_str().to_string()),
            });
        };

        // errorCode-3: a still-running run must be stopped before resuming.
        if !prior.status.is_terminal() {
            return Err(ToolError::InvalidInput {
                message: format!(
                    "Workflow {run_id} is still running (task {tid}). Stop it first with \
                     {stop}({{taskId:\"{tid}\"}}) before resuming.",
                    tid = prior.id,
                    stop = ToolName::TaskStop.as_str(),
                ),
                error_code: Some(WorkflowValidationCode::ResumeRunning.as_str().to_string()),
            });
        }

        // The prior run's script + journal hang off its output path
        // (`<task_id>.output` → `<task_id>.workflow.js` / `.journal.jsonl`).
        let output_path = task_handle.output_file_path(&prior.id).await.ok_or_else(|| {
            ToolError::execution_failed(format!(
                "Workflow {run_id}: prior run output path is unavailable; cannot locate its script."
            ))
        })?;
        let script_path = workflow_journal::script_path_for_output(&output_path);
        let source = tokio::fs::read_to_string(&script_path)
            .await
            .map_err(|error| {
                ToolError::execution_failed(format!(
                    "Workflow {run_id}: failed to read prior run script {path}: {error}",
                    path = script_path.display(),
                ))
            })?;
        let journal_path =
            workflow_journal::journal_path_for_output(&output_path).ok_or_else(|| {
                ToolError::execution_failed(format!(
                    "Workflow {run_id}: prior run journal path could not be derived."
                ))
            })?;

        Ok(ResolvedResume {
            run_id: run_id.to_string(),
            source,
            journal_path,
        })
    }
}

/// The stable rule-matching identity for a workflow invocation: the trimmed
/// `name` only when no `scriptPath` is present, else `None`.
/// `ruleKey = input.scriptPath ? undefined : input.name`.
fn workflow_rule_key(input: &WorkflowInput) -> Option<&str> {
    if input
        .script_path
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        return None;
    }
    input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn resolve_for_permission(input: &WorkflowInput, ctx: &ToolUseContext) -> Option<Value> {
    let source_input = coco_workflow::WorkflowSourceInput {
        script_path: input.script_path.as_ref().map(PathBuf::from),
        name: input.name.clone(),
        script: input.script.clone(),
        cwd: ctx.cwd_override.clone(),
    };
    let spec = coco_workflow::resolve_workflow_source(source_input).ok()?;
    let mut updated = serde_json::to_value(input).ok()?;
    if let Value::Object(object) = &mut updated {
        object.insert("script".to_string(), Value::String(spec.source));
    }
    Some(updated)
}

/// Max chars of resolved-script preview shown in the approval prompt (TS uses a
/// 400-char preview cap, `AWa`).
const WORKFLOW_PREVIEW_CHARS: usize = 400;

fn workflow_ask(name: &str, script_preview: Option<String>) -> ToolCheckResult {
    // Surface the resolved script in the approval prompt so a human approving a
    // named/scriptPath workflow reviews the actual code, not just `{name}` (TS
    // shows `updatedInput ?? input` in the dialog).
    let message = match script_preview {
        Some(preview) => {
            format!("Allow Workflow to run {name}?\n\nResolved script (preview):\n{preview}")
        }
        None => format!("Allow Workflow to run {name}?"),
    };
    ToolCheckResult::Ask {
        message,
        suggestions: workflow_name_suggestions(name),
        choices: None,
        detail: None,
    }
}

/// Resolve the workflow source and return a bounded preview for the approval
/// prompt. `None` if the source can't be resolved (the prompt then omits it).
fn resolved_script_preview(input: &WorkflowInput, ctx: &ToolUseContext) -> Option<String> {
    let source_input = coco_workflow::WorkflowSourceInput {
        script_path: input.script_path.as_ref().map(PathBuf::from),
        name: input.name.clone(),
        script: input.script.clone(),
        cwd: ctx.cwd_override.clone(),
    };
    let spec = coco_workflow::resolve_workflow_source(source_input).ok()?;
    let preview: String = spec.source.chars().take(WORKFLOW_PREVIEW_CHARS).collect();
    if spec.source.chars().count() > WORKFLOW_PREVIEW_CHARS {
        Some(format!("{preview}…"))
    } else {
        Some(preview)
    }
}

fn workflow_name_suggestions(name: &str) -> Vec<PermissionUpdate> {
    vec![PermissionUpdate::AddRules {
        rules: vec![PermissionRule {
            source: PermissionRuleSource::Session,
            behavior: PermissionBehavior::Allow,
            value: PermissionRuleValue {
                tool_pattern: ToolName::Workflow.as_str().to_string(),
                rule_content: Some(name.to_string()),
            },
        }],
        destination: PermissionUpdateDestination::Session,
    }]
}

/// A `Workflow` (or `*`) permission rule whose `rule_content` matches `name`.
/// Content-less rules deliberately do NOT match: because a workflow runs
/// arbitrary code, only an explicit `Workflow(name)` rule auto-allows, matching
/// The rule lookup keys solely on
/// `ruleContent`).
fn matching_workflow_rule<'a>(
    rules: &'a coco_types::PermissionRulesBySource,
    name: &str,
) -> Option<&'a PermissionRule> {
    rules.values().flatten().find(|rule| {
        (coco_types::tool_matches_pattern(&rule.value.tool_pattern, ToolName::Workflow.as_str())
            || rule.value.tool_pattern == "*")
            && rule
                .value
                .rule_content
                .as_deref()
                .is_some_and(|content| coco_types::content_matches(content, name))
    })
}

fn valid_workflow_run_id(run_id: &str) -> bool {
    let Some(rest) = run_id.strip_prefix("wf_") else {
        return false;
    };
    rest.len() >= 6
        && rest
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Generate a `wf_<lowercase-alnum>` run id satisfying [`valid_workflow_run_id`]
/// (`random_alphanumeric` is lowercase-only).
fn generate_workflow_run_id() -> String {
    let task_id = coco_types::generate_task_id(TaskType::LocalWorkflow);
    let suffix = task_id.strip_prefix('w').unwrap_or(task_id.as_str());
    format!("wf_{suffix}")
}

#[cfg(test)]
#[path = "workflow.test.rs"]
mod tests;
