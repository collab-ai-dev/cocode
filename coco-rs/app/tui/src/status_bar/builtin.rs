use coco_keybindings::KeybindingAction;
use coco_types::ModelRole;
use coco_types::PermissionMode;

use crate::i18n::t;
use crate::keybinding_bridge::KeybindingContext as TuiContext;
use crate::presentation::context_usage::render_context_usage;
use crate::state::AppState;
use crate::state::FocusTarget;
use crate::state::session::TaskEntryKind;
use crate::state::transcript_view::TranscriptCounts;
use crate::status_bar::StatusSpan;
use crate::status_bar::StatusTone;

/// The built-in status bar is a one-to-three-line block:
/// 1. model · effort · cycle-hint · ctx · total spend · transcript counts · MCP · LSP (always)
/// 2. main-thread spend (`↑in/$ ↓out/$ · cache`) + subagent aggregate
/// 3. permission mode (`▸▸ auto mode on`) · task pill · working dir `git:(branch)`
/// Lines 2 and 3 are emitted only when they have content, so the bar collapses
/// to a single row in the default state and grows to the full three rows in a
/// real session. [`built_in_line_count`] mirrors the same predicates for the
/// layout pass without building any spans.
pub(crate) fn built_in_status_lines(state: &AppState) -> Vec<Vec<StatusSpan>> {
    let mut lines = vec![identity_line(state)];
    if show_usage_line(state) {
        lines.push(usage_line(state));
    }
    if show_environment_line(state) {
        lines.push(environment_line(state));
    }
    lines
}

/// Row count of the built-in bar, cheaply (no span building) — for the layout
/// pass. MUST track the `push` conditions in [`built_in_status_lines`].
pub(crate) fn built_in_line_count(state: &AppState) -> u16 {
    1 + u16::from(show_usage_line(state)) + u16::from(show_environment_line(state))
}

/// Whether line 2 (permission mode / task pill) has content. Cheap: the task
/// side is an allocation-free `any`, not the formatted pill label.
fn show_permission_tasks_line(state: &AppState) -> bool {
    state.ui.input.vim.enabled
        || permission_mode_status(state.session.permission_mode).is_some()
        || state.session.active_goal.is_some()
        || state.session.has_running_background_task()
}

/// Whether line 2 (spend) has content: any main-thread token activity, or a
/// subagent has reported.
fn show_usage_line(state: &AppState) -> bool {
    let tokens = &state.session.token_usage;
    tokens.input_tokens > 0
        || tokens.output_tokens > 0
        || state.session.subagent_usage.has_activity()
}

/// Whether line 3 (permission mode / task pill / working dir) has content.
fn show_environment_line(state: &AppState) -> bool {
    show_permission_tasks_line(state) || state.session.working_dir.is_some()
}

/// Line 1 (identity + vitals): model · effort · cycle-hint | ctx | turn
/// total spend | counts | MCP | LSP. Always rendered.
fn identity_line(state: &AppState) -> Vec<StatusSpan> {
    let mut spans = Vec::new();
    let (provider, model_id) = state
        .session
        .model_by_role
        .get(&ModelRole::Main)
        .map(|b| (b.provider.clone(), b.model_id.clone()))
        .unwrap_or_else(|| (state.session.provider.clone(), state.session.model.clone()));
    let model_display = if !provider.is_empty() && !model_id.is_empty() {
        format!("{provider}/{model_id}")
    } else if !model_id.is_empty() {
        model_id
    } else {
        provider
    };
    let has_model = !model_display.is_empty();
    if has_model {
        spans.push(StatusSpan::bold(
            format!(" {model_display}"),
            StatusTone::Primary,
        ));
        if state.session.fast_mode {
            spans.push(StatusSpan::new(" ⚡", StatusTone::Warning));
        }
    }

    let join = if has_model { " * " } else { " " };
    spans.push(StatusSpan::new(join, StatusTone::Dim));
    spans.push(StatusSpan::new(
        state.session.thinking_effort.to_string(),
        StatusTone::Dim,
    ));
    if let Some(hint) = state
        .ui
        .kb_handle
        .display_for(&KeybindingAction::ChatCycleThinking, TuiContext::Chat)
    {
        spans.push(StatusSpan::new(" * ", StatusTone::Dim));
        spans.push(StatusSpan::new(format!("{hint} to cycle"), StatusTone::Dim));
    }

    if let Some(hint) = state.ui.kb_handle.pending_display() {
        separator(&mut spans);
        spans.push(StatusSpan::bold(hint, StatusTone::Warning));
    }

    if let Some(warning) = state.ui.terminal_compatibility_warning.as_ref() {
        separator(&mut spans);
        spans.push(StatusSpan::bold(warning.clone(), StatusTone::Warning));
    }

    separator(&mut spans);
    if let Some(usage) = render_context_usage(state) {
        let trigger = ctx_trigger_percent(state, usage.total);
        let (tone, bold) = ctx_tone(usage.percent, trigger);
        spans.push(StatusSpan {
            text: format!(
                "ctx {}/{}",
                format_ctx_percent(usage.percent_tenths),
                format_token_count(usage.total)
            ),
            tone,
            bold,
        });
    } else {
        spans.push(StatusSpan::new("ctx --", StatusTone::Dim));
    }

    if let Some(usage) = total_usage_summary(state) {
        separator(&mut spans);
        let mut text = t!(
            "status.total_usage",
            input = format_token_count(usage.input_tokens),
            output = format_token_count(usage.output_tokens)
        )
        .to_string();
        if let Some(cost) = usage.cost {
            text.push(' ');
            text.push_str(&cost);
        }
        spans.push(StatusSpan::new(text, StatusTone::Dim));
    }

    separator(&mut spans);
    spans.push(StatusSpan::new(
        transcript_count_status(state.session.transcript.cumulative_counts()),
        StatusTone::Dim,
    ));

    let mcp_count = state.session.connected_mcp_count();
    if mcp_count > 0 {
        separator(&mut spans);
        spans.push(StatusSpan::new(
            t!("status.mcp", count = mcp_count).to_string(),
            StatusTone::Dim,
        ));
    }

    if state.session.lsp_active {
        separator(&mut spans);
        spans.push(StatusSpan::new("LSP", StatusTone::Dim));
    }

    spans
}

/// Line 2 (spend): session `↑in/$ ↓out/$ · cache` and, once any subagent
/// reports, the aggregate `↳ subagents …` group. Both use 2-decimal costs for
/// a compact, scannable width. Rendered only when there is token activity
/// ([`show_usage_line`]).
fn usage_line(state: &AppState) -> Vec<StatusSpan> {
    let mut spans = Vec::new();
    let tokens = &state.session.token_usage;
    let usage_costs = state.session.session_usage.as_ref().map(|snapshot| {
        let input_cost = snapshot.totals.input_cost_usd
            + snapshot.totals.cache_read_cost_usd
            + snapshot.totals.cache_creation_cost_usd;
        let output_cost = snapshot.totals.output_cost_usd;
        let all_unpriced = snapshot.totals.request_count > 0
            && snapshot.totals.unpriced_request_count == snapshot.totals.request_count;
        (
            input_cost,
            output_cost,
            all_unpriced,
            snapshot.unpriced_models.len(),
        )
    });
    spans.push(StatusSpan::new(
        match usage_costs {
            Some((_, _, true, _)) => t!(
                "status.session_usage_unpriced",
                input = format_token_count(tokens.input_tokens),
                output = format_token_count(tokens.output_tokens)
            )
            .to_string(),
            Some((input_cost, output_cost, false, _)) => t!(
                "status.session_usage",
                input = format_token_count(tokens.input_tokens),
                input_cost = format_cost_2dp(input_cost),
                output = format_token_count(tokens.output_tokens),
                output_cost = format_cost_2dp(output_cost)
            )
            .to_string(),
            None => t!(
                "status.session_usage_tokens",
                input = format_token_count(tokens.input_tokens),
                output = format_token_count(tokens.output_tokens)
            )
            .to_string(),
        },
        StatusTone::Dim,
    ));
    spans.push(StatusSpan::new(
        format!(
            " · cache {}/{}",
            format_token_count(tokens.cache_read_tokens),
            format_cache_percent(cache_percent(tokens.cache_read_tokens, tokens.input_tokens))
        ),
        StatusTone::Dim,
    ));
    if let Some((_, _, false, unpriced_count)) = usage_costs
        && unpriced_count > 0
    {
        spans.push(StatusSpan::new(
            format!(" · unpriced {unpriced_count}"),
            StatusTone::Warning,
        ));
    }

    // Aggregate subagent spend — a separate session-cumulative bucket, never
    // mixed into the main-thread usage earlier on this line. Split per
    // direction (`↑in/$ ↓out/$`) to mirror the main thread. Agent-tool
    // subagents only; teammates run their own sessions and account their own
    // spend. Hidden until the first subagent reports.
    let sub = &state.session.subagent_usage;
    if sub.has_activity() {
        separator(&mut spans);
        spans.push(StatusSpan::new(
            t!(
                "status.subagent_usage",
                input = format_token_count(sub.input_tokens),
                input_cost = format_cost_2dp(sub.input_cost_usd),
                output = format_token_count(sub.output_tokens),
                output_cost = format_cost_2dp(sub.output_cost_usd),
                cache = format!(
                    "{}/{}",
                    format_token_count(sub.cache_read_tokens),
                    format_cache_percent(cache_percent(sub.cache_read_tokens, sub.input_tokens))
                )
            )
            .to_string(),
            StatusTone::Dim,
        ));
    }
    spans
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TotalUsageSummary {
    input_tokens: i64,
    output_tokens: i64,
    cost: Option<String>,
}

fn total_usage_summary(state: &AppState) -> Option<TotalUsageSummary> {
    let tokens = &state.session.token_usage;
    let sub = &state.session.subagent_usage;
    let input_tokens = tokens.input_tokens.saturating_add(sub.input_tokens);
    let output_tokens = tokens.output_tokens.saturating_add(sub.output_tokens);
    let has_activity = input_tokens > 0 || output_tokens > 0 || sub.cost_usd > 0.0;
    if !has_activity {
        return None;
    }

    let cost = match state.session.session_usage.as_ref() {
        Some(snapshot) => {
            let total_cost = snapshot.totals.total_cost_usd + sub.cost_usd;
            let all_main_unpriced = snapshot.totals.request_count > 0
                && snapshot.totals.unpriced_request_count == snapshot.totals.request_count;
            let has_partial_unpriced = !all_main_unpriced && !snapshot.unpriced_models.is_empty();
            Some(if all_main_unpriced && sub.cost_usd <= 0.0 {
                "$?".to_string()
            } else if has_partial_unpriced {
                format!("{}+?", format_cost_2dp(total_cost))
            } else {
                format_cost_2dp(total_cost)
            })
        }
        None if sub.cost_usd > 0.0 => Some(format_cost_2dp(sub.cost_usd)),
        None => None,
    };

    Some(TotalUsageSummary {
        input_tokens,
        output_tokens,
        cost,
    })
}

/// Fallback reserve (`min(max_output, 20K) = 20K` + `13K` buffer) used to
/// anchor the `ctx` color bands before the engine reports the exact
/// `auto_compact_threshold`. Mirrors `coco_compact`'s
/// `MAX_OUTPUT_TOKENS_FOR_SUMMARY + AUTOCOMPACT_BUFFER_TOKENS`.
const CTX_TRIGGER_FALLBACK_RESERVED: i64 = 33_000;

/// Auto-compact trigger as a percent of the full window — the anchor for the
/// `ctx` color bands. Prefers the engine-reported exact threshold; falls back
/// to the default-formula reserve when it is not yet known.
fn ctx_trigger_percent(state: &AppState, total_window: i64) -> i64 {
    let threshold = state
        .session
        .session_usage
        .as_ref()
        .and_then(|s| s.auto_compact_threshold)
        .unwrap_or((total_window - CTX_TRIGGER_FALLBACK_RESERVED).max(0));
    (threshold * 100 / total_window.max(1)).clamp(0, 100)
}

/// Reference trigger for the historical color ramp. The default compact
/// threshold on large windows lands near 90%, with tone boundaries at
/// 28/48/78/90. Lower trigger points scale those boundaries proportionally
/// instead of subtracting fixed percentage points that can go negative.
const CTX_REFERENCE_TRIGGER_PERCENT: i64 = 90;
const CTX_REFERENCE_ACCENT_START: i64 = 28;
const CTX_REFERENCE_WARNING_START: i64 = 48;
const CTX_REFERENCE_ERROR_START: i64 = 78;

/// `ctx` tone + bold on a 4-band ramp anchored to the auto-compact trigger
/// `T`: green · blue · yellow · red, with bold red at/over `T`. Higher ctx% =
/// fuller context = more urgent.
fn ctx_tone(pct: i64, trigger: i64) -> (StatusTone, bool) {
    let accent_start = ctx_scaled_boundary(trigger, CTX_REFERENCE_ACCENT_START);
    let warning_start = ctx_scaled_boundary(trigger, CTX_REFERENCE_WARNING_START);
    let error_start = ctx_scaled_boundary(trigger, CTX_REFERENCE_ERROR_START);

    if pct >= trigger {
        (StatusTone::Error, true)
    } else if pct >= error_start {
        (StatusTone::Error, false)
    } else if pct >= warning_start {
        (StatusTone::Warning, false)
    } else if pct >= accent_start {
        (StatusTone::Accent, false)
    } else {
        (StatusTone::Success, false)
    }
}

fn ctx_scaled_boundary(trigger: i64, reference_boundary: i64) -> i64 {
    if trigger <= 0 {
        return 0;
    }
    (trigger * reference_boundary / CTX_REFERENCE_TRIGGER_PERCENT).clamp(0, trigger)
}

/// Status-bar cost: always 2 decimals (`$0.26`) for compact width, unlike the
/// shared `coco_messages::format_cost` which widens to 4 decimals under $0.50.
fn format_cost_2dp(cost_usd: f64) -> String {
    format!("${cost_usd:.2}")
}

/// Cache-hit share of total input tokens, clamped to `[0, 100]`.
/// `input_tokens` is the cache-inclusive total, so the ratio reads as
/// "how much of the input was served from the prompt cache".
fn cache_percent(cache_read_tokens: i64, input_tokens: i64) -> f64 {
    if input_tokens > 0 {
        ((cache_read_tokens.max(0) as f64 * 100.0) / input_tokens as f64).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Session-cumulative user / assistant / tool counts. The fold lives in
/// [`crate::state::transcript_view::TranscriptView`] — deduped by
/// message uuid, surviving compaction, reset only at session boundaries
/// — so this is O(1) per frame.
fn transcript_count_status(counts: TranscriptCounts) -> String {
    if counts.tools > 0 {
        t!(
            "status.turn_counts_with_tools",
            users = counts.users,
            assistants = counts.assistants,
            tools = counts.tools
        )
        .to_string()
    } else {
        t!(
            "status.turn_counts",
            users = counts.users,
            assistants = counts.assistants
        )
        .to_string()
    }
}

fn separator(spans: &mut Vec<StatusSpan>) {
    spans.push(StatusSpan::new(" | ", StatusTone::Border));
}

/// Line 2: permission mode + cycle hint (`⏯ ask mode on · shift+tab to cycle`,
/// `▸▸ auto mode on · shift+tab to cycle`) followed by the background-task pill
/// (`· 1 agent · 2 shells`). Always rendered — every mode (incl. the baseline)
/// shows its glyph, label, and the shift+tab affordance uniformly.
fn permission_and_tasks_line(state: &AppState) -> Vec<StatusSpan> {
    let mut spans = Vec::new();
    // Vim mode badge — the in-band tell of NORMAL vs INSERT (the cursor shape
    // is the other half, see `cursor::vim_cursor_style`). Only shown when vim
    // editing is enabled.
    if state.ui.input.vim.enabled {
        let (label, tone) = if state.ui.input.vim.is_normal() {
            ("NORMAL", StatusTone::Accent)
        } else {
            ("INSERT", StatusTone::Primary)
        };
        spans.push(StatusSpan::bold(format!(" {label} "), tone));
    }
    if let Some((symbol, label, tone)) = permission_mode_status(state.session.permission_mode) {
        let lead = if spans.is_empty() { " " } else { " · " };
        spans.push(StatusSpan::new(format!("{lead}{symbol} {label}"), tone));
        // Every mode shows the cycle gesture, `·`-separated and dimmed, so the
        // shift+tab affordance is uniform across modes.
        spans.push(StatusSpan::new(" · ", StatusTone::Dim));
        spans.push(StatusSpan::new(
            t!("permission_mode.status.cycle_hint").to_string(),
            StatusTone::Dim,
        ));
    }
    if let Some(goal) = state.session.active_goal.as_ref() {
        let lead = if spans.is_empty() { " " } else { " · " };
        spans.push(StatusSpan::new(lead, StatusTone::Dim));
        spans.push(StatusSpan::bold(
            goal_status_label(goal),
            StatusTone::Accent,
        ));
    }
    if let Some(pill) = background_pill_label(state) {
        let lead = if spans.is_empty() { " " } else { " · " };
        spans.push(StatusSpan::new(lead, StatusTone::Dim));
        // Reverse-highlight when the footer pill holds focus (down-arrow from
        // the composer parks here; Enter opens the background-tasks dialog).
        let tone = if state.ui.focus == FocusTarget::FooterShells {
            StatusTone::Accent
        } else {
            StatusTone::Dim
        };
        spans.push(StatusSpan {
            text: pill,
            tone,
            bold: state.ui.focus == FocusTarget::FooterShells,
        });
    }
    spans
}

fn goal_status_label(goal: &coco_types::ActiveGoal) -> String {
    let elapsed_ms = unix_time_ms().saturating_sub(goal.set_at_ms);
    if elapsed_ms <= 0 {
        " /goal active".to_string()
    } else {
        format!(" /goal active ({})", format_goal_duration(elapsed_ms))
    }
}

fn format_goal_duration(ms: i64) -> String {
    let seconds = (ms / 1000).max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        if minutes == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{minutes}m")
        }
    }
}

fn unix_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

/// Symbol + localized label + tone for the current permission mode.
/// `⏯` (play/pause) for the baseline `ask` mode, `⏸` for plan, `▸▸` (fast-forward)
/// for the auto-proceed modes. The cycle hint is appended uniformly in
/// [`permission_and_tasks_line`].
/// Glyphs are chosen for cross-platform coverage: `⏵`/`⏵⏵` (U+23F5) lacks a
/// glyph in most Linux monospace fonts and renders as tofu boxes, so the
/// fast-forward look uses `▸▸` (U+25B8, Geometric Shapes — universally covered)
/// and the play glyph uses `⏯` (U+23EF, same media-control family as `⏸`).
/// Override-mode tones match TS: auto → warning (yellow), bypass/dont-ask →
/// error (red); the baseline stays dim.
fn permission_mode_status(mode: PermissionMode) -> Option<(&'static str, String, StatusTone)> {
    let (symbol, key, tone) = match mode {
        PermissionMode::Default => ("⏯", "permission_mode.status.default", StatusTone::Accent),
        PermissionMode::AcceptEdits => (
            "▸▸",
            "permission_mode.status.accept_edits",
            StatusTone::Accent,
        ),
        PermissionMode::Plan => ("⏸", "permission_mode.status.plan", StatusTone::Plan),
        PermissionMode::BypassPermissions => {
            ("▸▸", "permission_mode.status.bypass", StatusTone::Error)
        }
        PermissionMode::DontAsk => ("▸▸", "permission_mode.status.dont_ask", StatusTone::Error),
        PermissionMode::Auto => ("▸▸", "permission_mode.status.auto", StatusTone::Warning),
        PermissionMode::Bubble => ("▸▸", "permission_mode.status.bubble", StatusTone::Dim),
    };
    Some((symbol, t!(key).to_string(), tone))
}

/// `getPillLabel` port, extended for local workflows: "1 agent",
/// "2 shells", "1 workflow", or "1 agent · 2 shells · 1 workflow".
/// Counts only running tasks; `None` when nothing is running.
pub(crate) fn background_pill_label(state: &AppState) -> Option<String> {
    let mut shells = 0i64;
    let mut agents = 0i64;
    let mut workflows = 0i64;
    for task in state
        .session
        .active_tasks
        .iter()
        .filter(|t| t.is_running_background())
    {
        match task.kind {
            TaskEntryKind::Shell => shells += 1,
            TaskEntryKind::Agent => agents += 1,
            TaskEntryKind::Workflow => workflows += 1,
            TaskEntryKind::Other => {}
        }
    }
    let mut parts = Vec::new();
    if agents > 0 {
        parts.push(
            if agents == 1 {
                t!("status.background.agent_one", count = agents)
            } else {
                t!("status.background.agent_other", count = agents)
            }
            .to_string(),
        );
    }
    if shells > 0 {
        parts.push(
            if shells == 1 {
                t!("status.background.shell_one", count = shells)
            } else {
                t!("status.background.shell_other", count = shells)
            }
            .to_string(),
        );
    }
    if workflows > 0 {
        parts.push(
            if workflows == 1 {
                t!("status.background.workflow_one", count = workflows)
            } else {
                t!("status.background.workflow_other", count = workflows)
            }
            .to_string(),
        );
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Line 3 (environment): permission mode / task pill, then the working
/// directory + `git:(branch)`. Merges the mode and directory groups so the
/// row reads "in this dir, in this mode".
fn environment_line(state: &AppState) -> Vec<StatusSpan> {
    let mut spans = permission_and_tasks_line(state);
    if let Some(dir_spans) = directory_spans(state) {
        if spans.is_empty() {
            spans.push(StatusSpan::new(" ", StatusTone::Dim));
        } else {
            separator(&mut spans);
        }
        spans.extend(dir_spans);
    }
    spans
}

/// Working-directory basename + `git:(branch)`, zsh-prompt style (dim parens,
/// accent branch). `None` when no working directory is known. No leading
/// space — [`environment_line`] adds the pad/separator.
fn directory_spans(state: &AppState) -> Option<Vec<StatusSpan>> {
    let dir = state.session.working_dir.as_deref()?;
    let name = dir
        .rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty())
        .unwrap_or(dir);
    let mut spans = vec![StatusSpan::new(name.to_string(), StatusTone::Primary)];
    if let Some(branch) = state.session.git_branch.as_deref() {
        spans.push(StatusSpan::new(" git:(", StatusTone::Dim));
        spans.push(StatusSpan::new(branch.to_string(), StatusTone::Accent));
        spans.push(StatusSpan::new(")", StatusTone::Dim));
    }
    Some(spans)
}

pub(crate) fn format_token_count(count: i64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        format!("{count}")
    }
}

fn format_cache_percent(pct: f64) -> String {
    if pct == 0.0 {
        "0%".to_string()
    } else {
        format!("{pct:.1}%")
    }
}

fn format_ctx_percent(percent_tenths: i64) -> String {
    format!("{}.{:01}%", percent_tenths / 10, percent_tenths % 10)
}
