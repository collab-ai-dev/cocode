use coco_types::HookEventType;

/// Immutable allowlist applied at every hook dispatch boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookExecutionPolicy {
    /// Every configured hook family may run.
    All,
    /// Only tool lifecycle hooks may run. Unknown future hook families are
    /// denied by default.
    ToolLifecycleOnly,
}

impl HookExecutionPolicy {
    pub const fn allows(self, event: HookEventType) -> bool {
        match self {
            Self::All => true,
            Self::ToolLifecycleOnly => matches!(
                event,
                HookEventType::PreToolUse
                    | HookEventType::PostToolUse
                    | HookEventType::PostToolUseFailure
            ),
        }
    }
}
