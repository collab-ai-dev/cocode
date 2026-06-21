/// Tracks permission denials so auto-mode can fall back to prompting when the
/// agent is stuck in a denial loop.
///
/// `shouldFallbackToPrompting` fires on `consecutiveDenials >= 3 ||
/// totalDenials >= 20`. There is no persistent "tripped" latch — the check
/// runs fresh after each recorded denial, an allowed action clears the
/// consecutive streak (`reset_consecutive`), and hitting the total cap resets
/// both counters (`reset_after_total_limit`).
///
/// Lives in `coco-tool-runtime` because it is per-`ToolUseContext` runtime
/// state (subagent-isolated when `local_denial_tracking` is set — every
/// subagent/fork gets a fresh one, TS `createSubagentContext` parity;
/// session-scoped otherwise for the main loop). `coco-permissions` re-exports
/// the type and operates on it from the auto-mode classifier path.
#[derive(Debug, Default)]
pub struct DenialTracker {
    pub consecutive_denials: i32,
    pub total_denials: i32,
    /// Per-tool denial counts for targeted suggestions.
    per_tool_denials: std::collections::HashMap<String, i32>,
}

/// Threshold for consecutive denials before falling back to prompting.
const CONSECUTIVE_DENIAL_THRESHOLD: i32 = 3;

/// Threshold for total denials before falling back to prompting.
const TOTAL_DENIAL_THRESHOLD: i32 = 20;

impl DenialTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a denial for a specific tool.
    pub fn record_denial(&mut self, tool_name: &str) {
        self.consecutive_denials += 1;
        self.total_denials += 1;
        *self
            .per_tool_denials
            .entry(tool_name.to_string())
            .or_default() += 1;
    }

    /// Record a denial without a tool name (backward compat).
    pub fn record_denial_anonymous(&mut self) {
        self.record_denial("unknown");
    }

    /// Reset consecutive counter (on successful tool execution).
    pub fn reset_consecutive(&mut self) {
        self.consecutive_denials = 0;
    }

    /// Whether auto-mode should stop classifying and fall back to prompting:
    /// 3 consecutive OR 20 total denials.
    pub fn should_fallback_to_prompting(&self) -> bool {
        self.consecutive_denials >= CONSECUTIVE_DENIAL_THRESHOLD
            || self.total_denials >= TOTAL_DENIAL_THRESHOLD
    }

    /// Whether the fallback was triggered by the *total* cap (vs the
    /// consecutive one) — drives the warning wording and the counter reset.
    pub fn hit_total_limit(&self) -> bool {
        self.total_denials >= TOTAL_DENIAL_THRESHOLD
    }

    /// Reset both counters after the total cap is hit so the session can
    /// continue past a single review prompt instead of denying forever.
    pub fn reset_after_total_limit(&mut self) {
        self.consecutive_denials = 0;
        self.total_denials = 0;
    }

    /// Clear all denial state (consecutive + total + per-tool counts).
    /// Called by the post-compact observer because the conversational
    /// context that drove the denials is now archived.
    pub fn clear(&mut self) {
        self.consecutive_denials = 0;
        self.total_denials = 0;
        self.per_tool_denials.clear();
    }

    /// Whether the agent appears stuck in a denial loop.
    pub fn is_stuck(&self) -> bool {
        self.consecutive_denials >= CONSECUTIVE_DENIAL_THRESHOLD
    }

    /// Whether total denials suggest the user should adjust permissions.
    pub fn should_suggest_permissions(&self) -> bool {
        self.total_denials >= TOTAL_DENIAL_THRESHOLD
    }

    /// Get the most frequently denied tool (for targeted suggestions).
    pub fn most_denied_tool(&self) -> Option<(&str, i32)> {
        self.per_tool_denials
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(name, count)| (name.as_str(), *count))
    }

    /// Get denial count for a specific tool.
    pub fn tool_denial_count(&self, tool_name: &str) -> i32 {
        self.per_tool_denials.get(tool_name).copied().unwrap_or(0)
    }

    /// Build a suggestion message based on denial state.
    pub fn suggestion_message(&self) -> Option<String> {
        if self.should_suggest_permissions() {
            Some(format!(
                "You've had {total} permission denials this session. \
                 Consider running /permissions to adjust your settings.",
                total = self.total_denials
            ))
        } else if self.is_stuck() {
            let tool_hint = self
                .most_denied_tool()
                .map(|(name, _)| format!(" (most denied: {name})"))
                .unwrap_or_default();
            Some(format!(
                "{consecutive} consecutive permission denials{tool_hint}. \
                 The agent may be stuck. Falling back to interactive prompting.",
                consecutive = self.consecutive_denials
            ))
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "denial_tracking.test.rs"]
mod tests;
