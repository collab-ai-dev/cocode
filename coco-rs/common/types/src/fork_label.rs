//! Typed discriminator for framework-spawned, cache-shared,
//! fire-and-forget side-channel queries.
//!
//! Lives in `coco-types` (zero-dep) so logs, telemetry, transcripts,
//! and structured tracing all share one canonical name set without
//! `String` proliferation. Wire form is snake_case so it round-trips
//! cleanly through settings.json + transcript JSON.
//!
//! ## Variants
//!
//! | Variant | canUseTool policy |
//! |---|---|
//! | `PromptSuggestion` | deny-all |
//! | `SideQuestion` | deny-all |
//! | `Compact` | deny-all |
//! | `ExtractMemories` | auto-mem (Read/Glob/Grep + read-only Bash + Edit/Write within memory_dir) |
//! | `SessionMemoryAuto` | session-mem (Edit only on exact path, Read) |
//! | `SessionMemoryManual` | session-mem |
//! | `AgentSummary` | deny-all (30s timer fork) |
//! | `AutoDream` | auto-mem with broader memory_root |
//! | `Speculation` | 3-boundary (Edit/Write rewrites to overlay; Bash via shell-parser read-only check; deny default) |
//! | `HookAgent` | scoped StructuredOutput verifier |
//!
//! Order is deliberate: PromptSuggestion / SideQuestion / Compact are
//! the simplest deny-all callers; the memory family follows; finally
//! AgentSummary, AutoDream, Speculation. New forks should be inserted
//! in their thematic group.

use serde::Deserialize;
use serde::Serialize;

/// Typed discriminator for one of the framework-spawned, cache-shared,
/// fire-and-forget forks. See module docs for the per-variant canUseTool policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkLabel {
    /// Post-turn "what should the user type next" predictor. Renders
    /// behind the user's cursor in the TUI prompt input.
    PromptSuggestion,
    /// `/btw <question>` slash-command — answer one question without
    /// polluting main transcript.
    SideQuestion,
    /// `/compact` summarizer — fork produces the new summary message.
    Compact,
    /// Post-turn extraction of facts → MEMORY.md / CLAUDE.md.
    ExtractMemories,
    /// Auto-trigger 9-section session-memory rebuild.
    SessionMemoryAuto,
    /// `/summary` slash-command — manual session-memory rebuild.
    SessionMemoryManual,
    /// 30-second progress snapshot of a running Agent-tool subagent
    /// (coordinator-mode feature).
    AgentSummary,
    /// KAIROS-mode long-term memory consolidation (timer-driven).
    AutoDream,
    /// Pre-execute the next prompt-suggestion in a COW overlay
    /// sandbox so accept can instant-inject the result. Gated by
    /// `Feature::Speculation`.
    Speculation,
    /// Agent-type hook verifier. Runs as an isolated child query with
    /// a scoped StructuredOutput tool/enforcement hook.
    HookAgent,
}

impl ForkLabel {
    /// Stable snake_case wire string. Use this for logs / telemetry /
    /// transcript tags so `query_source` attribution survives
    /// cross-runtime joins.
    pub fn as_str(self) -> &'static str {
        match self {
            ForkLabel::PromptSuggestion => "prompt_suggestion",
            ForkLabel::SideQuestion => "side_question",
            ForkLabel::Compact => "compact",
            ForkLabel::ExtractMemories => "extract_memories",
            ForkLabel::SessionMemoryAuto => "session_memory_auto",
            ForkLabel::SessionMemoryManual => "session_memory_manual",
            ForkLabel::AgentSummary => "agent_summary",
            ForkLabel::AutoDream => "auto_dream",
            ForkLabel::Speculation => "speculation",
            ForkLabel::HookAgent => "hook_agent",
        }
    }
}

impl std::fmt::Display for ForkLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
#[path = "fork_label.test.rs"]
mod tests;
