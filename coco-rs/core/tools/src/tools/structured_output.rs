//! `StructuredOutput` synthetic tool.
//!
//! Captures the model's final response as schema-validated JSON for SDK
//! consumers.
//!
//! - Wire name is `"StructuredOutput"`.
//! - The tool's `input_schema()` **is** the user-supplied JSON schema —
//!   the model sees it via the standard tool-calling protocol and the
//!   provider's strict-tool-input validation does the first-line
//!   enforcement.
//! - `execute()` runs a second-line client-side check via the
//!   [`jsonschema`] crate; on success it forwards the input through
//!   [`ToolResult::with_structured_output`], which pushes a silent
//!   `StructuredOutput` attachment onto `new_messages` for the engine's
//!   side-channel pipeline.
//! - Tool is **never** registered by [`crate::register_all_tools`].
//!   Callers explicitly opt in via
//!   [`crate::register_structured_output_tool`] from non-interactive
//!   bootstrap paths (headless print mode, SDK NDJSON) after a
//!   `--json-schema` value has been parsed.
//!
//! Uses the [`jsonschema`] crate for client-side validation against
//! Draft 7 / 2020-12 dialect range.

use async_trait::async_trait;
use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_types::ToolId;
use coco_types::ToolName;
use serde_json::Value;

/// User-facing prompt body.
const TOOL_PROMPT: &str = "Use this tool to return your final response in the requested structured format. You MUST call this tool exactly once at the end of your response to provide the structured output.";

/// User-facing description.
const TOOL_DESCRIPTION: &str = "Return structured output in the requested format";

/// Compiled-validator-backed structured-output tool.
///
/// Stateless after construction; clone-cheap (the validator is shared
/// behind `Arc` inside the schema). `Debug` elides the compiled validator.
#[derive(Debug)]
pub struct StructuredOutputTool {
    /// Self-validating schema (v4.2) — owns the user document (model-facing)
    /// and the compiled validator. Replaces the old `schema: Value` +
    /// `validator: Arc<Validator>` pair.
    schema: coco_tool_runtime::ToolInputSchema,
}

impl StructuredOutputTool {
    /// Compile a `StructuredOutputTool` from a JSON Schema value.
    ///
    /// Returns an error string when the supplied schema fails its own
    /// meta-validation (invalid keywords, unsupported `$ref`, etc.).
    pub fn new(schema: Value) -> Result<Self, String> {
        let schema = coco_tool_runtime::ToolInputSchema::from_value(schema)
            .map_err(|e| format!("invalid JSON schema: {e}"))?;
        Ok(Self { schema })
    }

    /// The user-supplied JSON Schema — exposed for telemetry / `init`
    /// echo without cloning the validator.
    pub fn schema(&self) -> &Value {
        self.schema.as_value()
    }
}

#[async_trait]
impl Tool for StructuredOutputTool {
    /// Untyped input — the schema is user-supplied at runtime, not a
    /// fixed Rust type.
    type Input = Value;
    /// String-typed output — `render_for_model` emits the canonical
    /// success message verbatim.
    type Output = String;

    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        &self.schema
    }

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::StructuredOutput)
    }

    fn name(&self) -> &str {
        ToolName::StructuredOutput.as_str()
    }

    fn description(&self, _input: &Value, _options: &DescriptionOptions) -> String {
        TOOL_DESCRIPTION.into()
    }

    async fn prompt(&self, _options: &coco_tool_runtime::PromptOptions) -> String {
        TOOL_PROMPT.into()
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    /// Plan-mode safe — pure I/O of model-produced JSON.
    fn is_always_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("return the final response as structured JSON")
    }

    fn render_for_model(&self, out: &String) -> Vec<ToolResultContentPart> {
        vec![ToolResultContentPart::Text {
            text: out.clone(),
            provider_options: None,
        }]
    }

    async fn execute(
        &self,
        input: Value,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<String>, ToolError> {
        if let Err(issues) = self.schema.validate(&input) {
            let detail = issues
                .iter()
                .map(|i| format!("{i:?}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(ToolError::ExecutionFailed {
                message: format!("Output does not match required schema: {detail}"),
                display_data: None,
                source: None,
            });
        }
        Ok(
            ToolResult::data("Structured output provided successfully".to_string())
                .with_structured_output(input),
        )
    }
}

#[cfg(test)]
#[path = "structured_output.test.rs"]
mod tests;
