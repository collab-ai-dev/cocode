//! Bottom interaction-pane state.
//!
//! The pane owns composer-adjacent popups and agent/user prompts that should
//! remain in the retained viewport. Full-screen surfaces live in
//! [`crate::state::modal`].

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;
use std::time::Instant;

use crate::state::SuggestionKind;
use crate::state::question_prompt::QuestionPromptState;
use crate::state::surface_payloads::CostWarningPromptState;
use crate::state::surface_payloads::McpServerApprovalPromptState;
use crate::state::surface_payloads::PermissionPromptState;
use crate::state::surface_payloads::PlanApprovalPromptState;
use crate::state::surface_payloads::PlanEntryPromptState;
use crate::state::surface_payloads::SandboxPermissionPromptState;

const PERMISSION_PROMPT_DELAY: Duration = Duration::from_secs(1);

/// Validated slash command name without a leading slash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SlashCommandName(String);

impl SlashCommandName {
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidSlashCommandName> {
        let name = name.into();
        if name.is_empty()
            || name.contains('/')
            || name.chars().any(|ch| ch.is_whitespace() || ch.is_control())
        {
            return Err(InvalidSlashCommandName);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for SlashCommandName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl PartialEq<&str> for SlashCommandName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

/// Slash command name validation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidSlashCommandName;

impl fmt::Display for InvalidSlashCommandName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("slash command names must be non-empty and contain no slash, whitespace, or control characters")
    }
}

impl std::error::Error for InvalidSlashCommandName {}

/// Placeholder composer state. The editable buffer still lives in
/// `UiState::input`; this struct is the pane-local anchor for future composer
/// state that is not shared with legacy call sites.
#[derive(Debug, Clone, Default)]
pub struct ComposerState;

#[derive(Debug, Clone)]
pub enum ComposerPopupState {
    Slash(SlashPopupState),
    /// Unified `@` popup: agents + file paths + MCP resources in a
    /// single ranked list. Per-row kind lives on the suggestion item.
    At(AtPopupState),
    Symbol(SymbolPopupState),
}

#[derive(Debug, Clone)]
pub struct SlashPopupState;

#[derive(Debug, Clone)]
pub struct AtPopupState;

#[derive(Debug, Clone)]
pub struct SymbolPopupState;

impl ComposerPopupState {
    pub fn kind(&self) -> SuggestionKind {
        match self {
            Self::Slash(_) => SuggestionKind::SlashCommand,
            Self::At(_) => SuggestionKind::At,
            Self::Symbol(_) => SuggestionKind::Symbol,
        }
    }
}

// Single-instance UI state: `AppState` holds at most one active prompt at a
// time (never in a hot collection), so the per-variant size spread the
// `large_enum_variant` lint flags carries no real memory cost — boxing the
// payloads would only add indirection across 12+ construct/match sites.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum PanePromptState {
    Permission(PermissionPromptState),
    Question(QuestionPromptState),
    SandboxPermission(SandboxPermissionPromptState),
    CostWarning(CostWarningPromptState),
    PlanEntry(PlanEntryPromptState),
    PlanApproval(PlanApprovalPromptState),
    McpServerApproval(McpServerApprovalPromptState),
}

impl PanePromptState {
    pub fn priority(&self) -> i32 {
        match self {
            Self::SandboxPermission(_) => 0,
            Self::Permission(_) | Self::PlanEntry(_) => 1,
            Self::Question(_) | Self::McpServerApproval(_) | Self::PlanApproval(_) => 2,
            Self::CostWarning(_) => 3,
        }
    }

    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Permission(p) => Some(p.request_id.as_str()),
            Self::Question(q) => Some(q.request_id.as_str()),
            Self::SandboxPermission(s) => Some(s.request_id.as_str()),
            Self::CostWarning(_)
            | Self::PlanEntry(_)
            | Self::PlanApproval(_)
            | Self::McpServerApproval(_) => None,
        }
    }

    /// True for prompts that pause the status-indicator elapsed clock.
    ///
    /// Pauses when a tool-permission dialog is active ([`Self::Permission`]).
    ///
    /// DIVERGE: [`Self::SandboxPermission`] also pauses even though
    /// there is no analog variant upstream. The semantic is identical
    /// ("tool blocked waiting on user approval"), so widening the pause
    /// to cover sandbox approvals avoids a user-visible clock drift while
    /// we wait. Other prompts (Question, PlanEntry, PlanApproval,
    /// McpServerApproval, CostWarning) do not pause — the clock keeps
    /// running through them.
    pub fn pauses_status_clock(&self) -> bool {
        matches!(self, Self::Permission(_) | Self::SandboxPermission(_))
    }
}

#[cfg(test)]
#[path = "interaction.test.rs"]
mod tests;

/// State for the retained bottom interaction pane.
#[derive(Debug, Clone)]
pub struct InteractionPaneState {
    pub composer: ComposerState,
    pub popup: Option<ComposerPopupState>,
    pub active_prompt: Option<PanePromptState>,
    pub prompt_queue: VecDeque<PanePromptState>,
    pub delayed_permissions: VecDeque<DelayedPermissionPrompt>,
    /// Scroll offset (in rows) for the active permission prompt's scrollable
    /// command/detail body. Reset to 0 whenever the active prompt changes.
    /// Clamped to the real overflow at render time; the handler only bounds it
    /// loosely to prevent runaway.
    pub permission_scroll: u16,
}

impl InteractionPaneState {
    pub fn new() -> Self {
        Self {
            composer: ComposerState,
            popup: None,
            active_prompt: None,
            prompt_queue: VecDeque::new(),
            delayed_permissions: VecDeque::new(),
            permission_scroll: 0,
        }
    }

    pub fn push_prompt(&mut self, prompt: PanePromptState) {
        if self
            .active_prompt
            .as_ref()
            .and_then(PanePromptState::request_id)
            .zip(prompt.request_id())
            .is_some_and(|(active, incoming)| active == incoming)
        {
            self.active_prompt = Some(prompt);
            return;
        }
        // A different prompt is becoming active — start it un-scrolled.
        self.permission_scroll = 0;
        match self.active_prompt.take() {
            None => self.active_prompt = Some(prompt),
            Some(current) if prompt.priority() < current.priority() => {
                self.active_prompt = Some(prompt);
                self.enqueue_prompt(current);
            }
            Some(current) => {
                self.active_prompt = Some(current);
                self.enqueue_prompt(prompt);
            }
        }
    }

    pub fn push_permission(&mut self, prompt: PermissionPromptState, now: Instant) {
        let ready_at = now + PERMISSION_PROMPT_DELAY;
        self.delayed_permissions
            .push_back(DelayedPermissionPrompt { prompt, ready_at });
    }

    pub fn pop_ready_permission(&mut self, now: Instant) -> Option<PermissionPromptState> {
        if self
            .delayed_permissions
            .front()
            .is_some_and(|permission| permission.ready_at <= now)
        {
            return self
                .delayed_permissions
                .pop_front()
                .map(|permission| permission.prompt);
        }
        None
    }

    pub fn dismiss_active_prompt(&mut self) {
        self.active_prompt = self.prompt_queue.pop_front();
        self.permission_scroll = 0;
    }

    /// Move the permission-body scroll offset by `delta` rows, clamped to
    /// `[0, max]`. `max` is a loose upper bound (the render pass applies the
    /// exact clamp against the wrapped body height).
    pub fn scroll_permission(&mut self, delta: i32, max: u16) {
        let next = (i32::from(self.permission_scroll) + delta).clamp(0, i32::from(max));
        self.permission_scroll = next as u16;
    }

    pub fn active_prompt_mut(&mut self) -> Option<&mut PanePromptState> {
        self.active_prompt.as_mut()
    }

    fn enqueue_prompt(&mut self, prompt: PanePromptState) {
        let prio = prompt.priority();
        let pos = self
            .prompt_queue
            .iter()
            .position(|queued| queued.priority() > prio)
            .unwrap_or(self.prompt_queue.len());
        self.prompt_queue.insert(pos, prompt);
    }
}

impl Default for InteractionPaneState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct DelayedPermissionPrompt {
    pub prompt: PermissionPromptState,
    pub ready_at: Instant,
}
