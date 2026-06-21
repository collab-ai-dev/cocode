//! Prompt and modal payload types.
//!
//! The active surface state lives in `InteractionPaneState` and `ModalState`.
//! This module keeps the concrete payload structs shared by those enums and
//! their render/update code.

use super::session::ProviderUnavailableReason;

/// Permission approval state with tool-specific detail.
///
/// Each tool type has a specialized review UI.
#[derive(Debug, Clone)]
pub struct PermissionPromptState {
    pub request_id: String,
    pub tool_name: String,
    pub description: String,
    pub detail: PermissionDetail,
    /// Risk level badge from the permission explainer — color-coded
    /// LOW/MEDIUM/HIGH badge.
    pub risk_level: Option<RiskLevel>,
    /// Whether "Always Allow" option should be shown (gated by policy).
    pub show_always_allow: bool,
    /// Whether a background classifier check is in progress.
    pub classifier_checking: bool,
    /// Set when classifier auto-approved; shows checkmark before dismissal.
    pub classifier_auto_approved: Option<String>,
    /// Optional multi-choice payload. `None` means render the classic
    /// yes/no/always dialog. `Some` switches the renderer into a
    /// choice-list mode: Up/Down moves `selected_choice`, Enter (approve)
    /// echoes the picked value back to the tool via
    /// `UserCommand::ApprovalResponse.resolution_detail`.
    pub choices: Option<Vec<coco_types::PermissionAskChoice>>,
    /// Cursor position within `choices`, or within the classic
    /// approve / always-allow / deny action list when `choices.is_none()`.
    pub selected_choice: usize,
    /// Bounded, sanitized display projection of the tool input.
    /// Raw input must stay in `original_input` and must not be read by
    /// presentation code.
    pub display_input: coco_types::PermissionDisplayInput,
    /// Raw tool input captured at dialog-open time. Classic read dialogs use
    /// it to build path-scoped "always allow" updates.
    pub original_input: Option<serde_json::Value>,
    /// Tool execution working directory. Relative paths in `original_input`
    /// are resolved against this cwd when deriving scoped allow updates.
    pub cwd: Option<String>,
    /// Permission updates suggested by core for "always allow".
    /// Prefer these over UI-side inference.
    pub permission_suggestions: Vec<coco_types::PermissionUpdate>,
    /// Identity badge of the requesting cross-process teammate, if any.
    /// Rendered in the prompt title so the leader sees who is asking;
    /// `None` for the leader's own in-process requests.
    pub worker_badge: Option<coco_types::WorkerBadge>,
    /// Lazy Ctrl+E risk-explainer panel (TS `PermissionExplanation.tsx`):
    /// whether the panel is currently expanded.
    pub explanation_visible: bool,
    /// Lazily-fetched LLM risk explanation. Fetched on first Ctrl+E toggle and
    /// cached so re-toggling doesn't re-query (TS `createExplanationPromise`).
    pub explanation: ExplainerFetch,
    /// Editable "always allow" prefix for shell tools (`Bash` / `PowerShell`).
    /// `Some` only when `show_always_allow` and the tool is a shell command —
    /// seeded with the default `command subcommand:*` / `command:*` / exact rule
    /// (TS `BashPermissionRequest` editable field). When an allow row is focused
    /// the field becomes editable; committing that row writes `Bash(<value>)`
    /// instead of the engine-suggested rule.
    pub prefix_input: Option<PrefixInputState>,
}

/// A single-line editable text field for the permission dialog's "always
/// allow" rule prefix, with a cursor for inline readline-style editing.
#[derive(Debug, Clone)]
pub struct PrefixInputState {
    /// Current rule text (e.g. `git status:*`). Empty → commit allows once.
    pub value: String,
    /// Cursor byte offset into `value`; always on a char boundary.
    pub cursor: usize,
}

impl PrefixInputState {
    /// Seed the field with `value`, cursor at the end.
    pub fn new(value: String) -> Self {
        let cursor = value.len();
        Self { value, cursor }
    }

    /// Insert `c` at the cursor.
    pub fn insert(&mut self, c: char) {
        self.value.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the char before the cursor (Backspace).
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.value[..self.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(0);
        self.cursor -= prev;
        self.value
            .replace_range(self.cursor..self.cursor + prev, "");
    }

    /// Delete the whitespace-delimited word before the cursor (Ctrl+W).
    pub fn delete_word_backward(&mut self) {
        let head = &self.value[..self.cursor];
        let trimmed = head.trim_end_matches(|c: char| c.is_whitespace());
        let start = trimmed
            .rfind(char::is_whitespace)
            .map(|i| i + 1)
            .unwrap_or(0);
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Move the cursor one char left.
    pub fn left(&mut self) {
        if let Some(c) = self.value[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    /// Move the cursor one char right.
    pub fn right(&mut self) {
        if let Some(c) = self.value[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    /// Move the cursor to the start.
    pub fn home(&mut self) {
        self.cursor = 0;
    }

    /// Move the cursor to the end.
    pub fn end(&mut self) {
        self.cursor = self.value.len();
    }
}

/// Lifecycle of the lazily-fetched permission risk explanation. Fetched once
/// on the first Ctrl+E press rather than eagerly on every prompt.
#[derive(Debug, Clone, Default)]
pub enum ExplainerFetch {
    /// Not yet requested (panel never opened, or the explainer is disabled).
    #[default]
    NotFetched,
    /// Fetch in flight — the panel shows a loading line.
    Loading,
    /// Explanation available.
    Ready(coco_types::PermissionExplanation),
    /// Fetch failed or the explainer is disabled — "explanation unavailable".
    Unavailable,
}

/// Risk level for permission explainer badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

/// Tool-specific permission review content.
///
/// 12 specialized permission request variants.
#[derive(Debug, Clone)]
pub enum PermissionDetail {
    /// Bash command — show command, risk level, working directory.
    Bash {
        command: String,
        risk_description: Option<String>,
        working_dir: Option<String>,
    },
    /// File edit — show path and unified diff.
    FileEdit { path: String, diff: String },
    /// File write — show path and content preview.
    FileWrite {
        path: String,
        content_preview: String,
        is_new_file: bool,
    },
    /// Filesystem operation (mkdir, rm, mv, cp).
    Filesystem { operation: String, path: String },
    /// Web fetch — show URL.
    WebFetch { url: String, method: String },
    /// Skill execution — show skill name and description.
    Skill {
        skill_name: String,
        skill_description: Option<String>,
    },
    /// Sed in-place edit — show pattern and replacement.
    SedEdit {
        path: String,
        pattern: String,
        replacement: String,
    },
    /// Notebook cell edit — show path, cell, and change.
    NotebookEdit {
        path: String,
        cell_id: String,
        change_preview: String,
    },
    /// MCP tool call — show server and tool.
    McpTool {
        server_name: String,
        tool_name: String,
        input_preview: String,
    },
    /// PowerShell command approval.
    PowerShell {
        command: String,
        risk_description: Option<String>,
        working_dir: Option<String>,
    },
    /// Computer use (screen/mouse/keyboard) approval.
    ComputerUse { action: String, description: String },
    /// ExitPlanMode approval — show the plan and response choices.
    ExitPlanMode {
        outcome: coco_types::ExitPlanModeOutcome,
        plan: Option<String>,
        plan_file_path: Option<String>,
        allowed_prompts: Vec<coco_types::ExitPlanModeAllowedPrompt>,
    },
    /// Generic fallback — plain text description.
    Generic { input_preview: String },
}

/// Cost warning state.
#[derive(Debug, Clone)]
pub struct CostWarningPromptState {
    pub current_cost_cents: i64,
    pub threshold_cents: i64,
}

/// Model picker state — provider-grouped list of `(provider, model_id)`
/// candidates plus an inline thinking-effort selector. Tab cycles the
/// target role (Main / Fast / Plan / …); the confirm path persists
/// to that role's slot in `~/.coco.json::model_roles`.
#[derive(Debug, Clone)]
pub struct ModelPickerState {
    /// Which role we're configuring. Defaults to `Main` when launched
    /// by `Ctrl+M` / `/model`; Tab cycles forward, Shift+Tab back.
    pub role: coco_types::ModelRole,
    /// Available model entries, pre-sorted by `(provider, display_name)`
    /// so the rendered list is stable and provider headers fall
    /// naturally between consecutive same-provider rows.
    pub entries: Vec<ModelEntry>,
    /// Substring filter, lowercased — matches `display_name` and
    /// `provider_display`.
    pub filter: String,
    /// Index into the *filtered* entries list (0-based, headers skipped
    /// because they aren't selectable rows).
    pub selected: i32,
    /// Currently-chosen effort for the focused model. Re-derived from
    /// `default_effort` on every selection change (see `update::interaction`).
    /// `None` when the focused model declares no thinking levels.
    pub effort: Option<coco_types::ReasoningEffort>,
}

/// Teams roster picker — lets the leader cycle a teammate's permission
/// mode. Members come from `session.subagents` (kind == Teammate); each
/// member's CURRENT mode is seeded fresh from `team.json` so the picker shows
/// and cycles from the live mode (not a hardcoded default). Left/Right cycles
/// the focused member's mode in place; Enter dispatches it via
/// `UserCommand::SetTeammateMode`.
#[derive(Debug, Clone)]
pub struct TeamRosterState {
    /// Active team name (header).
    pub team_name: String,
    /// Running teammates, in roster order.
    pub members: Vec<TeamRosterMember>,
    /// Index of the focused teammate row.
    pub selected: usize,
}

/// One teammate row in the roster picker.
#[derive(Debug, Clone)]
pub struct TeamRosterMember {
    /// Display name (the `name` half of `name@team`), used as the
    /// `set_teammate_mode` target.
    pub name: String,
    /// Agent type label (e.g. `"explore"`), shown as a dim suffix.
    pub agent_type: String,
    /// Assigned palette color, if any.
    pub color: Option<coco_types::AgentColorName>,
    /// This teammate's CURRENT permission mode, seeded from `team.json` and
    /// cycled in place by Left/Right when focused. Per-member so divergent
    /// teammate modes are each shown and edited correctly (a single shared
    /// field could not represent that).
    pub mode: coco_types::PermissionMode,
}

/// One row in the picker — pre-resolved against the registry so the
/// renderer never has to reach back into config.
#[derive(Debug, Clone)]
pub struct ModelEntry {
    /// Canonical provider id (e.g. `"anthropic"`). Used as the persistence key.
    pub provider: String,
    /// Display label rendered in the section header (e.g. `"Anthropic"`).
    pub provider_display: String,
    pub model_id: String,
    /// Pretty name; falls back to `model_id` when registry lacks one.
    pub display_name: String,
    /// Context window in tokens — rendered as `"1M"` / `"200K"`.
    pub context_window: Option<i64>,
    /// Efforts the model supports; drives the inline footer cycle.
    /// Empty when the model has no thinking capability — the effort
    /// footer is then hidden.
    pub supported_efforts: Vec<coco_types::ReasoningEffort>,
    /// Model's preferred effort when none chosen explicitly.
    pub default_effort: Option<coco_types::ReasoningEffort>,
    /// `true` when this entry is the role's currently-applied selection.
    /// Rendered with a `[current]` badge.
    pub is_current_for_role: bool,
    /// Provider config issues that prevent this row from being selected.
    pub unavailable_reasons: Vec<ProviderUnavailableReason>,
}

/// Session browser state (list of saved sessions).
#[derive(Debug, Clone)]
pub struct SessionBrowserState {
    pub sessions: Vec<SessionOption>,
    pub filter: String,
    pub selected: i32,
}

/// A selectable session option.
#[derive(Debug, Clone)]
pub struct SessionOption {
    pub id: String,
    pub label: String,
    pub message_count: i32,
    pub created_at: String,
}

#[cfg(test)]
#[path = "surface_payloads.test.rs"]
mod surface_payload_tests;

/// Sandbox permission state.
#[derive(Debug, Clone)]
pub struct SandboxPermissionPromptState {
    pub request_id: String,
    pub description: String,
}

/// Plan mode entry state.
#[derive(Debug, Clone)]
pub struct PlanEntryPromptState {
    pub description: String,
}

/// Global search state (ripgrep streaming).
#[derive(Debug, Clone)]
pub struct GlobalSearchState {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub selected: i32,
    pub is_searching: bool,
}

/// A global search result entry.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub file: String,
    pub line_number: i32,
    pub content: String,
}

/// Quick file open state.
#[derive(Debug, Clone)]
pub struct QuickOpenState {
    pub filter: String,
    pub files: Vec<String>,
    pub selected: i32,
}

/// `/copy` picker state. The picker is mounted when the chosen
/// assistant message contains code blocks AND the user has not opted
/// into "always copy full response" via config.
#[derive(Debug, Clone)]
pub struct CopyPickerState {
    /// The full markdown source of the picked assistant message.
    pub full_text: String,
    /// Fenced code blocks extracted from `full_text`. Empty when the
    /// user opens the picker via the "always" path; otherwise at least
    /// one entry (the no-blocks case skips the picker entirely).
    pub code_blocks: Vec<CopyPickerCodeBlock>,
    /// 0 = latest, 1 = second-to-latest, …  Surfaced in the picker
    /// header so the user knows which turn they're copying.
    pub message_age: usize,
    /// Currently highlighted option.
    pub selected: CopyPickerSelection,
}

/// Identity of a code block inside the picker — owned copy so the
/// picker survives transcript mutation while it's open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyPickerCodeBlock {
    pub code: String,
    pub lang: Option<String>,
}

/// What the picker currently has selected. `Always` is the trailing
/// "Always copy full response" option that flips the
/// `copy_full_response` setting on confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyPickerSelection {
    Full,
    CodeBlock(usize),
    Always,
}

impl CopyPickerState {
    pub fn option_count(&self) -> usize {
        // Full + each code block + Always
        2 + self.code_blocks.len()
    }

    pub fn move_up(&mut self) {
        self.selected = match self.selected {
            CopyPickerSelection::Full if self.code_blocks.is_empty() => CopyPickerSelection::Always,
            CopyPickerSelection::Full => CopyPickerSelection::Always,
            CopyPickerSelection::CodeBlock(0) => CopyPickerSelection::Full,
            CopyPickerSelection::CodeBlock(n) => CopyPickerSelection::CodeBlock(n - 1),
            CopyPickerSelection::Always if self.code_blocks.is_empty() => CopyPickerSelection::Full,
            CopyPickerSelection::Always => {
                CopyPickerSelection::CodeBlock(self.code_blocks.len() - 1)
            }
        };
    }

    pub fn move_down(&mut self) {
        self.selected = match self.selected {
            CopyPickerSelection::Full if self.code_blocks.is_empty() => CopyPickerSelection::Always,
            CopyPickerSelection::Full => CopyPickerSelection::CodeBlock(0),
            CopyPickerSelection::CodeBlock(n) if n + 1 < self.code_blocks.len() => {
                CopyPickerSelection::CodeBlock(n + 1)
            }
            CopyPickerSelection::CodeBlock(_) => CopyPickerSelection::Always,
            CopyPickerSelection::Always => CopyPickerSelection::Full,
        };
    }
}

/// Export dialog state.
#[derive(Debug, Clone)]
pub struct ExportState {
    pub formats: Vec<ExportFormat>,
    pub selected: i32,
}

/// Supported export formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Markdown,
    Json,
    Text,
}

impl ExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown (.md)",
            Self::Json => "JSON (.json)",
            Self::Text => "Plain Text (.txt)",
        }
    }
}

/// Full-screen diff view state.
#[derive(Debug, Clone)]
pub struct DiffViewState {
    pub path: String,
    pub diff: String,
    pub scroll: i32,
}

/// MCP server approval state.
#[derive(Debug, Clone)]
pub struct McpServerApprovalPromptState {
    pub server_name: String,
    pub server_url: Option<String>,
    pub tools: Vec<String>,
    pub request_id: String,
}

/// Worktree exit confirmation state.
#[derive(Debug, Clone)]
pub struct WorktreeExitState {
    pub branch: String,
    pub has_uncommitted: bool,
    pub changed_files: Vec<String>,
}

/// Doctor/diagnostics state.
#[derive(Debug, Clone)]
pub struct DoctorState {
    pub checks: Vec<DoctorCheck>,
}

/// A single doctor check result.
#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

/// Bridge dialog state (IDE/REPL).
#[derive(Debug, Clone)]
pub struct BridgeState {
    pub bridge_type: String,
    pub status: String,
    pub details: String,
}

/// Invalid config warning state.
#[derive(Debug, Clone)]
pub struct InvalidConfigState {
    pub errors: Vec<String>,
}

/// Idle return confirmation state.
#[derive(Debug, Clone)]
pub struct IdleReturnState {
    pub idle_duration_secs: i64,
}

/// Trust dialog state.
#[derive(Debug, Clone)]
pub struct TrustState {
    pub path: String,
    pub description: String,
}

/// Bypass permissions confirmation state.
#[derive(Debug, Clone)]
pub struct BypassPermissionsState {
    pub current_mode: String,
}

/// Background task detail state.
#[derive(Debug, Clone)]
pub struct TaskDetailState {
    pub task_id: String,
    pub task_type: String,
    pub description: String,
    pub output: String,
    pub status: String,
    pub scroll: i32,
}

/// Background-tasks dialog state: a list of running shells/agents that drills
/// into a per-task detail view. The task rows are derived live from
/// `SessionState::running_background_tasks`; this struct holds only the
/// cursor and which layer is showing.
///
/// No output-scroll offset yet: the detail view's output is a placeholder
/// ("No output available") until live shell output is wired through a
/// TUI↔core round-trip, at which point a `scroll` field returns alongside the
/// renderer that reads it.
#[derive(Debug, Clone, Default)]
pub struct BackgroundTasksState {
    /// Index into the live row list (clamped on render).
    pub selected: usize,
    /// `Some(task_id)` when the detail layer is open for that task; `None`
    /// shows the list layer.
    pub detail: Option<String>,
}

/// Feedback survey state.
#[derive(Debug, Clone)]
pub struct FeedbackState {
    pub prompt: String,
    pub options: Vec<String>,
    pub selected: i32,
}

/// MCP server multi-select state.
#[derive(Debug, Clone)]
pub struct McpServerSelectState {
    pub servers: Vec<McpServerOption>,
    pub filter: String,
}

/// Selectable MCP server option.
#[derive(Debug, Clone)]
pub struct McpServerOption {
    pub name: String,
    pub selected: bool,
    pub tool_count: i32,
}

/// Plan-approval state shown to the team lead when a teammate sends
/// a `plan_approval_request` via mailbox. The leader picks approve /
/// deny (+ optional feedback); the TUI dispatches
/// `UserCommand::PlanApprovalResponse` which the engine writes back to
/// the teammate's inbox.
#[derive(Debug, Clone)]
pub struct PlanApprovalPromptState {
    /// Correlation id that will travel back in the response.
    pub request_id: String,
    /// Teammate agent name (who sent the request).
    pub from: String,
    /// Optional plan-file path on disk (.coco/plans/...). `None` when
    /// the request embeds the content inline instead.
    pub plan_file_path: Option<String>,
    /// The plan text itself (rendered markdown) — always present so the
    /// leader can review without opening a file.
    pub plan_content: String,
    /// Focused button index: 0 = Approve, 1 = Deny.
    pub focused: u8,
}

impl PlanApprovalPromptState {
    pub fn new(
        request_id: String,
        from: String,
        plan_file_path: Option<String>,
        plan_content: String,
    ) -> Self {
        Self {
            request_id,
            from,
            plan_file_path,
            plan_content,
            focused: 0,
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focused = if self.focused == 0 { 1 } else { 0 };
    }

    pub fn is_approve_focused(&self) -> bool {
        self.focused == 0
    }
}

/// `/memory` file-picker state state. Built from the
/// `TuiOnlyEvent::OpenMemoryDialog` payload; entries are pre-resolved
/// paths plus a label and scope tag. Selection is a simple index — there
/// is no filter (the entry count is small and fixed per session).
#[derive(Debug, Clone)]
pub struct MemoryDialogState {
    pub entries: Vec<MemoryDialogEntry>,
    pub selected: i32,
}

/// A single row in the memory picker — TUI-side mirror of
/// `coco_types::MemoryDialogEntry` so the state struct stays free of
/// the coco-types dependency at the field level.
#[derive(Debug, Clone)]
pub struct MemoryDialogEntry {
    pub path: std::path::PathBuf,
    pub label: String,
    pub scope: MemoryDialogScope,
    pub row_kind: MemoryDialogRowKind,
}

/// Scope tag for a memory file picker entry. Mirrors
/// `coco_types::MemoryDialogScope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryDialogScope {
    Managed,
    User,
    Project,
    ProjectLocal,
    ProjectConfig,
    Subdir,
    Imported,
    AutoMemFolder,
    TeamMemFolder,
    AgentMemFolder,
}

impl MemoryDialogScope {
    /// Build from the wire payload variant.
    pub fn from_wire(s: coco_types::MemoryDialogScope) -> Self {
        match s {
            coco_types::MemoryDialogScope::Managed => Self::Managed,
            coco_types::MemoryDialogScope::User => Self::User,
            coco_types::MemoryDialogScope::Project => Self::Project,
            coco_types::MemoryDialogScope::ProjectLocal => Self::ProjectLocal,
            coco_types::MemoryDialogScope::ProjectConfig => Self::ProjectConfig,
            coco_types::MemoryDialogScope::Subdir => Self::Subdir,
            coco_types::MemoryDialogScope::Imported => Self::Imported,
            coco_types::MemoryDialogScope::AutoMemFolder => Self::AutoMemFolder,
            coco_types::MemoryDialogScope::TeamMemFolder => Self::TeamMemFolder,
            coco_types::MemoryDialogScope::AgentMemFolder => Self::AgentMemFolder,
        }
    }
}

/// Semantic row kind for memory picker entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryDialogRowKind {
    File { exists: bool, read_only: bool },
    Folder { enabled: bool },
    Toggle { enabled: bool },
}

impl MemoryDialogRowKind {
    pub fn from_wire(kind: coco_types::MemoryDialogRowKind) -> Self {
        match kind {
            coco_types::MemoryDialogRowKind::File { exists, read_only } => {
                Self::File { exists, read_only }
            }
            coco_types::MemoryDialogRowKind::Folder { enabled } => Self::Folder { enabled },
            coco_types::MemoryDialogRowKind::Toggle { enabled } => Self::Toggle { enabled },
        }
    }

    pub fn is_file(self) -> bool {
        matches!(self, Self::File { .. })
    }
}

impl MemoryDialogState {
    /// Build from the wire payload (`TuiOnlyEvent::OpenMemoryDialog`).
    pub fn from_wire(entries: Vec<coco_types::MemoryDialogEntry>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|e| MemoryDialogEntry {
                    path: std::path::PathBuf::from(e.path),
                    label: e.label,
                    scope: MemoryDialogScope::from_wire(e.scope),
                    row_kind: MemoryDialogRowKind::from_wire(e.row_kind),
                })
                .collect(),
            selected: 0,
        }
    }
}

/// Standalone theme picker. Opened by `/theme`; navigating live-previews
/// the focused theme via an in-memory `apply_theme_setting`, Enter persists,
/// Esc restores `original_setting` so the preview never sticks.
#[derive(Debug, Clone)]
pub struct ThemePickerState {
    /// Auto + every built-in / custom theme, in display order.
    pub choices: Vec<crate::theme::ThemeChoice>,
    /// Index into `choices` (clamped to range by the renderer / nav).
    pub selected: i32,
    /// Theme setting active when the picker opened, restored on cancel.
    pub original_setting: crate::theme::ThemeSetting,
}

/// Interactive `/add-dir` (no-argument) overlay — a single-line directory-path
/// input with inline validation. Mirrors TS `AddWorkspaceDirectory`: Enter
/// validates the typed path and, on success, adds it to the session's working
/// directories; an invalid path keeps the form open with [`Self::error`] set.
/// Esc cancels. The actual add re-dispatches `/add-dir <path>` so the validated
/// session-add path is reused, never duplicated.
#[derive(Debug, Clone, Default)]
pub struct AddDirectoryState {
    /// Directory path the user is editing.
    pub input: super::WizardTextField,
    /// Localized validation error from the last failed confirm; cleared on the
    /// next edit so the message never lingers past a fix.
    pub error: Option<String>,
}

impl AddDirectoryState {
    pub fn new() -> Self {
        Self::default()
    }
}
