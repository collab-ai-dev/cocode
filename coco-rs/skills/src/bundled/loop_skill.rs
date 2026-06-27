//! `/loop` — schedule a recurring prompt by registering a cron job.
//!
//! Mirrors Claude Code 2.1.183 `loop.ts` for the local surfaces coco-rs has:
//! fixed-interval cron, loop.md/autonomous defaults, dynamic self-pacing via
//! `ScheduleWakeup`, and event monitors via `Monitor`.

use coco_types::ToolName;
use std::path::Path;

const DEFAULT_INTERVAL: &str = "10m";
const LOOP_FILE_MAX_BYTES: usize = 25_000;

pub const LOOP_FILE_SENTINEL: &str = "<<loop.md>>";
pub const LOOP_FILE_DYNAMIC_SENTINEL: &str = "<<loop.md-dynamic>>";
pub const AUTONOMOUS_LOOP_SENTINEL: &str = "<<autonomous-loop>>";
pub const AUTONOMOUS_LOOP_DYNAMIC_SENTINEL: &str = "<<autonomous-loop-dynamic>>";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopFile {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LoopSentinelState {
    last_loop_file_content: Option<String>,
    last_loop_file_dynamic_content: Option<String>,
    autonomous_delivered: bool,
    autonomous_dynamic_delivered: bool,
}

impl LoopSentinelState {
    pub fn reset(&mut self) {
        self.last_loop_file_content = None;
        self.last_loop_file_dynamic_content = None;
        self.autonomous_delivered = false;
        self.autonomous_dynamic_delivered = false;
    }
}

/// Render the `/loop` skill prompt with coco built-in tool names substituted.
pub fn prompt() -> String {
    render_fixed_prompt("$ARGUMENTS", false)
}

/// Render the prompt for a concrete `/loop` invocation.
pub fn prompt_for_command(
    args: &str,
    project_root: &Path,
    cwd: &Path,
    default_prompt_enabled: bool,
    dynamic_loop_enabled: bool,
    persistent_preamble_enabled: bool,
    remote_schedule_enabled: bool,
) -> String {
    let trimmed = args.trim();
    let every_interval = parse_bare_every_interval(trimmed);
    let interval_or_every = is_interval_token(trimmed) || every_interval.is_some();

    if default_prompt_enabled && (trimmed.is_empty() || interval_or_every) {
        let interval = every_interval.as_deref().unwrap_or(if trimmed.is_empty() {
            DEFAULT_INTERVAL
        } else {
            trimmed
        });
        let loop_file = read_loop_file(project_root, cwd);
        if trimmed.is_empty() && dynamic_loop_enabled {
            return match loop_file {
                Some(loop_file) => render_loop_file_dynamic_default_prompt(&loop_file),
                None => render_autonomous_dynamic_default_prompt(persistent_preamble_enabled),
            };
        }
        return match loop_file {
            Some(loop_file) => render_loop_file_default_prompt(&loop_file, interval),
            None => render_autonomous_default_prompt(interval, persistent_preamble_enabled),
        };
    }

    if trimmed.is_empty() {
        return if dynamic_loop_enabled {
            render_dynamic_usage()
        } else {
            render_legacy_usage()
        };
    }

    if dynamic_loop_enabled {
        return render_dynamic_prompt(trimmed, remote_schedule_enabled);
    }

    render_fixed_prompt(trimmed, remote_schedule_enabled)
}

/// Expand a scheduled `/loop` sentinel into the prompt that should be enqueued.
pub fn expand_sentinel_prompt(
    prompt: &str,
    project_root: &Path,
    cwd: &Path,
    persistent_preamble_enabled: bool,
) -> Option<String> {
    let mut state = LoopSentinelState::default();
    expand_sentinel_prompt_with_state(
        prompt,
        project_root,
        cwd,
        &mut state,
        persistent_preamble_enabled,
    )
}

/// Expand a scheduled `/loop` sentinel while preserving Claude Code's compact
/// reminder behavior for repeated unchanged ticks.
pub fn expand_sentinel_prompt_with_state(
    prompt: &str,
    project_root: &Path,
    cwd: &Path,
    state: &mut LoopSentinelState,
    persistent_preamble_enabled: bool,
) -> Option<String> {
    match prompt.trim() {
        LOOP_FILE_SENTINEL => Some(render_loop_file_tick_prompt(
            project_root,
            cwd,
            state,
            false,
            persistent_preamble_enabled,
        )),
        LOOP_FILE_DYNAMIC_SENTINEL => Some(render_loop_file_tick_prompt(
            project_root,
            cwd,
            state,
            true,
            persistent_preamble_enabled,
        )),
        AUTONOMOUS_LOOP_SENTINEL => Some(render_autonomous_tick_prompt(
            state,
            false,
            persistent_preamble_enabled,
        )),
        AUTONOMOUS_LOOP_DYNAMIC_SENTINEL => Some(render_autonomous_tick_prompt(
            state,
            true,
            persistent_preamble_enabled,
        )),
        _ => None,
    }
}

pub fn read_loop_file(project_root: &Path, cwd: &Path) -> Option<LoopFile> {
    let mut candidates = vec![
        project_root.join(".claude").join("loop.md"),
        cwd.join("loop.md"),
    ];
    candidates.dedup();

    candidates.into_iter().find_map(|path| {
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let content = std::fs::read_to_string(&path).ok()?;
        let content = content.trim();
        if content.is_empty() {
            return None;
        }
        Some(LoopFile {
            path: path.display().to_string(),
            content: truncate_loop_file(content),
        })
    })
}

fn render_fixed_prompt(args: &str, remote_schedule_enabled: bool) -> String {
    FIXED_TEMPLATE
        .replace("__CRON_CREATE__", ToolName::CronCreate.as_str())
        .replace("__CRON_DELETE__", ToolName::CronDelete.as_str())
        .replace(
            "__CLOUD_OFFER__",
            &cloud_offer_section(remote_schedule_enabled),
        )
        .replace(
            "__SESSION_ONLY_FOOTER__",
            &session_only_footer_line(remote_schedule_enabled),
        )
        .replace("$ARGUMENTS", args)
}

fn render_legacy_usage() -> String {
    format!(
        r#"Usage: /loop [interval] <prompt>

Run a prompt or slash command on a recurring interval.

Intervals: Ns, Nm, Nh, Nd (e.g. 5m, 30m, 2h, 1d). Minimum granularity is 1 minute.
If no interval is specified, defaults to {DEFAULT_INTERVAL}.

Examples:
  /loop 5m /babysit-prs
  /loop 30m check the deploy
  /loop 1h /standup 1
  /loop check the deploy          (defaults to {DEFAULT_INTERVAL})
  /loop check the deploy every 20m"#
    )
}

fn render_dynamic_usage() -> String {
    r#"Usage: /loop [interval] <prompt>

Run a prompt or slash command on a recurring interval — or with no interval, let the model self-pace based on the task.

Intervals: Ns, Nm, Nh, Nd (e.g. 5m, 30m, 2h, 1d). Minimum granularity is 1 minute.
If no interval is specified, the model picks a delay between iterations based on what it's doing.

Examples:
  /loop 5m /babysit-prs
  /loop 30m check the deploy
  /loop 1h /standup 1
  /loop check the deploy          (dynamic — model picks delays)
  /loop check the deploy every 20m"#
        .to_string()
}

fn render_dynamic_prompt(args: &str, remote_schedule_enabled: bool) -> String {
    DYNAMIC_TEMPLATE
        .replace("__CRON_CREATE__", ToolName::CronCreate.as_str())
        .replace("__CRON_DELETE__", ToolName::CronDelete.as_str())
        .replace("__SCHEDULE_WAKEUP__", ToolName::ScheduleWakeup.as_str())
        .replace("__MONITOR__", ToolName::Monitor.as_str())
        .replace("__TASK_LIST__", ToolName::TaskList.as_str())
        .replace("__TASK_STOP__", ToolName::TaskStop.as_str())
        .replace(
            "__CLOUD_OFFER__",
            &cloud_offer_section(remote_schedule_enabled),
        )
        .replace(
            "__SESSION_ONLY_FOOTER__",
            &session_only_footer_line(remote_schedule_enabled),
        )
        .replace("$ARGUMENTS", args)
}

fn cloud_offer_section(remote_schedule_enabled: bool) -> String {
    if !remote_schedule_enabled {
        return String::new();
    }

    format!(
        r#"
## Offer cloud first

Before any scheduling step, check whether EITHER is true:
- the parsed interval (rule 1 or 2) is **>=60 minutes**, or
- regardless of which rule matched, the original input uses daily phrasing ("every morning", "daily", "every day", "each night", "every weekday")

If either is true, call {ask_user_question} first:
- `question`: "This loop stops when you close this session. Set it up as a cloud schedule instead so it keeps running?"
- `header`: "Schedule"
- `options`: `[{{label: "Cloud schedule (recommended)", description: "Runs in the cloud even after you close this session"}}, {{label: "This session only", description: "Runs in this terminal until you exit"}}]`

If they pick **Cloud schedule**: do NOT call {cron_create}. Invoke the `schedule` skill directly via the {skill} tool with `args` set to their original input verbatim (e.g. `{skill}({{skill: "schedule", args: "every morning tell me a joke"}})`), then follow that skill's instructions to completion. Do NOT tell the user to run /schedule themselves. **Then stop - do not continue to any section below** (no {cron_create}, no {schedule_wakeup}, no "execute the prompt now").
If they pick **This session only**:
- If the trigger was a parsed >=60-minute interval (rule 1 or 2): continue below with that interval.
- If the trigger was daily phrasing only (rule 3, no parsed interval): do NOT call {cron_create}. Explain that a daily-cadence loop won't fire before this session closes, so there's nothing useful to schedule locally - suggest they either pick Cloud schedule, or re-run `/loop` with an explicit shorter interval (e.g. `/loop 1h <prompt>`) if they want a session loop. Then stop.
If neither trigger condition was met: continue below.
"#,
        ask_user_question = ToolName::AskUserQuestion.as_str(),
        cron_create = ToolName::CronCreate.as_str(),
        schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
        skill = ToolName::Skill.as_str(),
    )
}

fn session_only_footer_line(remote_schedule_enabled: bool) -> String {
    if !remote_schedule_enabled {
        return String::new();
    }

    format!(
        " Only if you did NOT show the cloud-offer {ask_user_question} above (i.e., neither trigger condition applied), end the confirmation with this exact line on its own, italicized: `_Runs until you close this session · For durable cloud-based loops, use /schedule_`. If the user already answered that question, omit this line.",
        ask_user_question = ToolName::AskUserQuestion.as_str(),
    )
}

fn render_loop_file_default_prompt(loop_file: &LoopFile, interval: &str) -> String {
    format!(
        r#"# /loop — schedule loop.md tasks

The user invoked `/loop` with no prompt (input was empty or just the interval `{interval}`) and has a loop-tasks file at `{path}`. Schedule a recurring cron that runs those tasks each tick, then run the first tick immediately.

## Action

1. Convert `{interval}` to a 5-field cron expression. Supported suffixes: `s` -> ceil to nearest minute, `m` (minutes), `h` (hours), `d` (days). Examples: `5m` -> `*/5 * * * *`, `1h` -> `0 * * * *`, `1d` -> `0 0 * * *`. If the interval doesn't cleanly divide its unit, round to the nearest clean interval and tell the user what you rounded to.
2. Call {cron_create} with:
   - `cron`: the expression from step 1
   - `prompt`: the literal string `{sentinel}` — the scheduler expands it at fire time to the current loop.md contents.
   - `recurring`: `true`
3. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that it's running tasks from `{path}`, that recurring tasks auto-expire after 7 days, and that the user can cancel sooner with {cron_delete} (include the job ID).
4. **Then immediately run the loop.md tasks now**, following the instructions inlined below. Don't wait for the first cron fire.

## Loop tasks (from {path})

{content}"#,
        cron_create = ToolName::CronCreate.as_str(),
        cron_delete = ToolName::CronDelete.as_str(),
        sentinel = LOOP_FILE_SENTINEL,
        path = loop_file.path,
        content = loop_file.content,
    )
}

fn render_autonomous_default_prompt(interval: &str, persistent_preamble_enabled: bool) -> String {
    let preamble = autonomous_preamble(persistent_preamble_enabled);
    format!(
        r#"# /loop — schedule the autonomous default

The user invoked `/loop` with no prompt (input was empty or just the interval `{interval}`). Schedule the autonomous-loop default and then run the first autonomous check immediately.

## Action

1. Convert `{interval}` to a 5-field cron expression. Supported suffixes: `s` -> ceil to nearest minute, `m` (minutes), `h` (hours), `d` (days). Examples: `5m` -> `*/5 * * * *`, `1h` -> `0 * * * *`, `1d` -> `0 0 * * *`. If the interval doesn't cleanly divide its unit, round to the nearest clean interval and tell the user what you rounded to.
2. Call {cron_create} with:
   - `cron`: the expression from step 1
   - `prompt`: the literal string `{sentinel}` — it expands at fire time to the autonomous-loop instructions.
   - `recurring`: `true`
3. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after 7 days, and that they can cancel sooner with {cron_delete} (include the job ID). Mention this is the autonomous default and that the autonomous-loop instructions are baked in.
4. **Then immediately run the autonomous check now**, following the instructions inlined below. Don't wait for the first cron fire.

## Autonomous-loop instructions (for the immediate execution and every fire)

{preamble}"#,
        cron_create = ToolName::CronCreate.as_str(),
        cron_delete = ToolName::CronDelete.as_str(),
        sentinel = AUTONOMOUS_LOOP_SENTINEL,
        preamble = preamble,
    )
}

fn render_loop_file_dynamic_default_prompt(loop_file: &LoopFile) -> String {
    format!(
        r#"# /loop — loop.md tasks with dynamic pacing

The user invoked `/loop` with no prompt and no interval and has a loop-tasks file at `{path}`. Run those tasks now, then self-pace the next iteration via {schedule_wakeup} — no cron.

## Action

1. **Run the loop.md tasks now**, following the instructions inlined below.
2. **If the next tick is gated on an event** (CI finishing, a PR comment, a log line) and no {monitor} is already running for it: arm one now with `persistent: true`. Its events wake this loop immediately — you do not wait for the {schedule_wakeup} deadline. Arm once; on later ticks call {task_list} first and skip if a monitor is already running.
3. **Briefly confirm**: that you're running tasks from `{path}` in dynamic-pacing mode, that you ran the first tick now, whether a {monitor} is the primary wake signal, and what fallback delay you're about to pick. Write this as text *before* calling {schedule_wakeup} — the turn ends as soon as that tool returns.
4. **Then, as the last action of this turn, call {schedule_wakeup}** with:
   - `delaySeconds`: with a {monitor} armed this is the fallback heartbeat (lean 1200–1800s). Without one, pick based on what you observed this turn — quiet branch? wait longer. Lots in flight? wait shorter. Read the tool's own description for cache-aware delay guidance.
   - `reason`: one short sentence on why you picked that delay.
   - `prompt`: the literal string `{sentinel}` — the dynamic-mode sentinel expands at fire time to the full instructions (first fire / first fire post-compact / loop.md edited) or a dynamic-pacing-specific short reminder (subsequent fires). Do not pass the full instructions; that is handled automatically.
5. **If woken by a `<task-notification>`** rather than this prompt: handle the event, then call {schedule_wakeup} again with `{sentinel}` and the same 1200–1800s `delaySeconds` from step 4 — the {monitor} remains the wake signal; this only resets the safety net.
6. **To stop the loop**, omit the {schedule_wakeup} call and {task_stop} any {monitor} you armed (use {task_list} to find the task ID if it is no longer in context).

## Loop tasks (from {path})

{content}"#,
        path = loop_file.path,
        schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
        monitor = ToolName::Monitor.as_str(),
        task_list = ToolName::TaskList.as_str(),
        task_stop = ToolName::TaskStop.as_str(),
        sentinel = LOOP_FILE_DYNAMIC_SENTINEL,
        content = loop_file.content,
    )
}

fn render_autonomous_dynamic_default_prompt(persistent_preamble_enabled: bool) -> String {
    let preamble = autonomous_preamble(persistent_preamble_enabled);
    format!(
        r#"# /loop — autonomous default with dynamic pacing

The user invoked `/loop` with no prompt and no interval. Run the autonomous check now, then self-pace the next iteration via {schedule_wakeup} — no cron.

## Action

1. **Run the autonomous check now**, following the instructions inlined below.
2. **If the next tick is gated on an event** (CI finishing, a PR comment, a log line) and no {monitor} is already running for it: arm one now with `persistent: true`. Its events wake this loop immediately — you do not wait for the {schedule_wakeup} deadline. Arm once; on later ticks call {task_list} first and skip if a monitor is already running.
3. **Briefly confirm**: that this is the autonomous default in dynamic-pacing mode, that you ran the check now, whether a {monitor} is the primary wake signal, and what fallback delay you're about to pick. Write this as text *before* calling {schedule_wakeup} — the turn ends as soon as that tool returns.
4. **Then, as the last action of this turn, call {schedule_wakeup}** with:
   - `delaySeconds`: with a {monitor} armed this is the fallback heartbeat (lean 1200–1800s). Without one, pick based on what you observed this turn — quiet branch? wait longer. Lots in flight? wait shorter. Read the tool's own description for cache-aware delay guidance.
   - `reason`: one short sentence on why you picked that delay.
   - `prompt`: the literal string `{sentinel}` — the dynamic-mode sentinel expands at fire time to the full instructions (first fire / first fire post-compact) or a dynamic-pacing-specific short reminder (subsequent fires). Do not pass the full instructions; that is handled automatically.
5. **If woken by a `<task-notification>`** rather than this prompt: handle the event, then call {schedule_wakeup} again with `{sentinel}` and the same 1200–1800s `delaySeconds` from step 4 — the {monitor} remains the wake signal; this only resets the safety net.
6. **To stop the loop**, omit the {schedule_wakeup} call and {task_stop} any {monitor} you armed (use {task_list} to find the task ID if it is no longer in context).

## Autonomous-loop instructions (for the immediate execution and every fire)

{preamble}"#,
        schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
        monitor = ToolName::Monitor.as_str(),
        task_list = ToolName::TaskList.as_str(),
        task_stop = ToolName::TaskStop.as_str(),
        sentinel = AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
        preamble = preamble,
    )
}

fn render_loop_file_tick_prompt(
    project_root: &Path,
    cwd: &Path,
    state: &mut LoopSentinelState,
    dynamic: bool,
    persistent_preamble_enabled: bool,
) -> String {
    if let Some(loop_file) = read_loop_file(project_root, cwd) {
        let last_content = if dynamic {
            &mut state.last_loop_file_dynamic_content
        } else {
            &mut state.last_loop_file_content
        };
        if last_content.as_deref() == Some(loop_file.content.as_str()) {
            return render_loop_file_tick_reminder(dynamic);
        }
        *last_content = Some(loop_file.content.clone());
        return format!(
            r#"# /loop tick — tasks from {path}

The user configured a loop-tasks file. Work through the tasks defined below; these are the instructions for this tick and every subsequent tick (the reminder on later fires refers back to this message).

---

{content}

---

{reminder}"#,
            path = loop_file.path,
            content = loop_file.content,
            reminder = render_loop_file_tick_reminder(dynamic),
        );
    }

    let delivered = if dynamic {
        &mut state.autonomous_dynamic_delivered
    } else {
        &mut state.autonomous_delivered
    };
    if *delivered {
        return render_loop_file_absent_reminder(dynamic);
    }
    *delivered = true;
    format!(
        "{preamble}\n\n---\n\n{reminder}",
        preamble = autonomous_preamble(persistent_preamble_enabled),
        reminder = render_loop_file_absent_reminder(dynamic),
    )
}

fn render_autonomous_tick_prompt(
    state: &mut LoopSentinelState,
    dynamic: bool,
    persistent_preamble_enabled: bool,
) -> String {
    let reminder = render_autonomous_tick_reminder(dynamic);
    let delivered = if dynamic {
        &mut state.autonomous_dynamic_delivered
    } else {
        &mut state.autonomous_delivered
    };
    if *delivered {
        return reminder;
    }
    *delivered = true;
    format!(
        "{}\n\n---\n\n{reminder}",
        autonomous_preamble(persistent_preamble_enabled)
    )
}

fn render_autonomous_tick_reminder(dynamic: bool) -> String {
    if dynamic {
        return format!(
            r#"# Autonomous loop tick (dynamic pacing)

Run the autonomous check using the loop instructions established earlier in this conversation. If you cannot find them, treat this as a no-op tick.

If this tick was triggered by a `<task-notification>`, handle that event in the context of the autonomous loop task. If a {monitor} is still the primary wake signal, re-arm {schedule_wakeup} with the literal sentinel `{sentinel}` and a 1200–1800s fallback heartbeat; this only resets the safety net. Otherwise pick a delay based on what you observed. To stop the loop, omit {schedule_wakeup} and {task_stop} any monitor you armed."#,
            schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
            monitor = ToolName::Monitor.as_str(),
            task_stop = ToolName::TaskStop.as_str(),
            sentinel = AUTONOMOUS_LOOP_DYNAMIC_SENTINEL,
        );
    }

    format!(
        r#"# Autonomous loop tick

Run the autonomous check using the loop instructions established earlier in this conversation. If you cannot find them, treat this as a no-op tick. The recurring cron will fire the next tick automatically — do not call {schedule_wakeup} from this tick."#,
        schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
    )
}

fn render_loop_file_tick_reminder(dynamic: bool) -> String {
    if dynamic {
        return format!(
            r#"# /loop tick — loop.md tasks (dynamic pacing)

Work the tasks from the loop.md contents established earlier in this conversation. If you cannot find them, treat this as a no-op tick.

If this tick was triggered by a `<task-notification>`, handle that event in the context of the loop.md task. If a {monitor} is still the primary wake signal, re-arm {schedule_wakeup} with the literal sentinel `{sentinel}` and a 1200–1800s fallback heartbeat; this only resets the safety net. Otherwise pick a delay based on what you observed. To stop the loop, omit {schedule_wakeup} and {task_stop} any monitor you armed."#,
            schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
            monitor = ToolName::Monitor.as_str(),
            task_stop = ToolName::TaskStop.as_str(),
            sentinel = LOOP_FILE_DYNAMIC_SENTINEL,
        );
    }

    format!(
        r#"# /loop tick — loop.md tasks

Work the tasks from the loop.md contents established earlier in this conversation. If you cannot find them, treat this as a no-op tick. The recurring cron will fire the next tick automatically — do not call {schedule_wakeup} from this tick."#,
        schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
    )
}

fn render_loop_file_absent_reminder(dynamic: bool) -> String {
    if dynamic {
        return format!(
            r#"# /loop tick — loop.md absent (dynamic pacing)

loop.md is not currently present. Run the autonomous check using the loop instructions established earlier in this conversation.

If this tick was triggered by a `<task-notification>`, handle that event in the context of the loop task. If a {monitor} is still the primary wake signal, re-arm {schedule_wakeup} with the literal sentinel `{sentinel}` and a 1200–1800s fallback heartbeat; this only resets the safety net. Otherwise pick a delay based on what you observed. To stop the loop, omit {schedule_wakeup} and {task_stop} any monitor you armed."#,
            schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
            monitor = ToolName::Monitor.as_str(),
            task_stop = ToolName::TaskStop.as_str(),
            sentinel = LOOP_FILE_DYNAMIC_SENTINEL,
        );
    }

    format!(
        r#"# Autonomous loop tick

Run the autonomous check using the loop instructions established earlier in this conversation. If you cannot find them, treat this as a no-op tick. The recurring cron will fire the next tick automatically — do not call {schedule_wakeup} from this tick."#,
        schedule_wakeup = ToolName::ScheduleWakeup.as_str(),
    )
}

fn truncate_loop_file(content: &str) -> String {
    if content.len() <= LOOP_FILE_MAX_BYTES {
        return content.to_string();
    }

    let mut max = LOOP_FILE_MAX_BYTES;
    while !content.is_char_boundary(max) {
        max -= 1;
    }

    let cutoff = content[..max]
        .rfind('\n')
        .filter(|idx| *idx > 0)
        .unwrap_or(max);

    format!(
        "{}\n\n> WARNING: loop.md was truncated to {LOOP_FILE_MAX_BYTES} bytes. Keep the task list concise.",
        &content[..cutoff]
    )
}

fn is_interval_token(value: &str) -> bool {
    let Some(unit) = value.as_bytes().last().copied() else {
        return false;
    };
    matches!(unit, b's' | b'm' | b'h' | b'd')
        && value.len() > 1
        && value[..value.len() - 1].bytes().all(|b| b.is_ascii_digit())
}

fn parse_bare_every_interval(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let (keyword, rest) = trimmed.split_once(char::is_whitespace)?;
    if !keyword.eq_ignore_ascii_case("every") {
        return None;
    }
    let rest = rest.trim_start();
    let amount_end = rest
        .char_indices()
        .find_map(|(idx, ch)| (!ch.is_ascii_digit()).then_some(idx))
        .unwrap_or(rest.len());
    if amount_end == 0 {
        return None;
    }
    let amount = &rest[..amount_end];
    let unit_text = rest[amount_end..].trim();
    let mut unit_parts = unit_text.split_whitespace();
    let unit = unit_parts.next()?;
    if unit_parts.next().is_some() {
        return None;
    }
    let suffix = match unit.to_ascii_lowercase().as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => "s",
        "m" | "min" | "mins" | "minute" | "minutes" => "m",
        "h" | "hr" | "hrs" | "hour" | "hours" => "h",
        "d" | "day" | "days" => "d",
        _ => return None,
    };
    Some(format!("{amount}{suffix}"))
}

fn autonomous_preamble(persistent_preamble_enabled: bool) -> &'static str {
    if persistent_preamble_enabled {
        PERSISTENT_AUTONOMOUS_LOOP_PREAMBLE
    } else {
        AUTONOMOUS_LOOP_PREAMBLE
    }
}

const FIXED_TEMPLATE: &str = r#"# /loop — schedule a recurring prompt

Parse the input below into `[interval] <prompt…>` and schedule it with __CRON_CREATE__.

## Parsing (in priority order)

1. **Leading token**: if the first whitespace-delimited token matches `^\d+[smhd]$` (e.g. `5m`, `2h`), that's the interval; the rest is the prompt.
2. **Trailing "every" clause**: otherwise, if the input ends with `every <N><unit>` or `every <N> <unit-word>` (e.g. `every 20m`, `every 5 minutes`, `every 2 hours`), extract that as the interval and strip it from the prompt. Only match when what follows "every" is a time expression — `check every PR` has no interval.
3. **Default**: otherwise, interval is `10m` and the entire input is the prompt.

If the resulting prompt is empty, show usage `/loop [interval] <prompt>` and stop — do not call __CRON_CREATE__.

Examples:
- `5m /babysit-prs` → interval `5m`, prompt `/babysit-prs` (rule 1)
- `check the deploy every 20m` → interval `20m`, prompt `check the deploy` (rule 2)
- `run tests every 5 minutes` → interval `5m`, prompt `run tests` (rule 2)
- `check the deploy` → interval `10m`, prompt `check the deploy` (rule 3)
- `check every PR` → interval `10m`, prompt `check every PR` (rule 3 — "every" not followed by time)
- `5m` → empty prompt → show usage
__CLOUD_OFFER__

## Interval → cron

Supported suffixes: `s` (seconds, rounded up to nearest minute, min 1), `m` (minutes), `h` (hours), `d` (days). Convert:

| Interval pattern      | Cron expression     | Notes                                    |
|-----------------------|---------------------|------------------------------------------|
| `Nm` where N ≤ 59     | `*/N * * * *`       | every N minutes                          |
| `Nm` where N ≥ 60     | `0 */H * * *`       | round to hours (H = N/60, must divide 24)|
| `Nh` where N ≤ 23     | `0 */N * * *`       | every N hours                            |
| `Nd`                  | `0 0 */N * *`       | every N days at midnight local           |
| `Ns`                  | treat as `ceil(N/60)m` | cron minimum granularity is 1 minute  |

**If the interval doesn't cleanly divide its unit** (e.g. `7m` → `*/7 * * * *` gives uneven gaps at :56→:00; `90m` → 1.5h which cron can't express), pick the nearest clean interval and tell the user what you rounded to before scheduling.

## Action

`/loop` REGISTERS A RECURRING CRON JOB and returns — it does NOT loop in-process or busy-wait.

1. Call __CRON_CREATE__ with:
   - `cron`: the expression from the table above
   - `prompt`: the parsed prompt from above, verbatim (slash commands are passed through unchanged)
   - `recurring`: `true`
2. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after 7 days, and that they can cancel sooner with __CRON_DELETE__ (include the job ID).__SESSION_ONLY_FOOTER__
3. **Then immediately execute the parsed prompt now** — don't wait for the first cron fire. If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.

## Input

$ARGUMENTS
"#;

const DYNAMIC_TEMPLATE: &str = r#"# /loop — schedule a recurring or self-paced prompt

Parse the input below into `[interval] <prompt…>` and schedule it.

## Parsing (in priority order)

1. **Leading token**: if the first whitespace-delimited token matches `^\d+[smhd]$` (e.g. `5m`, `2h`), that's the interval; the rest is the prompt.
2. **Trailing "every" clause**: otherwise, if the input ends with `every <N><unit>` or `every <N> <unit-word>` (e.g. `every 20m`, `every 5 minutes`, `every 2 hours`), extract that as the interval and strip it from the prompt. Only match when what follows "every" is a time expression — `check every PR` has no interval.
3. **No interval**: otherwise, the entire input is the prompt and you'll self-pace dynamically (see "Dynamic mode" below).

If the resulting prompt is empty, show usage `/loop [interval] <prompt>` and stop.
__CLOUD_OFFER__

## Fixed-interval mode (rules 1 and 2)

Convert the interval to a cron expression:

Supported suffixes: `s` (seconds, rounded up to nearest minute, min 1), `m` (minutes), `h` (hours), `d` (days). Convert:

| Interval pattern      | Cron expression     | Notes                                    |
|-----------------------|---------------------|------------------------------------------|
| `Nm` where N <= 59    | `*/N * * * *`       | every N minutes                          |
| `Nm` where N >= 60    | `0 */H * * *`       | round to hours (H = N/60, must divide 24)|
| `Nh` where N <= 23    | `0 */N * * *`       | every N hours                            |
| `Nd`                  | `0 0 */N * *`       | every N days at midnight local           |
| `Ns`                  | treat as `ceil(N/60)m` | cron minimum granularity is 1 minute  |

**If the interval doesn't cleanly divide its unit** (e.g. `7m` -> `*/7 * * * *` gives uneven gaps at :56->:00; `90m` -> 1.5h which cron can't express), pick the nearest clean interval and tell the user what you rounded to before scheduling.

Then:

1. Call __CRON_CREATE__ with: `cron` (the expression), `prompt` (the parsed prompt verbatim), `recurring: true`.
2. Briefly confirm: what's scheduled, the cron expression, the human-readable cadence, that recurring tasks auto-expire after 7 days, and that the user can cancel sooner with __CRON_DELETE__ (include the job ID).__SESSION_ONLY_FOOTER__
3. **Then immediately execute the parsed prompt now** — don't wait for the first cron fire. If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.

## Dynamic mode (rule 3 — no interval)

The user wants you to self-pace. Decide what makes the next iteration worth running — a passage of time, or an observable event.

1. **Run the parsed prompt now.** If it's a slash command, invoke it via the Skill tool; otherwise act on it directly.
2. **If the next run is gated on an event** (CI finishing, a log line matching, a file changing, a PR comment) and no __MONITOR__ is already running for it: arm one now with `persistent: true`. Its events arrive as `<task-notification>` messages and wake this loop immediately — you do not wait for the __SCHEDULE_WAKEUP__ deadline. Arm once; on later iterations call __TASK_LIST__ first and skip this step if a monitor is already running.
3. **Briefly confirm**: that you're self-pacing, whether a __MONITOR__ is the primary wake signal, that you ran the task now, and what fallback delay you're about to pick. Write this as text *before* calling __SCHEDULE_WAKEUP__ — the turn ends as soon as that tool returns.
4. **Then, as the last action of this turn, call __SCHEDULE_WAKEUP__** with:
   - `delaySeconds`: with a __MONITOR__ armed this is the **fallback heartbeat** — how long to wait if no event fires (lean 1200–1800s; idle ticks past the 5-minute cache window are pure overhead). Without a __MONITOR__ this is the cadence — pick based on what you observed. Read the tool's own description for cache-aware delay guidance.
   - `reason`: one short sentence on why you picked that delay.
   - `prompt`: the full original `/loop` input verbatim, prefixed with `/loop ` so the next firing re-enters this skill and continues the loop.
5. **If you were woken by a `<task-notification>`** rather than this prompt: handle the event in the context of the loop task, then call __SCHEDULE_WAKEUP__ again with the same `prompt` and the same 1200–1800s `delaySeconds` from step 4 — the __MONITOR__ remains the wake signal; this only resets the safety net.
6. **To stop the loop**, omit the __SCHEDULE_WAKEUP__ call and __TASK_STOP__ any __MONITOR__ you armed (use __TASK_LIST__ to find the task ID if it is no longer in context).

## Input

$ARGUMENTS
"#;

const AUTONOMOUS_LOOP_PREAMBLE: &str = r#"# Autonomous loop check

You're being invoked on a timer while the user is away or occupied. Continue work that is already established in the conversation: finish local implementation, run skipped verification, address failing checks, or maintain the current branch/PR when that clearly follows from the transcript.

Act when the next step is reversible and grounded in prior context. For irreversible actions such as pushing, deleting, or sending messages, require clear authorization in the transcript. If there is nothing actionable, say so briefly and keep the loop alive unless the original task is complete or the user asked you to stop.

Read and analyze freely. Make edits and run tests when they continue established work. Do not invent unrelated new projects or broaden scope without a transcript-backed reason.
"#;

const PERSISTENT_AUTONOMOUS_LOOP_PREAMBLE: &str = r#"# Autonomous loop check

You're being invoked on a timer while the user is away or occupied. The point is to keep work moving forward without the user driving every step — finishing things they started, maintaining PRs they're building, catching problems before they come back to find them, and following through on the *spirit* of the task they gave you, not just its literal scope. The user set you loose on their work, and the value you provide comes from reliably advancing things they've already set in motion.

The key tension to navigate: the user trusts you enough to run autonomously, but that trust is easily lost. Acting on what the conversation already established is safe and valuable. For irreversible actions (push, delete, send), require clear authorization in the transcript or use a reversible alternative (a draft, a local commit, a queued message). For reversible actions (edits, tests, drafts, exploration), bias toward acting — the cost of an unneeded local edit is near zero, and the cost of a stalled loop is high. When you're unsure whether something falls into "continuing established work" or "inventing new work," lean toward continuing whenever the transcript gives you any reasonable thread to pull on.

## What to act on

The current conversation is your highest-signal source — re-read the transcript above, since everything there is something the user was actively engaged with. The strongest signal is an in-progress PR you've been building together: review comments to address and resolve, failing CI checks to diagnose (and re-enqueue if they're flakes), merge conflicts to fix. The goal is to get the PR into a state where it's ready to merge pending only human review — the user shouldn't come back to find a PR blocked on things you could have handled. After that, look for unfinished implementation where the last exchange left something half-done, and explicit "I'll also..." or "next I'll..." commitments the conversation made and didn't honor. Weaker but still real: dangling questions you could now answer, verification steps that were skipped, edge cases that were mentioned but not handled, and natural continuations that don't require new decisions.

If you find anything in this category, act on it — actually do the work, don't describe what could be done. Run the tests, don't say "you could run the tests." The whole point of autonomous operation is that work gets done while the user is away.

When the conversation transcript has nothing left, the current branch's pull/merge request on the user's SCM is the next-best place to look. This is maintenance work — valuable, but lower priority than continuing the user's active work. Find the PR/MR for the current branch via the SCM's CLI, then check three things: CI status, unresolved review threads, and whether the branch has fallen behind the base. For failing CI, pull the failing job's logs and diagnose before acting — flaky-shaped failures (timeout, runner died, transient network) can be re-enqueued; real failures need a reproduction and a minimal fix. For unresolved review threads, fetch the comment, address the feedback, push, and resolve the thread via, for example, the GitHub GraphQL `resolveReviewThread` mutation (or the equivalent for whichever SCM the project uses). Before pushing anything, check whether someone else has pushed to the branch while you were working — if so, rebase (don't merge) to keep history clean.

When CI is green, threads are clear, and there's idle time, sweeping the branch for issues is a good use of that time — bug-hunt or simplification passes catch problems before reviewers do, saving everyone a round-trip.

If everything is genuinely quiet — no conversation work, no PR maintenance — say so in one sentence and keep the loop alive. Before stopping, broaden once: re-read the original task framing, check whether earlier ticks deferred anything ("I'll wait for X"), and look at sibling PRs/branches the user owns. Persistence is the point of autonomous mode. Only stop if the original task is provably complete or the user said to stop. (Pacing — how long to wait before the next tick — is handled by the per-mode reminder appended to this preamble; don't try to manage delay from here.)

## Repeated invocations

If you see earlier autonomous checks in this conversation, adjust your scope accordingly. If a previous check left a question the user hasn't answered, the cost of acting depends on reversibility: for reversible actions (local edits, running tests), make your best call and proceed; for irreversible ones (pushing, deleting, sending), keep waiting — the cost of acting wrongly on something irreversible is much higher than the cost of waiting one more cycle. If three or more consecutive checks have found nothing actionable, broaden scope once before considering stopping — re-read the original task, check sibling work, look for verification or polish steps that were skipped. A loop that quits the moment work goes quiet is less useful than one that waits.

Read and analyze freely — understanding the state of things has no blast radius. Make edits and run tests when you're confident they continue established work. Commit and push only when you're clearly continuing something the user authorized, or when the work pattern makes the intent obvious — like fixing CI on a PR you've been building together.
"#;
