//! Middleware that appends input examples to tool descriptions.

use std::sync::Arc;

use vercel_ai_provider::AISdkError;
use vercel_ai_provider::LanguageModelV4CallOptions;
use vercel_ai_provider::LanguageModelV4Middleware;
use vercel_ai_provider::LanguageModelV4Tool;
use vercel_ai_provider::language_model_middleware::TransformParamsOptions;

/// Middleware that appends input examples to tool descriptions.
///
/// This is useful for providers that don't natively support the `inputExamples`
/// property. The middleware serializes examples into the tool's description text.
pub struct AddToolInputExamplesMiddleware {
    /// Prefix to prepend before examples.
    prefix: String,
    /// Whether to remove inputExamples after adding them.
    remove: bool,
}

impl AddToolInputExamplesMiddleware {
    /// Create a new add tool input examples middleware.
    pub fn new() -> Self {
        Self {
            prefix: "Input Examples:".to_string(),
            remove: true,
        }
    }

    /// Create with custom options.
    #[allow(dead_code)]
    pub fn with_options(prefix: impl Into<String>, remove: bool) -> Self {
        Self {
            prefix: prefix.into(),
            remove,
        }
    }
}

impl Default for AddToolInputExamplesMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl LanguageModelV4Middleware for AddToolInputExamplesMiddleware {
    async fn transform_params(
        &self,
        options: TransformParamsOptions,
    ) -> Result<LanguageModelV4CallOptions, AISdkError> {
        let params = options.params;

        let tools = match params.tools {
            Some(tools) => tools,
            None => return Ok(params),
        };

        let transformed_tools: Vec<LanguageModelV4Tool> = tools
            .into_iter()
            .map(|tool| {
                match tool {
                    LanguageModelV4Tool::Function(mut func_tool) => {
                        // Only transform tools with input examples
                        if let Some(ref examples) = func_tool.input_examples
                            && !examples.is_empty()
                        {
                            let formatted_examples: Vec<String> = examples
                                .iter()
                                .enumerate()
                                .map(|(i, ex)| {
                                    serde_json::to_string(&ex.input)
                                        .unwrap_or_else(|_| format!("Example {}", i + 1))
                                })
                                .collect();

                            let examples_section =
                                format!("{}\n{}", self.prefix, formatted_examples.join("\n"));

                            let new_description = match &func_tool.description {
                                Some(desc) => format!("{desc}\n\n{examples_section}"),
                                None => examples_section,
                            };

                            func_tool.description = Some(new_description);

                            if self.remove {
                                func_tool.input_examples = None;
                            }
                        }
                        LanguageModelV4Tool::Function(func_tool)
                    }
                    // Provider tools don't have input examples, pass through
                    other => other,
                }
            })
            .collect();

        Ok(LanguageModelV4CallOptions {
            tools: Some(transformed_tools),
            ..params
        })
    }
}

/// Create an add tool input examples middleware with default settings.
pub fn add_tool_input_examples_middleware() -> Arc<dyn LanguageModelV4Middleware> {
    Arc::new(AddToolInputExamplesMiddleware::new())
}

/// Create an add tool input examples middleware with custom options.
#[allow(dead_code)]
pub fn add_tool_input_examples_middleware_with_options(
    prefix: impl Into<String>,
    remove: bool,
) -> Arc<dyn LanguageModelV4Middleware> {
    Arc::new(AddToolInputExamplesMiddleware::with_options(prefix, remove))
}

#[cfg(test)]
#[path = "add_tool_input_examples_middleware.test.rs"]
mod tests;
