//! Sandbox violation tracking.
//!
//! Stores sandbox violations in a bounded ring buffer for prompt injection
//! and UI display. Supports command correlation via tags and filtering
//! of benign violations.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::time::SystemTime;

use tokio::sync::mpsc;

use crate::config::IgnoreViolationsConfig;

/// Maximum number of violations stored in the ring buffer.
const DEFAULT_MAX_SIZE: i32 = 100;

/// A sandbox violation event.
#[derive(Debug, Clone)]
pub struct Violation {
    /// When the violation occurred.
    pub timestamp: SystemTime,
    /// The operation that was denied (e.g., "file-write-data", "network-outbound").
    pub operation: String,
    /// The path involved, if applicable.
    pub path: Option<String>,
    /// Correlation tag for the command that caused this violation.
    pub command_tag: Option<String>,
    /// Whether this is a benign/expected violation.
    pub benign: bool,
}

/// Benign violation operations that are always filtered.
const BENIGN_OPERATIONS: &[&str] = &[
    "mDNSResponder",
    "diagnosticd",
    "analyticsd",
    "com.apple.trustd",
];

impl Violation {
    /// Check if this violation matches a known benign pattern.
    pub fn is_benign_pattern(&self) -> bool {
        BENIGN_OPERATIONS
            .iter()
            .any(|pattern| self.operation.contains(pattern))
    }
}

/// Ring buffer store for sandbox violations.
///
/// Supports an optional observer channel that receives the delta count
/// of non-benign violations on each push. Used by the loop driver to
/// emit `SandboxViolationsDetected` for TUI flash messages.
pub struct ViolationStore {
    violations: VecDeque<Violation>,
    max_size: i32,
    total_count: i32,
    /// Optional observer that receives (new non-benign count) on each non-benign push.
    observer: Option<mpsc::UnboundedSender<i32>>,
    /// Per-command and global ignore patterns from config.
    ignore_patterns: IgnoreViolationsConfig,
}

impl ViolationStore {
    /// Create a new violation store with default capacity.
    pub fn new() -> Self {
        Self {
            violations: VecDeque::with_capacity(DEFAULT_MAX_SIZE as usize),
            max_size: DEFAULT_MAX_SIZE,
            total_count: 0,
            observer: None,
            ignore_patterns: HashMap::new(),
        }
    }

    /// Create a new violation store with a custom max size.
    pub fn with_max_size(max_size: i32) -> Self {
        Self {
            violations: VecDeque::with_capacity(max_size as usize),
            max_size,
            total_count: 0,
            observer: None,
            ignore_patterns: HashMap::new(),
        }
    }

    /// Create a violation store with an observer channel.
    ///
    /// The receiver gets the current non-benign count on each non-benign push.
    pub fn with_observer() -> (Self, mpsc::UnboundedReceiver<i32>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let store = Self {
            violations: VecDeque::with_capacity(DEFAULT_MAX_SIZE as usize),
            max_size: DEFAULT_MAX_SIZE,
            total_count: 0,
            observer: Some(tx),
            ignore_patterns: HashMap::new(),
        };
        (store, rx)
    }

    /// Set ignore patterns from config.
    ///
    /// Violations matching these patterns are silently dropped.
    /// Key `"*"` applies to all commands; other keys match the decoded
    /// command from the violation's tag.
    pub fn set_ignore_patterns(&mut self, patterns: IgnoreViolationsConfig) {
        self.ignore_patterns = patterns;
    }

    /// Add a violation, evicting the oldest if at capacity.
    ///
    /// Violations matching `ignore_patterns` are silently dropped.
    /// Notifies the observer with the updated non-benign count when
    /// a non-benign violation is added.
    pub fn push(&mut self, violation: Violation) {
        if self.should_ignore(&violation) {
            return;
        }

        let is_non_benign = !violation.benign;
        // Emit a structured log line on every non-benign violation. The
        // OTel layer in `coco-otel` subscribes to the `tracing`
        // dispatcher, so this becomes an observable event without
        // adding a direct dep on the otel crate (and without inverting
        // the dependency graph — `coco-otel` lives at L1).
        if is_non_benign {
            tracing::warn!(
                target: "coco_sandbox::violation",
                operation = %violation.operation,
                path = violation.path.as_deref().unwrap_or(""),
                command_tag = violation.command_tag.as_deref().unwrap_or(""),
                "sandbox violation"
            );
        }
        self.total_count += 1;
        if self.violations.len() as i32 >= self.max_size {
            self.violations.pop_front();
        }
        self.violations.push_back(violation);

        if is_non_benign && let Some(ref tx) = self.observer {
            let _ = tx.send(self.non_benign_count());
        }
    }

    /// Check if a violation should be ignored based on configured patterns.
    ///
    /// Global patterns (key `"*"`) apply to all violations.
    /// Command-specific patterns match the decoded command from the tag.
    fn should_ignore(&self, violation: &Violation) -> bool {
        if self.ignore_patterns.is_empty() {
            return false;
        }

        // Check global patterns (key = "*")
        if let Some(global_ops) = self.ignore_patterns.get("*")
            && global_ops
                .iter()
                .any(|pattern| violation.operation.contains(pattern.as_str()))
        {
            return true;
        }

        // Check command-specific patterns
        if let Some(ref tag) = violation.command_tag {
            for (cmd_pattern, ops) in &self.ignore_patterns {
                if cmd_pattern == "*" {
                    continue;
                }
                if tag.contains(cmd_pattern.as_str())
                    && ops
                        .iter()
                        .any(|op| violation.operation.contains(op.as_str()))
                {
                    return true;
                }
            }
        }

        false
    }

    /// Number of violations currently in the buffer.
    pub fn count(&self) -> i32 {
        self.violations.len() as i32
    }

    /// Total number of violations since session start (including evicted).
    pub fn total_count(&self) -> i32 {
        self.total_count
    }

    /// Count of non-benign violations in the buffer.
    pub fn non_benign_count(&self) -> i32 {
        self.violations.iter().filter(|v| !v.benign).count() as i32
    }

    /// Get the most recent `n` violations.
    pub fn recent(&self, n: i32) -> Vec<&Violation> {
        let n = n as usize;
        self.violations.iter().rev().take(n).collect()
    }

    /// Get violations for a specific command tag.
    pub fn for_command(&self, tag: &str) -> Vec<&Violation> {
        self.violations
            .iter()
            .filter(|v| v.command_tag.as_deref() == Some(tag))
            .collect()
    }

    /// Clear all violations.
    pub fn clear(&mut self) {
        self.violations.clear();
    }
}

impl std::fmt::Debug for ViolationStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ViolationStore")
            .field("count", &self.violations.len())
            .field("total_count", &self.total_count)
            .field("has_observer", &self.observer.is_some())
            .finish()
    }
}

impl Default for ViolationStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "violation.test.rs"]
mod tests;
