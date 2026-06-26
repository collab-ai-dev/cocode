use std::collections::HashSet;

use coco_keybindings::KeybindingAction;
use coco_types::ModelRole;
use coco_types::PermissionMode;

use crate::i18n::t;
use crate::keybinding_bridge::KeybindingContext as TuiContext;
use crate::presentation::context_usage::render_context_usage;
use crate::state::AppState;
use crate::state::FocusTarget;
use crate::state::session::TaskEntryKind;
use crate::status_bar::StatusSpan;
use crate::status_bar::StatusTone;
use crate::transcript::cells::CellKind;
use crate::transcript::cells::RenderedCell;

/// The built-in status bar is a one-to-three-line block:
/// 1. model · effort · tokens · cache · context · transcript counts (always)
/// 2. permission mode (`▸▸ auto mode on`) · background-task pill
/// 3. working-directory basename · `git:(branch)`
/// Lines 2 and 3 are emitted only when they have content, so the bar collapses
/// to a single row in the default state and grows to the full three rows in a
/// real session (a permission mode set + a working dir). [`built_in_line_count`]
/// mirrors the same predicates for the layout pass without building any spans.
pub(crate) fn built_in_status_lines(state: &AppState) -> Vec<Vec<StatusSpan>> {
    let mut lines = vec![model_and_usage_line(state)];
    if show_permission_tasks_line(state) {
        lines.push(permission_and_tasks_line(state));
    }
    if show_directory_line(state) {
        lines.push(directory_line(state));
    }
    lines
}

/// Row count of the built-in bar, cheaply (no span building) — for the layout
/// pass. MUST track the `push` conditions in [`built_in_status_lines`].
pub(crate) fn built_in_line_count(state: &AppState) -> u16 {
    1 + u16::from(show_permission_tasks_line(state)) + u16::from(show_directory_line(state))
}

/// Whether line 2 (permission mode / task pill) has content. Cheap: the task
/// side is an allocation-free `any`, not the formatted pill label.
fn show_permission_tasks_line(state: &AppState) -> bool {
    state.ui.input.vim.enabled
        || permission_mode_status(state.session.permission_mode).is_some()
        || state.session.has_running_background_task()
}

/// Whether line 3 (working dir / git branch) has content.
fn show_directory_line(state: &AppState) -> bool {
    state.session.working_dir.is_some()
}

fn model_and_usage_line(state: &AppState) -> Vec<StatusSpan> {
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
    separator(&mut spans);
    spans.push(StatusSpan::new(
        match usage_costs {
            Some((_, _, true, _)) => format!(
                "↑{}/$? ↓{}/$?",
                format_token_count(tokens.input_tokens),
                format_token_count(tokens.output_tokens)
            ),
            Some((input_cost, output_cost, false, _)) => format!(
                "↑{}/{} ↓{}/{}",
                format_token_count(tokens.input_tokens),
                format_cost(input_cost),
                format_token_count(tokens.output_tokens),
                format_cost(output_cost)
            ),
            None => format!(
                "↑{} ↓{}",
                format_token_count(tokens.input_tokens),
                format_token_count(tokens.output_tokens)
            ),
        },
        StatusTone::Dim,
    ));
    let cache_pct = if tokens.input_tokens > 0 {
        ((tokens.cache_read_tokens.max(0) as f64 * 100.0) / tokens.input_tokens as f64)
            .clamp(0.0, 100.0)
    } else {
        0.0
    };
    spans.push(StatusSpan::new(
        format!(
            " · cache {}/{}",
            format_token_count(tokens.cache_read_tokens),
            format_cache_percent(cache_pct)
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

    separator(&mut spans);
    if let Some(ctx_pct) = render_context_usage(state).map(|u| u.percent) {
        spans.push(StatusSpan {
            text: format!("ctx {ctx_pct}%"),
            tone: if ctx_pct > 90 {
                StatusTone::Error
            } else if ctx_pct > 70 {
                StatusTone::Warning
            } else {
                StatusTone::Dim
            },
            bold: ctx_pct > 90,
        });
    } else {
        spans.push(StatusSpan::new("ctx --", StatusTone::Dim));
    }

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

    separator(&mut spans);
    spans.push(StatusSpan::new(
        transcript_count_status(state.session.transcript.cells()),
        StatusTone::Dim,
    ));
    spans
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TranscriptCounts {
    users: usize,
    assistants: usize,
    tools: usize,
}

fn transcript_count_status(cells: &[RenderedCell]) -> String {
    let counts = transcript_counts(cells);
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

fn transcript_counts(cells: &[RenderedCell]) -> TranscriptCounts {
    let mut seen = HashSet::new();
    let mut counts = TranscriptCounts::default();
    for cell in cells {
        if !seen.insert(cell.message_uuid) {
            continue;
        }
        match &cell.kind {
            CellKind::UserText { .. } => counts.users += 1,
            CellKind::AssistantText { .. }
            | CellKind::AssistantThinking { .. }
            | CellKind::AssistantRedactedThinking
            | CellKind::ToolUse { .. } => counts.assistants += 1,
            CellKind::ToolResult { .. } => counts.tools += 1,
            CellKind::Attachment | CellKind::System(_) => {}
        }
    }
    counts
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

/// Line 3: working-directory basename and `git:(branch)`, zsh-prompt style.
/// Empty when no working directory is known.
fn directory_line(state: &AppState) -> Vec<StatusSpan> {
    let mut spans = Vec::new();
    let Some(dir) = state.session.working_dir.as_deref() else {
        return spans;
    };
    let name = dir
        .rsplit(['/', '\\'])
        .find(|seg| !seg.is_empty())
        .unwrap_or(dir);
    spans.push(StatusSpan::new(format!(" {name}"), StatusTone::Primary));
    if let Some(branch) = state.session.git_branch.as_deref() {
        spans.push(StatusSpan::new(" git:(", StatusTone::Dim));
        spans.push(StatusSpan::new(branch.to_string(), StatusTone::Accent));
        spans.push(StatusSpan::new(")", StatusTone::Dim));
    }
    spans
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

fn format_cost(cost_usd: f64) -> String {
    coco_messages::format_cost(cost_usd)
}
