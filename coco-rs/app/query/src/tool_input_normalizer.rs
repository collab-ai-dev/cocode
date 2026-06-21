use coco_types::ToolName;
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ToolInputNormalizationContext<'a> {
    /// Current working directory string, used by the Bash branch to
    /// strip a model-emitted `cd $cwd && ` prefix. `None` skips that
    /// rewrite (e.g. test paths or environments where cwd isn't
    /// known).
    pub cwd: Option<&'a str>,
}

/// Normalize assistant-emitted tool input before it reaches transcript,
/// stream events, hooks, permission, and execution.
///
/// Per-tool branches:
///
/// - `ExitPlanMode` — strip stale internal fields from old transcripts or
///   clients. The plan snapshot is carried in typed permission detail instead
///   of observable tool input.
/// - `Bash` — strip a leading `cd $cwd && ` prefix and rewrite `\\;`
///   into `\;` (find-exec quoting fix).
/// - `TaskOutput` — alias-map `agentId` / `bash_id` → `task_id` and
///   `wait_up_to` (seconds) → `timeout` (ms).
///
/// FileEdit normalization is intentionally **not** ported here: the TS
/// branch reads the target file from disk to fuzz-match `old_string`
/// after whitespace normalization. That logic belongs in the
/// Edit/MultiEdit tools themselves (where file I/O is already gated by
/// permission). Adding it here would couple every wire-parsing → schema-validation
/// hop to filesystem reads.
pub(crate) fn normalize_observable_tool_input(
    tool_name: &str,
    input: Value,
    ctx: ToolInputNormalizationContext<'_>,
) -> Value {
    if tool_name == ToolName::ExitPlanMode.as_str() {
        return normalize_exit_plan_mode(input, ctx);
    }
    if tool_name == ToolName::Bash.as_str() {
        return normalize_bash(input, ctx);
    }
    if tool_name == ToolName::TaskOutput.as_str() {
        return normalize_task_output(input);
    }
    input
}

fn normalize_exit_plan_mode(input: Value, _ctx: ToolInputNormalizationContext<'_>) -> Value {
    let mut object = match input {
        Value::Object(map) => map,
        other => return other,
    };
    object.remove(coco_messages::EXIT_PLAN_MODE_INJECTED_PLAN_FIELD);
    object.remove(coco_messages::EXIT_PLAN_MODE_INJECTED_PLAN_FILE_PATH_FIELD);
    object.remove("user_choice");
    object.remove("outcome");
    Value::Object(object)
}

fn normalize_bash(input: Value, ctx: ToolInputNormalizationContext<'_>) -> Value {
    let mut object = match input {
        Value::Object(map) => map,
        other => return other,
    };
    let Some(Value::String(command)) = object.get_mut("command") else {
        return Value::Object(object);
    };

    // Strip a leading `cd $cwd && ` so transcript + hooks see the
    // semantic command. The model often prepends this to set
    // execution directory; downstream tools already track cwd.
    if let Some(cwd) = ctx.cwd {
        let prefix = format!("cd {cwd} && ");
        if let Some(stripped) = command.strip_prefix(&prefix) {
            *command = stripped.to_string();
        }
    }

    // Replace `\\;` → `\;`. Models often double-escape the
    // find-exec terminator when emitting JSON; the desired wire
    // shell form is single-backslash.
    if command.contains("\\\\;") {
        *command = command.replace("\\\\;", "\\;");
    }

    Value::Object(object)
}

fn normalize_task_output(input: Value) -> Value {
    let mut object = match input {
        Value::Object(map) => map,
        other => return other,
    };

    // `task_id` already wins if present; otherwise fall back to legacy
    // aliases the model may emit (`agentId` from the V1 AgentOutput,
    // `bash_id` from the V1 BashOutput).
    if !object.contains_key("task_id")
        && let Some(legacy) = object
            .remove("agentId")
            .or_else(|| object.remove("bash_id"))
    {
        object.insert("task_id".to_string(), legacy);
    }

    // `wait_up_to` (seconds) → `timeout` (ms). Only fires when the
    // canonical `timeout` is absent.
    if !object.contains_key("timeout")
        && let Some(Value::Number(n)) = object.remove("wait_up_to")
        && let Some(seconds) = n.as_f64()
    {
        let millis = (seconds * 1000.0).round() as i64;
        object.insert(
            "timeout".to_string(),
            Value::Number(serde_json::Number::from(millis)),
        );
    }

    Value::Object(object)
}

#[cfg(test)]
#[path = "tool_input_normalizer.test.rs"]
mod tests;
