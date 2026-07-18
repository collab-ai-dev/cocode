//! Construction-time execution profile for a session runtime.
//!
//! A profile is chosen once when the runtime is built and never changes. It
//! decides which durable/background capabilities are installed and whether the
//! model tool boundary and hook set are restricted. `Primary` is the ordinary
//! full-capability session; `SideChatReadOnly` is the ephemeral, read-only
//! sidechat child (see `docs/internal/sidechat-architecture.md`).
//!
//! The profile is threaded through runtime construction and late-bind
//! installation. The predicate methods here are the single decision table those
//! installers read — no construction site re-derives "is this a sidechat".

pub use coco_hooks::HookExecutionPolicy;

/// How a session runtime is constructed and what it is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionExecutionProfile {
    /// Ordinary full-capability conversation.
    Primary,
    /// Ephemeral, read-only sidechat child: no durable or background ownership,
    /// a structural read-only tool boundary, and tool-lifecycle hooks only.
    SideChatReadOnly,
}

impl SessionExecutionProfile {
    /// Transcript / usage / file-history persistence.
    pub fn persists_history(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// AppServer `SessionManager` registration (installed at the bridge layer).
    pub fn registers_session_manager(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// OS process PID registration in the concurrent-sessions registry.
    pub fn registers_pid(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// Goal store persistence and the background goal driver.
    pub fn runs_goals(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// Auto-memory extraction and auto-dream.
    pub fn runs_auto_memory(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// Skill learning / review.
    pub fn runs_skill_learning(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// Post-turn prompt-suggestion / speculation fork dispatch.
    pub fn runs_prompt_suggestion(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// Scheduled-task store and background task spawning.
    pub fn runs_scheduled_tasks(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// Auto title generation and persistence.
    pub fn runs_auto_title(self) -> bool {
        matches!(self, Self::Primary)
    }

    /// True when the model tool boundary is restricted to structural reads.
    pub fn read_only_tools(self) -> bool {
        matches!(self, Self::SideChatReadOnly)
    }

    /// Which hook families this profile permits.
    pub fn hook_policy(self) -> HookExecutionPolicy {
        match self {
            Self::Primary => HookExecutionPolicy::All,
            Self::SideChatReadOnly => HookExecutionPolicy::ToolLifecycleOnly,
        }
    }
}

#[cfg(test)]
#[path = "execution_profile.test.rs"]
mod tests;
