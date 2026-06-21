//! AskUserQuestionTool — structured multi-choice questions to the user.
//!
//! The tool returns the questions payload as its result; the TUI/CLI layer
//! intercepts, presents the interactive overlay (see
//! `app/tui/src/render_overlays/question.rs`), collects answers, and fills
//! them in before the result goes back to the model.

use coco_messages::ToolResult;
use coco_tool_runtime::DescriptionOptions;
use coco_tool_runtime::PromptOptions;
use coco_tool_runtime::Tool;
use coco_tool_runtime::ToolError;
use coco_tool_runtime::ToolResultContentPart;
use coco_tool_runtime::ToolUseContext;
use coco_tool_runtime::ValidationResult;
use coco_types::AskUserQuestionAnswered;
use coco_types::AskUserQuestionResult;
use coco_types::ToolCheckResult;
use coco_types::ToolDisplayData;
use coco_types::ToolId;
use coco_types::ToolName;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;

/// Typed input for [`AskUserQuestionTool`].
///
/// All four fields stay as opaque `Value` because:
///   - `questions` is a heterogeneous union (`multiSelect` toggles
///     option shape) that the model freely populates; the rich
///     constraints are encoded in [`AskUserQuestionTool::input_schema`].
///   - `answers` / `annotations` are TUI-spliced via
///     `PermissionOutcome::Allow.updated_input` *after* the model emits
///     the call — keeping them as `Value` matches that pipeline.
///   - `metadata` is an analytics passthrough, intentionally lax.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct AskUserQuestionInput {
    /// Questions array — model-supplied. See `input_schema()` for the
    /// full constraint set (1-4 questions, each with 2-4 options,
    /// optional `multiSelect`, optional `preview` per option).
    pub questions: Value,
    /// (Internal) User answers, spliced into the input by the
    /// TUI/CLI host before `tool_call_preparer` re-validates.
    #[serde(default)]
    pub answers: Option<Value>,
    /// (Internal) Per-question annotations (preview / notes).
    #[serde(default)]
    pub annotations: Option<Value>,
    /// Optional analytics metadata.
    #[serde(default)]
    pub metadata: Option<Value>,
}

/// Short description shown in tool-catalog listings.
const ASK_USER_QUESTION_DESCRIPTION: &str = "Asks the user multiple choice questions to gather information, \
     clarify ambiguity, understand preferences, make decisions or offer \
     them choices.";

/// Full system-prompt contribution for the tool.
///
/// `ExitPlanMode` is interpolated via `ToolName::ExitPlanMode.as_str()`
/// — follows the "no hardcoded strings for closed sets" rule.
static ASK_USER_QUESTION_PROMPT: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let exit = coco_types::ToolName::ExitPlanMode.as_str();
    format!(
        "\
Use this tool when you need to ask the user questions during execution. This allows you to:
1. Gather user preferences or requirements
2. Clarify ambiguous instructions
3. Get decisions on implementation choices as you work
4. Offer choices to the user about what direction to take.

Usage notes:
- Users will always be able to select \"Other\" to provide custom text input
- Use multiSelect: true to allow multiple answers to be selected for a question
- If you recommend a specific option, make that the first option in the list and add \"(Recommended)\" at the end of the label

Plan mode note: In plan mode, use this tool to clarify requirements or choose between approaches BEFORE finalizing your plan. Do NOT use this tool to ask \"Is my plan ready?\" or \"Should I proceed?\" - use {exit} for plan approval. IMPORTANT: Do not reference \"the plan\" in your questions (e.g., \"Do you have feedback about the plan?\", \"Does the plan look good?\") because the user cannot see the plan in the UI until you call {exit}. If you need plan approval, use {exit} instead.

Preview feature:
Use the optional `preview` field on options when presenting concrete artifacts that users need to visually compare:
- ASCII mockups of UI layouts or components
- Code snippets showing different implementations
- Diagram variations
- Configuration examples

Preview content is rendered as markdown in a monospace box. Multi-line text with newlines is supported. When any option has a preview, the UI switches to a side-by-side layout with a vertical option list on the left and preview on the right. Do not use previews for simple preference questions where labels and descriptions suffice. Note: previews are only supported for single-select questions (not multiSelect).
"
    )
});

/// Max display width of a question's `header` chip — mirrored in the schema
/// description below and the TUI renderer's chip truncation
/// (`app/tui::presentation::request::chip`).
const ASK_USER_QUESTION_CHIP_WIDTH: usize = 12;

pub struct AskUserQuestionTool;

#[async_trait::async_trait]
impl Tool for AskUserQuestionTool {
    type Input = AskUserQuestionInput;
    // Static schema from a literal `json!`; a parse failure means the literal
    // is malformed (a programmer error), so panicking on first build is correct.
    #[allow(clippy::expect_used)]
    fn runtime_validation_schema(&self) -> &coco_tool_runtime::ToolInputSchema {
        static SCHEMA: std::sync::OnceLock<coco_tool_runtime::ToolInputSchema> =
            std::sync::OnceLock::new();
        SCHEMA.get_or_init(|| {
            coco_tool_runtime::ToolInputSchema::from_static_value(serde_json::json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "Questions to ask the user (1-4 questions)",
                        "minItems": 1,
                        "maxItems": 4,
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "The complete question to ask the user. Should be clear, specific, and end with a question mark. Example: \"Which library should we use for date formatting?\" If multiSelect is true, phrase it accordingly, e.g. \"Which features do you want to enable?\""
                                },
                                "header": {
                                    "type": "string",
                                    "description": format!("Very short label displayed as a chip/tag (max {ASK_USER_QUESTION_CHIP_WIDTH} chars). Examples: \"Auth method\", \"Library\", \"Approach\".")
                                },
                                "options": {
                                    "type": "array",
                                    "description": "The available choices for this question. Must have 2-4 options. Each option should be a distinct, mutually exclusive choice (unless multiSelect is enabled). There should be no 'Other' option, that will be provided automatically.",
                                    "minItems": 2,
                                    "maxItems": 4,
                                    "items": {
                                        "type": "object",
                                        "additionalProperties": false,
                                        "properties": {
                                            "label": {
                                                "type": "string",
                                                "description": "The display text for this option that the user will see and select. Should be concise (1-5 words) and clearly describe the choice."
                                            },
                                            "description": {
                                                "type": "string",
                                                "description": "Explanation of what this option means or what will happen if chosen. Useful for providing context about trade-offs or implications."
                                            },
                                            "preview": {
                                                "type": "string",
                                                "description": "Optional preview content rendered when this option is focused. Use for mockups, code snippets, or visual comparisons that help users compare options. See the tool description for the expected content format."
                                            }
                                        },
                                        "required": ["label", "description"]
                                    }
                                },
                                "multiSelect": {
                                    "type": "boolean",
                                    "description": "Set to true to allow the user to select multiple options instead of just one. Use when choices are not mutually exclusive.",
                                    "default": false
                                }
                            },
                            "required": ["question", "header", "options"]
                        }
                    },
                    // `answers` and `annotations` are optional fields the TUI/CLI
                    // layer splices into the tool input via
                    // `PermissionOutcome::Allow.updated_input` BEFORE `tool_call_preparer`
                    // re-validates the rewritten input. Declaring them here keeps
                    // schema validation green; the model itself is not expected to
                    // populate these — the prompt teaches it to emit `questions` only.
                    "answers": {
                        "type": "object",
                        "description": "(Internal) User-supplied answers, spliced by the host before invocation. Map of question text → selected option label.",
                        "additionalProperties": { "type": "string" }
                    },
                    "annotations": {
                        "type": "object",
                        "description": "(Internal) Per-question annotations (preview / notes), spliced by the host before invocation.",
                        "additionalProperties": { "type": "object" }
                    },
                    // Optional analytics-tracking blob the model may emit alongside
                    // the question (e.g. `{source: "remember"}` for the /remember
                    // command). Echoed straight through to logs; never user-visible.
                    "metadata": {
                        "type": "object",
                        "description": "Optional metadata for tracking and analytics purposes. Not displayed to user.",
                        "properties": {
                            "source": {
                                "type": "string",
                                "description": "Optional identifier for the source of this question (e.g., \"remember\" for /remember command). Used for analytics tracking."
                            }
                        }
                    }
                },
                "required": ["questions"]
            }))
        })
    }
    /// Output stays on `Value`: `render_for_model` walks the
    /// `answers` / `annotations` maps generically, and the
    /// envelope is downstream-consumed (TUI overlay) so a typed
    /// envelope would just force redundant round-trips.
    type Output = serde_json::Value;

    fn id(&self) -> ToolId {
        ToolId::Builtin(ToolName::AskUserQuestion)
    }
    fn name(&self) -> &str {
        ToolName::AskUserQuestion.as_str()
    }
    fn search_hint(&self) -> Option<&str> {
        Some("prompt the user with a multiple-choice question")
    }
    fn description(&self, _input: &AskUserQuestionInput, _options: &DescriptionOptions) -> String {
        ASK_USER_QUESTION_DESCRIPTION.into()
    }
    async fn prompt(&self, _options: &PromptOptions) -> String {
        ASK_USER_QUESTION_PROMPT.clone()
    }
    fn requires_user_interaction(&self) -> bool {
        true
    }

    /// Multiple questions issued in the same turn are presented together by
    /// the TUI, so the executor can batch them concurrently rather than
    /// serializing.
    fn is_concurrency_safe(&self, _input: &AskUserQuestionInput) -> bool {
        true
    }

    fn validate_input(
        &self,
        input: &AskUserQuestionInput,
        _ctx: &ToolUseContext,
    ) -> ValidationResult {
        let Some(questions) = input.questions.as_array() else {
            return ValidationResult::invalid("questions must be an array");
        };

        let mut seen_questions = HashSet::new();
        for question in questions {
            let question_text = question
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !seen_questions.insert(question_text) {
                return ValidationResult::invalid(format!(
                    "question text must be unique: {question_text}"
                ));
            }

            let Some(options) = question.get("options").and_then(Value::as_array) else {
                continue;
            };
            let mut seen_labels = HashSet::new();
            for option in options {
                let label = option
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !seen_labels.insert(label) {
                    return ValidationResult::invalid(format!(
                        "option labels must be unique for question '{question_text}': {label}"
                    ));
                }
            }
        }

        ValidationResult::Valid
    }

    /// Render the user's answers as prose. The TUI/CLI layer splices
    /// answers (and optional `annotations` for preview / notes) into
    /// the tool result before the model sees it; this fn
    /// reads that envelope and produces:
    ///
    ///   `User has answered your questions: "Q1"="A1" selected
    ///   preview:\n... user notes: ..., "Q2"="A2". You can now
    ///   continue with the user's answers in mind.`
    ///
    /// When the splicer hasn't run (test fixtures, dialog declined),
    /// fall back to the defensive JSON-or-string emit.
    fn render_for_model(&self, data: &Value) -> Vec<ToolResultContentPart> {
        let answers = data
            .get("answers")
            .and_then(Value::as_object)
            .filter(|m| !m.is_empty());
        let Some(answers) = answers else {
            return coco_tool_runtime::render_text_or_json(data);
        };
        let annotations = data.get("annotations").and_then(Value::as_object);
        let mut entries: Vec<String> = Vec::with_capacity(answers.len());
        for (question, answer_v) in answers {
            let answer = answer_v.as_str().unwrap_or("");
            let mut parts = vec![format!("\"{question}\"=\"{answer}\"")];
            if let Some(annotation) = annotations.and_then(|a| a.get(question)) {
                if let Some(preview) = annotation.get("preview").and_then(Value::as_str) {
                    parts.push(format!("selected preview:\n{preview}"));
                }
                if let Some(notes) = annotation.get("notes").and_then(Value::as_str) {
                    parts.push(format!("user notes: {notes}"));
                }
            }
            entries.push(parts.join(" "));
        }
        let answers_text = entries.join(", ");
        let text = format!(
            "User has answered your questions: {answers_text}. You can now continue with the user's answers in mind."
        );
        vec![ToolResultContentPart::Text {
            text,
            provider_options: None,
        }]
    }

    /// Always require the user to answer — this is the seam that turns the tool
    /// call into the interactive Question overlay.
    ///
    /// The tool is read-only (no side effects), so without this override the
    /// evaluator auto-allows it and `execute()` just echoes the questions as a
    /// raw JSON tool result. Returning `Ask` (at evaluator step 1c, before the
    /// read-only mode fallthrough) routes through the permission bridge, which
    /// emits `QuestionAsked` and pushes the Question overlay; the answer
    /// round-trip (`updated_input` → `execute`) is already wired. `DontAsk`
    /// mode causes the evaluator to convert this to
    /// `Deny` (never prompt), which is the intended posture.
    async fn check_permissions(
        &self,
        _input: &AskUserQuestionInput,
        _ctx: &ToolUseContext,
    ) -> ToolCheckResult {
        ToolCheckResult::Ask {
            message: "Answer questions?".to_string(),
            suggestions: Vec::new(),
            choices: None,
            detail: None,
        }
    }

    async fn execute(
        &self,
        input: AskUserQuestionInput,
        _ctx: &ToolUseContext,
    ) -> Result<ToolResult<Value>, ToolError> {
        let questions = if input.questions.is_null() {
            Value::Array(vec![])
        } else {
            input.questions
        };

        // Propagate `answers` and `annotations` through to the result
        // when the host (TUI/CLI/test harness) has already spliced
        // them into the tool input via `PermissionOutcome::Allow.updated_input`.
        // `render_for_model` reads them off `data` and produces the
        // user-answered prose; without this propagation the splice
        // would be invisible to the renderer.
        let mut data = serde_json::Map::new();
        data.insert("questions".into(), questions);
        if let Some(answers) = input.answers {
            data.insert("answers".into(), answers);
        }
        if let Some(annotations) = input.annotations {
            data.insert("annotations".into(), annotations);
        }
        // Structured answers for the styled transcript cell (the model still
        // sees the prose via `render_for_model`). `None` when no answers were
        // spliced (declined / test fixtures) — the renderer then falls back.
        let display_data = ask_user_question_display(&data);
        Ok(ToolResult {
            data: Value::Object(data),
            new_messages: vec![],
            app_state_patch: None,
            permission_updates: Vec::new(),
            display_data,
        })
    }
}

/// Build the structured [`ToolDisplayData::AskUserQuestionResult`] for the
/// styled transcript cell from the spliced `answers`/`annotations` envelope.
/// Question order follows the model's `questions` array (the answers map order
/// is not guaranteed). Returns `None` when no answers were spliced — the
/// renderer then falls back to the prose.
fn ask_user_question_display(data: &serde_json::Map<String, Value>) -> Option<ToolDisplayData> {
    let answers = data
        .get("answers")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())?;
    let annotations = data.get("annotations").and_then(Value::as_object);
    // Preserve the model's question order; fall back to the answers map order.
    let ordered: Vec<String> = match data.get("questions").and_then(Value::as_array) {
        Some(qs) => qs
            .iter()
            .filter_map(|q| q.get("question").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        None => answers.keys().cloned().collect(),
    };
    let questions = ordered
        .into_iter()
        .map(|question| {
            let answer = answers
                .get(&question)
                .and_then(Value::as_str)
                .unwrap_or_default();
            // `build_answer_payload` joins multi-select labels with ", ".
            let answers = if answer.is_empty() {
                Vec::new()
            } else {
                answer.split(", ").map(str::to_string).collect()
            };
            let note = annotations
                .and_then(|a| a.get(&question))
                .and_then(|entry| entry.get("notes"))
                .and_then(Value::as_str)
                .map(str::to_string);
            AskUserQuestionAnswered {
                question,
                answers,
                note,
            }
        })
        .collect();
    Some(ToolDisplayData::AskUserQuestionResult(
        AskUserQuestionResult { questions },
    ))
}

#[cfg(test)]
#[path = "ask_user_question.test.rs"]
mod tests;
