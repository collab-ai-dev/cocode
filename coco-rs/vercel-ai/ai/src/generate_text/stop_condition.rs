//! Stop conditions for multi-step generation.
//!
//! This module provides utilities for defining stop conditions
//! that determine when multi-step generation should end.

use super::step_result::StepResult;

/// A function that determines if generation should stop.
///
/// Takes a slice reference to avoid cloning the entire steps vector.
pub type StopConditionFn = Box<dyn Fn(&[StepResult]) -> bool + Send + Sync>;

/// Stop condition for multi-step generation.
pub struct StopCondition {
    /// The condition function.
    condition: StopConditionFn,
    /// Description of the condition.
    description: String,
}

impl StopCondition {
    /// Create a new stop condition.
    pub fn new<F>(description: impl Into<String>, condition: F) -> Self
    where
        F: Fn(&[StepResult]) -> bool + Send + Sync + 'static,
    {
        Self {
            condition: Box::new(condition),
            description: description.into(),
        }
    }

    /// Check if this condition is met.
    pub fn is_met(&self, steps: &[StepResult]) -> bool {
        (self.condition)(steps)
    }

    /// Get the description.
    pub fn description(&self) -> &str {
        &self.description
    }
}

/// Create a stop condition that triggers when the number of completed steps equals `step_count`.
pub fn is_step_count(step_count: usize) -> StopCondition {
    StopCondition::new(format!("Stop after {step_count} steps"), move |steps| {
        steps.len() == step_count
    })
}

/// Alias for [`is_step_count`] (previous name).
pub fn step_count_is(step_count: usize) -> StopCondition {
    is_step_count(step_count)
}

/// Create a stop condition that never stops. Lets the loop run until a natural termination.
pub fn is_loop_finished() -> StopCondition {
    StopCondition::new("Never stop (loop finished)", |_steps| false)
}

/// Create a stop condition that triggers when a specific tool is called.
pub fn has_tool_call(tool_name: impl Into<String>) -> StopCondition {
    let tool_name = tool_name.into();
    StopCondition::new(
        format!("Stop when tool '{tool_name}' is called"),
        move |steps| {
            steps
                .last()
                .is_some_and(|step| step.tool_calls.iter().any(|tc| tc.tool_name == tool_name))
        },
    )
}

/// Create a stop condition that triggers when the response contains specific text.
pub fn response_contains(text: impl Into<String>) -> StopCondition {
    let text = text.into();
    StopCondition::new(
        format!("Stop when response contains '{text}'"),
        move |steps| steps.last().is_some_and(|step| step.text.contains(&*text)),
    )
}

/// Check if any stop condition is met.
pub fn is_stop_condition_met(stop_conditions: &[StopCondition], steps: &[StepResult]) -> bool {
    stop_conditions
        .iter()
        .any(|condition| condition.is_met(steps))
}

#[cfg(test)]
#[path = "stop_condition.test.rs"]
mod tests;
