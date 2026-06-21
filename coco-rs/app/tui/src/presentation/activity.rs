//! View model for the live activity surface in the main console workspace.

use std::borrow::Cow;

use coco_types::ExpandedView;
use unicode_width::UnicodeWidthStr;

use crate::i18n::t;
use crate::state::AppState;
use crate::state::SubagentStatus;
use crate::state::session::TaskEntryStatus;
use crate::state::session::ToolStatus;
use coco_tui_ui::constants;

const MAX_TOOL_ACTIVITY_DISPLAY: usize = 5;
// Per-subagent row caps so a long task description or live-action tail can't
// run to the screen edge (the Agents panel paints without `Wrap`, so overflow
// is hard-clipped mid-word). Mirrors the Tools section's `truncate_chars`.
const SUBAGENT_DESC_MAX_CHARS: usize = 48;
const SUBAGENT_TAIL_MAX_CHARS: usize = 40;
// Row budgets for the inline activity panel. Raised from 3/5/7 so the
// subagent panel renders every agent (one line each now) instead of
// folding the tail behind `…more in activity`. `inline_activity_height`
// still clamps to the actual available screen height, so a large agent
// count can't overrun the viewport.
const INLINE_ACTIVITY_ROWS_NARROW: u16 = 6;
const INLINE_ACTIVITY_ROWS_NORMAL: u16 = 10;
const INLINE_ACTIVITY_ROWS_WIDE: u16 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityTitle {
    Activity,
    Agents,
    Coordinator,
    Tasks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityBorder {
    Plan,
    Agents,
    Coordinator,
    Activity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityTone {
    Text,
    Dim,
    Accent,
    Running,
    Completed,
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivitySpan {
    /// `Cow<'static, str>` so `&'static str` literals stay borrowed
    /// (zero alloc) while `String` values (`format!`, owned clones)
    /// flow in as `Cow::Owned` without an extra round-trip. The
    /// `'static` bound keeps the type free of lifetime parameters
    /// so `ActivityLine` / `ActivitySurfaceView` / `TurnActivityView`
    /// don't have to thread a lifetime through every renderer.
    pub(crate) text: Cow<'static, str>,
    pub(crate) tone: ActivityTone,
    pub(crate) bold: bool,
    /// Per-agent badge color. When `Some`, the renderer tints this span
    /// with the agent's assigned color instead of the `tone` foreground
    /// (TS `AgentProgressLine` parity). `None` ⇒ plain `tone` styling.
    pub(crate) color: Option<coco_types::AgentColorName>,
}

impl ActivitySpan {
    pub(crate) fn raw(text: impl Into<Cow<'static, str>>) -> Self {
        Self {
            text: text.into(),
            tone: ActivityTone::Text,
            bold: false,
            color: None,
        }
    }

    pub(crate) fn tone(text: impl Into<Cow<'static, str>>, tone: ActivityTone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: false,
            color: None,
        }
    }

    pub(crate) fn bold(text: impl Into<Cow<'static, str>>, tone: ActivityTone) -> Self {
        Self {
            text: text.into(),
            tone,
            bold: true,
            color: None,
        }
    }

    /// Set the per-agent badge color (builder).
    pub(crate) fn with_color(mut self, color: Option<coco_types::AgentColorName>) -> Self {
        self.color = color;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivityLine {
    pub(crate) spans: Vec<ActivitySpan>,
}

impl ActivityLine {
    pub(crate) fn blank() -> Self {
        Self { spans: Vec::new() }
    }

    pub(crate) fn section(title: impl Into<Cow<'static, str>>) -> Self {
        Self {
            spans: vec![ActivitySpan::bold(title, ActivityTone::Accent)],
        }
    }

    pub(crate) fn text(text: impl Into<Cow<'static, str>>, tone: ActivityTone) -> Self {
        Self {
            spans: vec![ActivitySpan::tone(text, tone)],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivitySurfaceView {
    pub(crate) title: ActivityTitle,
    pub(crate) border: ActivityBorder,
    pub(crate) lines: Vec<ActivityLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TurnActivityView {
    None,
    Surface(ActivitySurfaceView),
}

pub(crate) fn turn_activity_view(state: &AppState, width: u16) -> TurnActivityView {
    // The Agents panel is a transient stage view (like the inline Tools
    // list), not an accumulating log: it shows only *live* agents and
    // disappears once none remain. Completed one-shot subagents drop out
    // and their final summary commits to the transcript (Agent tool result
    // cell), so they never pile up here across batches.
    let has_subagents = state.session.subagents.iter().any(is_live_agent);
    // Include `active_tasks` so the Tasks panel can open when only running
    // background tasks (bash / agent) exist, without any plan items or
    // per-agent todos. Before: opening ctrl+t after `run_in_background`
    // showed an empty pane because the gate ignored `active_tasks`.
    let has_plan_activity = !state.session.plan_tasks.is_empty()
        || !state.session.todos_by_agent.is_empty()
        || !state.session.active_tasks.is_empty();

    if matches!(state.session.expanded_view, ExpandedView::Tasks) && has_plan_activity {
        return TurnActivityView::Surface(limit_surface_rows(plan_surface(state), width));
    }

    if matches!(state.session.expanded_view, ExpandedView::Teammates) && has_subagents {
        return TurnActivityView::Surface(limit_surface_rows(agent_surface(state), width));
    }

    if has_subagents {
        TurnActivityView::Surface(limit_surface_rows(agent_surface(state), width))
    } else if state.session.stream_stall {
        // No separate "Activity / Tools:" panel for ordinary single-agent tool
        // runs — in-flight tools render inline in the transcript as
        // `● Tool(args) (elapsed)` plus the bottom status spinner (codex /
        // claude-code parity). The panel survives only for a stream stall
        // (warning) and the swarm / task surfaces handled above.
        TurnActivityView::Surface(limit_surface_rows(activity_surface(state), width))
    } else {
        TurnActivityView::None
    }
}

pub(crate) fn inline_activity_height(
    view: &TurnActivityView,
    available_height: u16,
    width: u16,
) -> u16 {
    let TurnActivityView::Surface(surface) = view else {
        return 0;
    };
    if available_height == 0 || surface.lines.is_empty() {
        return 0;
    }
    let visible_rows = surface.lines.len().min(activity_row_budget(width) as usize) as u16;
    (visible_rows + 1).min(available_height)
}

fn limit_surface_rows(mut surface: ActivitySurfaceView, width: u16) -> ActivitySurfaceView {
    let budget = activity_row_budget(width) as usize;
    if surface.lines.len() > budget {
        surface.lines.truncate(budget.saturating_sub(1));
        surface.lines.push(ActivityLine::text(
            t!("activity.more").to_string(),
            ActivityTone::Dim,
        ));
    }
    surface
}

fn activity_row_budget(width: u16) -> u16 {
    if width <= 60 {
        INLINE_ACTIVITY_ROWS_NARROW
    } else if width <= 100 {
        INLINE_ACTIVITY_ROWS_NORMAL
    } else {
        INLINE_ACTIVITY_ROWS_WIDE
    }
}

fn plan_surface(state: &AppState) -> ActivitySurfaceView {
    let mut lines = status_activity_lines(state);
    append_plan_lines(state, &mut lines);
    append_tool_lines(state, &mut lines);
    trim_trailing_blank(&mut lines);
    if lines.is_empty() {
        lines.push(ActivityLine::text(
            format!("  {}", t!("plan_panel.empty")),
            ActivityTone::Dim,
        ));
    }
    ActivitySurfaceView {
        title: ActivityTitle::Tasks,
        border: ActivityBorder::Plan,
        lines,
    }
}

fn agent_surface(state: &AppState) -> ActivitySurfaceView {
    let mut lines = status_activity_lines(state);
    if state.ui.coordinator_mode_active {
        append_coordinator_lines(state, &mut lines);
    } else {
        append_subagent_lines(state, &mut lines);
    }
    append_tool_lines(state, &mut lines);
    trim_trailing_blank(&mut lines);
    if lines.is_empty() {
        lines.push(ActivityLine::text(
            format!("  {}", t!("subagent.no_agents")),
            ActivityTone::Dim,
        ));
    }
    ActivitySurfaceView {
        title: if state.ui.coordinator_mode_active {
            ActivityTitle::Coordinator
        } else {
            ActivityTitle::Agents
        },
        border: if state.ui.coordinator_mode_active {
            ActivityBorder::Coordinator
        } else {
            ActivityBorder::Agents
        },
        lines,
    }
}

/// Stall-only surface. The tool list moved inline to the transcript, so this
/// now carries just the stream-stall warning (`status_activity_lines`); it is
/// only reached when `stream_stall` is set.
fn activity_surface(state: &AppState) -> ActivitySurfaceView {
    let mut lines = status_activity_lines(state);
    trim_trailing_blank(&mut lines);
    ActivitySurfaceView {
        title: ActivityTitle::Activity,
        border: ActivityBorder::Activity,
        lines,
    }
}

fn status_activity_lines(state: &AppState) -> Vec<ActivityLine> {
    let mut lines = Vec::new();
    if state.session.stream_stall {
        lines.push(ActivityLine {
            spans: vec![
                ActivitySpan::tone("  ! ", ActivityTone::Warning),
                ActivitySpan::tone(
                    t!("toast.stream_stall_detected").to_string(),
                    ActivityTone::Warning,
                ),
            ],
        });
    }
    if !lines.is_empty() {
        lines.push(ActivityLine::blank());
    }
    lines
}

fn append_plan_lines(state: &AppState, lines: &mut Vec<ActivityLine>) {
    // V2 (plan_tasks) and V1 (todos_by_agent) are rendered through the
    // dedicated todo panel module so the V1/V2 mutual exclusion,
    // priority sort, and glyphs live in one place.
    crate::widgets::todo_panel::append_lines(state, lines);

    if !state.session.active_tasks.is_empty() {
        lines.push(ActivityLine::section(
            t!("plan_panel.section_running").to_string(),
        ));
        for task in &state.session.active_tasks {
            let (icon, tone) = match task.status {
                TaskEntryStatus::Running => ("●", ActivityTone::Running),
                TaskEntryStatus::Completed => ("✓", ActivityTone::Completed),
                TaskEntryStatus::Failed => ("✗", ActivityTone::Error),
                TaskEntryStatus::Stopped => ("◐", ActivityTone::Dim),
            };
            lines.push(ActivityLine {
                spans: vec![
                    ActivitySpan::raw("  "),
                    ActivitySpan::tone(format!("{icon} "), tone),
                    ActivitySpan::raw(task.description.clone()),
                ],
            });
        }
        lines.push(ActivityLine::blank());
    }
}

fn append_subagent_lines(state: &AppState, lines: &mut Vec<ActivityLine>) {
    // Tree mode: when the Teammates view is expanded we draw a leader
    // row at the top and prefix each subagent with ├─ / └─ tree
    // connectors (highlighted ╞═ / ╘═ on the focused row). Otherwise
    // we keep the flat 2-space indent used by the inline activity surface.
    let tree_mode = matches!(
        state.session.expanded_view,
        coco_types::ExpandedView::Teammates
    );
    let now = state.clock.now_ms();
    // Aggregate counters over the current wave (live agents + their
    // just-finished siblings) so the header can show `completed/total` and
    // keep finished agents' tokens / cost in the tally. Anchoring on the
    // running batch keeps this scoped to the current stage, not a running
    // cross-batch sum. The per-agent *rows* below stay live-only.
    let agg = SubagentAggregate::of(&state.session.subagents);
    let summary = agg.summary_text();

    if tree_mode && !state.session.subagents.is_empty() {
        let mut leader = vec![
            ActivitySpan::tone("● ", ActivityTone::Accent),
            ActivitySpan::bold("Leader", ActivityTone::Text),
            ActivitySpan::tone(" (lead)", ActivityTone::Dim),
        ];
        if let Some(summary) = &summary {
            leader.push(ActivitySpan::tone(
                format!("   {summary}"),
                ActivityTone::Dim,
            ));
        }
        lines.push(ActivityLine { spans: leader });
    } else if let Some(summary) = &summary {
        // Flat (compact inline) view has no leader row — surface the
        // aggregate as a one-line header so the subagent cost stays
        // visible without expanding the Teammates view.
        lines.push(ActivityLine {
            spans: vec![ActivitySpan::tone(
                format!("  {summary}"),
                ActivityTone::Dim,
            )],
        });
    }
    // Only live agents render (transient stage view). Keep the original
    // index for focus matching; use the in-list position for tree
    // connectors so `└─` lands on the last *visible* row.
    let live: Vec<(usize, &crate::state::SubagentInstance)> = state
        .session
        .subagents
        .iter()
        .enumerate()
        .filter(|(_, agent)| is_live_agent(agent))
        .collect();
    let total = live.len();
    // Merged agent switcher: while it holds focus (`Shift+↑/↓`), the panel
    // doubles as the switcher — a `❯` cursor on the selected agent (index
    // aligned with `SessionState::switcher_agents`, no `◯ main` row). The
    // viewed agent carries a `◀` marker; a hint line lists the keys.
    let switcher_active = state.ui.focus == crate::state::FocusTarget::AgentSwitcher;
    let switcher_sel = state.ui.agent_switcher_selected;
    for (pos, (i, agent)) in live.iter().enumerate() {
        let i = *i;
        let is_focused = state.session.focused_subagent_index == Some(i as i32);
        let is_switcher_sel = switcher_active && switcher_sel == pos;
        let is_viewed = state.session.viewing_agent_id.as_deref() == Some(agent.agent_id.as_str());
        // Backgrounded is orthogonal to status — a Running agent flipped
        // to background renders with the dim half-circle so the user can
        // tell it's detached but still alive.
        let (icon, tone) = if agent.is_backgrounded {
            ("◐", ActivityTone::Dim)
        } else {
            match agent.status {
                SubagentStatus::Running => ("●", ActivityTone::Running),
                SubagentStatus::Completed => ("✓", ActivityTone::Completed),
                SubagentStatus::Failed => ("✗", ActivityTone::Error),
            }
        };
        // Tree prefix in Teammates view: ╞═ / ╘═ for the focused row
        // (TS highlight state), ├─ / └─ otherwise. Falls back to the
        // existing 2-space focus marker in flat mode. All branches
        // return string literals, so we keep this as `&'static str`
        // and let `ActivitySpan::tone` do the single `String::from`
        // at the boundary instead of allocating twice.
        let row_prefix: &'static str = if switcher_active {
            // The switcher cursor overrides the passive focus/tree markers.
            if is_switcher_sel { "❯ " } else { "  " }
        } else if tree_mode {
            match (is_focused, pos + 1 == total) {
                (true, true) => "╘═ ",
                (true, false) => "╞═ ",
                (false, true) => "└─ ",
                (false, false) => "├─ ",
            }
        } else if is_focused {
            "▸ "
        } else {
            "  "
        };
        // Kind badge: differentiate persistent team members (`@name` prefix)
        // from Agent-tool workers (plain agent_type).
        let agent_type = &agent.agent_type;
        let label = match agent.kind {
            crate::state::SubagentKind::Teammate => match agent.team_name.as_deref() {
                Some(team) => format!("@{agent_type}@{team}"),
                None => format!("@{agent_type}"),
            },
            crate::state::SubagentKind::Subagent => agent_type.clone(),
        };
        // Lead with the kind badge (`Explore: …`, `@name@team: …`) so the
        // type reads first and the descriptions stay left-aligned under it.
        // Tint the badge with its assigned color (TS `AgentProgressLine`
        // parity); falls back to Dim when unset.
        let mut spans = vec![
            ActivitySpan::tone(row_prefix, ActivityTone::Dim),
            ActivitySpan::tone(format!("{icon} "), tone),
            ActivitySpan::tone(format!("{label}: "), ActivityTone::Dim).with_color(agent.color),
            ActivitySpan::raw(truncate_chars(&agent.description, SUBAGENT_DESC_MAX_CHARS)),
        ];

        // One line per agent: tool count · (frozen) elapsed · tokens ·
        // live action / terminal verb, joined by " · ". Collapsing the
        // former status-row + progress-subline + completion-summary into
        // a single row drops the duplicate duration field and keeps each
        // subagent to exactly one line.
        let mut stats: Vec<String> = Vec::new();
        if agent.tool_count > 0 {
            stats.push(format!("{} tools", agent.tool_count));
        }
        if let Some(started_ms) = agent.started_at_ms {
            // Terminal agents freeze at `completed_at_ms` so a finished
            // subagent's timer stops instead of tracking `now()` with its
            // still-running siblings.
            let end_ms = if matches!(agent.status, SubagentStatus::Running) {
                now
            } else {
                agent.completed_at_ms.unwrap_or(now)
            };
            stats.push(format_short_elapsed((end_ms - started_ms).max(0)));
        }
        // Split token counter: `↑in ↓out` (+ cache-hit %) once the
        // in/out breakdown has arrived; fall back to the combined `↕total`
        // for the brief window before the first usage report.
        if agent.input_tokens > 0 || agent.output_tokens > 0 {
            stats.push(format!(
                "↑{} ↓{}",
                format_short_tokens(agent.input_tokens),
                format_short_tokens(agent.output_tokens)
            ));
            if agent.input_tokens > 0 && agent.cache_read_tokens > 0 {
                let pct = (agent.cache_read_tokens * 100 / agent.input_tokens).clamp(0, 100);
                stats.push(format!("cache {pct}%"));
            }
        } else if agent.total_tokens > 0 {
            stats.push(format!("↕{}", format_short_tokens(agent.total_tokens)));
        }
        // Per-agent spend, mirroring the header aggregate and the committed
        // transcript run summary. Populated on completion (the progress
        // heartbeat carries tokens but not cost), so it tails the token split.
        if agent.cost_usd > 0.0 {
            stats.push(format!("${:.2}", agent.cost_usd));
        }
        let tail = match agent.status {
            SubagentStatus::Running => {
                crate::widgets::activity_summary::summarize_trailing(&agent.recent_activities)
                    .or_else(|| agent.last_tool_name.clone())
            }
            SubagentStatus::Completed => Some("Done".to_string()),
            SubagentStatus::Failed => Some("Failed".to_string()),
        };
        if let Some(tail) = tail {
            stats.push(truncate_chars(&tail, SUBAGENT_TAIL_MAX_CHARS));
        }
        if !stats.is_empty() {
            spans.push(ActivitySpan::tone(
                format!("  {}", stats.join(" · ")),
                ActivityTone::Dim,
            ));
        }
        // Mark the agent whose conversation is open in the read-only overlay.
        if is_viewed {
            spans.push(ActivitySpan::tone(" ◀", ActivityTone::Accent));
        }
        // The switcher-selected row recolors its WHOLE line to accent (not just
        // the `❯` cursor) so the selection reads at a glance.
        if is_switcher_sel {
            for span in &mut spans {
                span.tone = ActivityTone::Accent;
                span.color = None;
            }
        }
        lines.push(ActivityLine { spans });

        // Backgrounded but still alive — hint the user how to bring it back.
        // After the underlying task terminates the flag stays set but the
        // status icon already conveys the outcome, so the hint stays useful
        // only while running.
        if agent.is_backgrounded && matches!(agent.status, SubagentStatus::Running) {
            lines.push(ActivityLine {
                spans: vec![
                    ActivitySpan::raw("      "),
                    ActivitySpan::tone("↓ manage · Ctrl+T → Subagents", ActivityTone::Dim),
                ],
            });
        }

        if state.ui.show_teammate_message_preview {
            // Engine-pushed teammate Informational entries (Commit 2
            // routes them via `UserCommand::PushSystemMessage` with
            // `title = "teammate:<agent_id>"`) land in the engine's
            // `MessageHistory` and surface as cells.
            let cells = state.session.transcript.cells();
            for preview in last_preview_lines(cells, &agent.agent_id, 3) {
                lines.push(ActivityLine {
                    spans: vec![
                        ActivitySpan::raw("    "),
                        ActivitySpan::tone(preview, ActivityTone::Dim),
                    ],
                });
            }
        }
    }

    // Switcher key hints. When focused, list the in-switcher keys; otherwise
    // surface the entry affordance so `Shift+↑` is discoverable. Only shown
    // when there's at least one switchable agent.
    if switcher_active {
        lines.push(ActivityLine {
            spans: vec![ActivitySpan::tone(
                t!("switcher.panel_hint").to_string(),
                ActivityTone::Dim,
            )],
        });
    } else if !live.is_empty() {
        lines.push(ActivityLine {
            spans: vec![ActivitySpan::tone(
                format!("  {}", t!("switcher.hint_collapsed")),
                ActivityTone::Dim,
            )],
        });
    }
}

/// Whether a subagent row is shown in the (transient) Agents panel.
/// Persistent teammates always show; one-shot subagents show only while
/// `Running`. Completed subagents drop out — their final summary lands in
/// the transcript via the Agent tool result cell, so the panel stays a
/// current-stage view instead of an accumulating log.
fn is_live_agent(agent: &crate::state::SubagentInstance) -> bool {
    matches!(agent.kind, crate::state::SubagentKind::Teammate)
        || matches!(agent.status, SubagentStatus::Running)
}

/// Aggregate counters across the current *wave* of subagents, driving the
/// leader/header summary line. The wave is the batch spawned together: every
/// live subagent plus any sibling that has already finished but started
/// alongside the still-running ones. Keeping finished siblings in the tally
/// lets the `completed/total` fraction climb (0/3 → 1/3 → 2/3) instead of the
/// denominator shrinking as agents land, and stops their tokens / cost from
/// vanishing from the header the moment they complete.
struct SubagentAggregate {
    /// One-shot subagents in the wave — the `completed/total` denominator.
    total: usize,
    /// Wave subagents that reached a terminal state — the numerator.
    completed: usize,
    /// Live rows of any kind (incl. persistent teammates); the fallback
    /// label when the wave holds no one-shot subagent.
    live: usize,
    tools: i32,
    input: i64,
    output: i64,
    cache_read: i64,
    cost: f64,
}

impl SubagentAggregate {
    /// A finished sibling that started within this slack of the earliest
    /// still-running subagent counts as part of the same wave; agents from a
    /// prior batch started long before the anchor and drop out.
    const WAVE_ANCHOR_SLACK_MS: i64 = 2_000;

    fn of(subagents: &[crate::state::SubagentInstance]) -> Self {
        // Anchor the wave on the earliest start among running one-shot
        // subagents. A terminal subagent belongs to the wave only while a
        // sibling is still running (anchor present) and it started no earlier
        // than that sibling (minus a small spawn-stagger slack) — so a single
        // running batch keeps its finished members, but a previously-completed
        // batch can't inflate the count once a new batch begins.
        let anchor = subagents
            .iter()
            .filter(|a| {
                matches!(a.kind, crate::state::SubagentKind::Subagent)
                    && matches!(a.status, SubagentStatus::Running)
            })
            .filter_map(|a| a.started_at_ms)
            .min();
        let in_wave = |a: &crate::state::SubagentInstance| match a.kind {
            crate::state::SubagentKind::Teammate => is_live_agent(a),
            crate::state::SubagentKind::Subagent => match a.status {
                SubagentStatus::Running => true,
                SubagentStatus::Completed | SubagentStatus::Failed => {
                    anchor.is_some_and(|anchor| {
                        a.started_at_ms
                            .is_some_and(|started| started >= anchor - Self::WAVE_ANCHOR_SLACK_MS)
                    })
                }
            },
        };
        let mut a = SubagentAggregate {
            total: 0,
            completed: 0,
            live: 0,
            tools: 0,
            input: 0,
            output: 0,
            cache_read: 0,
            cost: 0.0,
        };
        for s in subagents.iter().filter(|s| in_wave(s)) {
            if matches!(s.kind, crate::state::SubagentKind::Subagent) {
                a.total += 1;
                if matches!(s.status, SubagentStatus::Completed | SubagentStatus::Failed) {
                    a.completed += 1;
                }
            }
            if is_live_agent(s) {
                a.live += 1;
            }
            a.tools += s.tool_count;
            a.input += s.input_tokens;
            a.output += s.output_tokens;
            a.cache_read += s.cache_read_tokens;
            a.cost += s.cost_usd;
        }
        a
    }

    /// `Subagents (1/3) · 99 tools · ↑128k ↓4k · cache 92% · $0.42`. The
    /// `completed/total` fraction shows how many of the current batch have
    /// landed; the token / cache / cost segments fill in as usage arrives, so
    /// the line is present from the first spawn instead of waiting on the
    /// first token report. Falls back to a bare count for teammate-only panels.
    fn summary_text(&self) -> Option<String> {
        if self.total == 0 && self.live == 0 {
            return None;
        }
        let head = if self.total > 0 {
            format!("Subagents ({}/{})", self.completed, self.total)
        } else {
            format!("Subagents ({})", self.live)
        };
        let mut parts = vec![head];
        if self.tools > 0 {
            parts.push(format!("{} tools", self.tools));
        }
        if self.input > 0 || self.output > 0 {
            parts.push(format!(
                "↑{} ↓{}",
                format_short_tokens(self.input),
                format_short_tokens(self.output)
            ));
            if self.input > 0 && self.cache_read > 0 {
                let pct = (self.cache_read * 100 / self.input).clamp(0, 100);
                parts.push(format!("cache {pct}%"));
            }
        }
        if self.cost > 0.0 {
            parts.push(format!("${:.2}", self.cost));
        }
        Some(parts.join(" · "))
    }
}

fn append_coordinator_lines(state: &AppState, lines: &mut Vec<ActivityLine>) {
    for (i, agent) in state.session.subagents.iter().enumerate() {
        let is_selected = state.session.focused_subagent_index == Some(i as i32);
        let selector = if is_selected { "▸ " } else { "  " };
        let is_running = matches!(agent.status, SubagentStatus::Running);
        let status_icon = if is_running { "▶" } else { "⏸" };
        let status_tone = if is_running {
            ActivityTone::Running
        } else {
            ActivityTone::Dim
        };
        let desc = truncate_chars(&agent.description, 48);
        let mut spans = vec![
            ActivitySpan::raw(selector),
            ActivitySpan::tone(format!("{status_icon} "), status_tone),
            ActivitySpan::raw(desc),
            ActivitySpan::tone(" 0s", ActivityTone::Dim),
        ];
        if agent.total_tokens > 0 {
            spans.push(ActivitySpan::tone(
                format!(" ↕{}", format_short_tokens(agent.total_tokens)),
                ActivityTone::Dim,
            ));
        }
        lines.push(ActivityLine { spans });
    }
}

fn append_tool_lines(state: &AppState, lines: &mut Vec<ActivityLine>) {
    let tools: Vec<_> = state
        .session
        .tool_executions
        .iter()
        .filter(|tool| {
            matches!(
                tool.status,
                ToolStatus::Streaming | ToolStatus::Queued | ToolStatus::Running
            )
        })
        // Agent (subagent) spawns are shown as their own rows above in the
        // Agents panel — drop them from the "Tools:" section so the same
        // spawn isn't listed twice (and the two surfaces can't disagree on
        // count, e.g. 1 panel row vs 3 `⏳ Agent(...)` rows).
        .filter(|tool| tool.name != coco_types::ToolName::Agent.as_str())
        .collect();
    if tools.is_empty() {
        return;
    }

    if !lines.is_empty() && !matches!(lines.last(), Some(line) if line.spans.is_empty()) {
        lines.push(ActivityLine::blank());
    }
    lines.push(ActivityLine::section(
        t!("activity.section_tools").to_string(),
    ));
    let display_count = tools.len().min(MAX_TOOL_ACTIVITY_DISPLAY);
    let start = tools.len().saturating_sub(display_count);
    for tool in &tools[start..] {
        let (icon, tone) = match tool.status {
            // Streaming shares the queued glyph — both read as "pending"; the
            // growing arg preview already signals that input is arriving.
            ToolStatus::Streaming | ToolStatus::Queued => ("◦", ActivityTone::Running),
            ToolStatus::Running => ("⏳", ActivityTone::Running),
            ToolStatus::Completed => ("✓", ActivityTone::Completed),
            ToolStatus::Failed => ("✗", ActivityTone::Error),
        };
        let elapsed = tool.elapsed();
        let elapsed_str = if elapsed.as_secs() >= 60 {
            format!("{}m{}s", elapsed.as_secs() / 60, elapsed.as_secs() % 60)
        } else if elapsed.as_secs() > 0 {
            format!("{}s", elapsed.as_secs())
        } else {
            format!("{}ms", elapsed.as_millis())
        };
        // Mirror the committed-transcript invocation header
        // (`assistant.rs`): `{name}({arg preview})`. Prefer the
        // input-derived preview; fall back to a non-arg `description`
        // (e.g. MCP progress text) so the row always carries whatever
        // detail is available beyond the bare tool name.
        let mut spans = vec![
            ActivitySpan::tone(format!("{icon} "), tone),
            ActivitySpan::raw(tool.name.clone()),
        ];
        let preview = tool
            .input_preview
            .as_deref()
            .or(tool.description.as_deref())
            .filter(|preview| !preview.is_empty());
        if let Some(preview) = preview {
            let truncated = truncate_chars(preview, constants::TOOL_DESCRIPTION_MAX_CHARS as usize);
            spans.push(ActivitySpan::raw("("));
            spans.push(ActivitySpan::tone(truncated, ActivityTone::Dim));
            spans.push(ActivitySpan::raw(")"));
        }
        spans.push(ActivitySpan::tone(
            format!(" ({elapsed_str})"),
            ActivityTone::Dim,
        ));
        lines.push(ActivityLine { spans });
    }
    lines.push(ActivityLine::blank());
}

/// Walk the engine-authoritative cells in reverse for the latest
/// `n` non-blank teammate-attributed preview lines. Teammate messages
/// arrive as `SystemMessage::Informational` cells whose title is
/// `teammate:<agent_id>` — that convention is set in
/// `server_notification_handler::protocol::push_teammate_message`.
fn last_preview_lines(
    cells: &[crate::transcript::cells::RenderedCell],
    teammate_id: &str,
    n: usize,
) -> Vec<String> {
    let prefix = format!("teammate:{teammate_id}");
    let mut lines: Vec<String> = Vec::new();
    for cell in cells.iter().rev() {
        let coco_messages::Message::System(coco_messages::SystemMessage::Informational(info)) =
            cell.source.as_ref()
        else {
            continue;
        };
        if info.title != prefix {
            continue;
        }
        for line in info.message.lines().rev() {
            if lines.len() >= n {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            lines.push(trimmed.to_string());
        }
        if lines.len() >= n {
            break;
        }
    }
    lines.reverse();
    lines
}

fn format_short_elapsed(ms: i64) -> String {
    let secs = (ms / 1000).max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3_600, (secs % 3_600) / 60)
    }
}

pub(crate) fn format_short_tokens(total: i64) -> String {
    if total < 1_000 {
        format!("{total}")
    } else {
        format!("{:.1}k", total as f64 / 1_000.0)
    }
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.width() <= max_chars {
        value.to_string()
    } else if max_chars == 0 {
        String::new()
    } else {
        format!(
            "{}…",
            value
                .chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    }
}

fn trim_trailing_blank(lines: &mut Vec<ActivityLine>) {
    while matches!(lines.last(), Some(line) if line.spans.is_empty()) {
        lines.pop();
    }
}

#[cfg(test)]
#[path = "activity.test.rs"]
mod tests;
