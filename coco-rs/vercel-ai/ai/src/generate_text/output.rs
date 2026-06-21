//! Output parameter support for generate_text.
//!
//! This module provides types for structured output in generate_text,
//! allowing users to request JSON output that conforms to a schema.

use vercel_ai_provider::JSONSchema;
use vercel_ai_provider::ResponseFormat;

/// Context for output parsing.
///
/// Provides additional context from the response to help output specs
/// decide how to parse the output.
pub struct OutputParseContext {
    /// The finish reason from the model.
    pub finish_reason: vercel_ai_provider::FinishReason,
    /// Token usage.
    pub usage: vercel_ai_provider::Usage,
}

/// Trait for output specifications.
///
/// This trait defines the interface for different output formats (text, object, array, choice, json).
/// Implementations provide schema information, response format configuration,
/// and parsing capabilities.
pub trait OutputSpec: Send + Sync {
    /// Get the name of this output spec.
    fn name(&self) -> &str;

    /// Get the response format for the provider, if any.
    fn response_format(&self) -> Option<ResponseFormat>;

    /// Parse a complete output string into a JSON value.
    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error>;

    /// Parse a complete output string with context.
    fn parse_complete_output_with_context(
        &self,
        text: &str,
        _context: &OutputParseContext,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        self.parse_complete_output(text)
    }

    /// Parse a partial output string (for streaming), returning whatever is parseable so far.
    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error>;
}

/// Create a text output spec (no schema, returns text as-is).
pub fn text_output() -> Box<dyn OutputSpec> {
    Box::new(TextOutputSpec)
}

/// Create an object output spec from a JSON schema.
pub fn object_output(schema: JSONSchema) -> Box<dyn OutputSpec> {
    Box::new(ObjectOutputSpec {
        schema,
        name: "output".to_string(),
        description: None,
    })
}

/// Create an array output spec from an element JSON schema.
pub fn array_output(element_schema: JSONSchema) -> Box<dyn OutputSpec> {
    let array_schema = serde_json::json!({
        "type": "array",
        "items": element_schema
    });
    Box::new(ObjectOutputSpec {
        schema: array_schema,
        name: "output".to_string(),
        description: None,
    })
}

/// Create a choice output spec that validates output is one of the allowed values.
pub fn choice_output(options: Vec<String>) -> Box<dyn OutputSpec> {
    Box::new(ChoiceOutputSpec { options })
}

/// Create a JSON output spec that accepts any valid JSON without schema.
pub fn json_output() -> Box<dyn OutputSpec> {
    Box::new(JsonOutputSpec)
}

/// Options for [`array_output_with`].
pub struct ArrayOutputOptions {
    /// The element schema.
    pub element_schema: JSONSchema,
    /// Optional name for the output schema.
    pub name: Option<String>,
    /// Optional description for the output schema.
    pub description: Option<String>,
}

/// Options for [`choice_output_with`].
pub struct ChoiceOutputOptions {
    /// The allowed choices.
    pub options: Vec<String>,
    /// Optional name for the output schema.
    pub name: Option<String>,
    /// Optional description for the output schema.
    pub description: Option<String>,
}

/// Options for [`json_output_with`].
pub struct JsonOutputOptions {
    /// Optional name for the output schema.
    pub name: Option<String>,
    /// Optional description for the output schema.
    pub description: Option<String>,
}

/// Create an array output spec with name/description, using a wrapped object schema.
///
/// Wraps the element schema in `{ type: object, properties: { elements: { type: array, items: schema } }, required: ["elements"] }`
/// and unwraps `.elements` during parse.
pub fn array_output_with(opts: ArrayOutputOptions) -> Box<dyn OutputSpec> {
    Box::new(WrappedArrayOutputSpec {
        element_schema: opts.element_schema,
        name: opts.name.unwrap_or_else(|| "output".to_string()),
        description: opts.description,
    })
}

/// Create a choice output spec with name/description, using a wrapped object schema.
///
/// Wraps the choice in `{ type: object, properties: { result: { type: string, enum: options } }, required: ["result"] }`
/// and unwraps `.result` during parse.
pub fn choice_output_with(opts: ChoiceOutputOptions) -> Box<dyn OutputSpec> {
    Box::new(WrappedChoiceOutputSpec {
        options: opts.options,
        name: opts.name.unwrap_or_else(|| "choice".to_string()),
        description: opts.description,
    })
}

/// Create a JSON output spec with optional name/description.
///
/// Like [`json_output`] but allows specifying name and description.
pub fn json_output_with(opts: JsonOutputOptions) -> Box<dyn OutputSpec> {
    Box::new(NamedJsonOutputSpec {
        name: opts.name.unwrap_or_else(|| "json".to_string()),
        description: opts.description,
    })
}

/// Text output spec — no schema validation, returns raw text.
struct TextOutputSpec;

impl OutputSpec for TextOutputSpec {
    fn name(&self) -> &str {
        "text"
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        None
    }

    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        Ok(Some(serde_json::Value::String(text.to_string())))
    }

    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        Ok(Some(serde_json::Value::String(text.to_string())))
    }
}

/// Object/array output spec — validates against a JSON schema.
struct ObjectOutputSpec {
    schema: JSONSchema,
    name: String,
    description: Option<String>,
}

impl OutputSpec for ObjectOutputSpec {
    fn name(&self) -> &str {
        &self.name
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        let mut format =
            ResponseFormat::json_with_schema(self.schema.clone()).with_name(self.name.clone());
        if let Some(ref desc) = self.description {
            format = format.with_description(desc.clone());
        }
        Some(format)
    }

    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        Ok(Some(value))
    }

    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Ok(None), // Partial JSON is not yet parseable
        }
    }
}

/// Choice output spec — validates output is one of the allowed values.
struct ChoiceOutputSpec {
    options: Vec<String>,
}

impl OutputSpec for ChoiceOutputSpec {
    fn name(&self) -> &str {
        "choice"
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        // Build an enum schema
        let schema = serde_json::json!({
            "type": "string",
            "enum": self.options
        });
        Some(ResponseFormat::json_with_schema(schema).with_name("choice"))
    }

    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        // Try parsing as JSON string first (e.g., "\"option1\"")
        let value: String = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(_) => text.trim().to_string(), // Treat as raw text
        };

        if self.options.contains(&value) {
            Ok(Some(serde_json::Value::String(value)))
        } else {
            Ok(None) // Not a valid choice
        }
    }

    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        // For partial output, check if any option starts with the text
        let trimmed = text.trim().trim_matches('"');
        for option in &self.options {
            if option.starts_with(trimmed) {
                return Ok(Some(serde_json::Value::String(trimmed.to_string())));
            }
        }
        Ok(None)
    }
}

/// JSON output spec — accepts any valid JSON without schema validation.
struct JsonOutputSpec;

impl OutputSpec for JsonOutputSpec {
    fn name(&self) -> &str {
        "json"
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        Some(ResponseFormat::json())
    }

    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        Ok(Some(value))
    }

    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Ok(None),
        }
    }
}

/// Wrapped array output spec — wraps element schema in an object with an `elements` field.
///
/// The provider sees `{ type: object, properties: { elements: { type: array, items: schema } }, required: ["elements"] }`.
/// During parse, the `.elements` field is extracted and returned.
struct WrappedArrayOutputSpec {
    element_schema: JSONSchema,
    name: String,
    description: Option<String>,
}

impl OutputSpec for WrappedArrayOutputSpec {
    fn name(&self) -> &str {
        &self.name
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        let wrapped_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "elements": {
                    "type": "array",
                    "items": self.element_schema
                }
            },
            "required": ["elements"],
            "additionalProperties": false
        });
        let mut format =
            ResponseFormat::json_with_schema(wrapped_schema).with_name(self.name.clone());
        if let Some(ref desc) = self.description {
            format = format.with_description(desc.clone());
        }
        Some(format)
    }

    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        // Unwrap the `.elements` field from the wrapper object.
        if let Some(elements) = value.get("elements") {
            Ok(Some(elements.clone()))
        } else {
            // `.elements` absent means the model didn't produce the wrapper — return None.
            Ok(None)
        }
    }

    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => {
                if let Some(elements) = value.get("elements") {
                    Ok(Some(elements.clone()))
                } else {
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        }
    }
}

/// Wrapped choice output spec — wraps choice enum in an object with a `result` field.
///
/// The provider sees `{ type: object, properties: { result: { type: string, enum: options } }, required: ["result"] }`.
/// During parse, the `.result` field is extracted and validated against the allowed options.
struct WrappedChoiceOutputSpec {
    options: Vec<String>,
    name: String,
    description: Option<String>,
}

impl OutputSpec for WrappedChoiceOutputSpec {
    fn name(&self) -> &str {
        &self.name
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        let wrapped_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "result": {
                    "type": "string",
                    "enum": self.options
                }
            },
            "required": ["result"],
            "additionalProperties": false
        });
        let mut format =
            ResponseFormat::json_with_schema(wrapped_schema).with_name(self.name.clone());
        if let Some(ref desc) = self.description {
            format = format.with_description(desc.clone());
        }
        Some(format)
    }

    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        // Unwrap the `.result` field from the wrapper object.
        let result_str = if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
            result.to_string()
        } else {
            // `.result` absent means the model didn't produce the wrapper — return None.
            return Ok(None);
        };

        if self.options.contains(&result_str) {
            Ok(Some(serde_json::Value::String(result_str)))
        } else {
            Ok(None)
        }
    }

    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => {
                let candidate = if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                    result
                } else {
                    return Ok(None);
                };

                for option in &self.options {
                    if option.starts_with(candidate) {
                        return Ok(Some(serde_json::Value::String(candidate.to_string())));
                    }
                }
                Ok(None)
            }
            Err(_) => Ok(None),
        }
    }
}

/// Named JSON output spec — accepts any valid JSON with optional name/description.
struct NamedJsonOutputSpec {
    name: String,
    description: Option<String>,
}

impl OutputSpec for NamedJsonOutputSpec {
    fn name(&self) -> &str {
        &self.name
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        let mut format = ResponseFormat::json();
        if let Some(ref desc) = self.description {
            format = format.with_description(desc.clone());
        }
        Some(format)
    }

    fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        Ok(Some(value))
    }

    fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Ok(None),
        }
    }
}

/// Output configuration for structured output in generate_text.
///
/// When specified, the model will generate output that conforms to
/// the provided JSON schema.
#[derive(Debug, Clone)]
pub struct Output {
    /// The JSON schema for the output.
    pub schema: JSONSchema,
    /// Optional name for the schema.
    pub name: Option<String>,
    /// Optional description for the schema.
    pub description: Option<String>,
}

impl Output {
    /// Create a new output configuration with a JSON schema.
    pub fn new(schema: JSONSchema) -> Self {
        Self {
            schema,
            name: None,
            description: None,
        }
    }

    /// Create an output configuration from a schema with a name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Create an output configuration from a schema with a description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Create from a serializable type using schemars.
    pub fn from_type<T: schemars::JsonSchema>() -> Self {
        let schema = schemars::schema_for!(T);
        let schema_value = serde_json::to_value(&schema).unwrap_or_default();
        Self::new(schema_value)
    }

    /// Convert to a ResponseFormat for the provider.
    pub fn to_response_format(&self) -> ResponseFormat {
        let name = self.name.clone().unwrap_or_else(|| "output".to_string());
        let mut format = ResponseFormat::json_with_schema(self.schema.clone()).with_name(name);

        if let Some(desc) = &self.description {
            format = format.with_description(desc.clone());
        }

        format
    }

    /// Parse a complete output string according to this output's schema.
    pub fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        Ok(Some(value))
    }

    /// Parse a partial output string (for streaming).
    pub fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Ok(None),
        }
    }
}

/// Output mode enum for the output strategy pattern.
///
/// Supports different output modes: text (default), object with schema,
/// and array with element schema.
#[derive(Debug, Clone)]
pub enum OutputMode {
    /// Plain text output (default).
    Text,
    /// Structured object output conforming to a JSON schema.
    Object {
        /// The JSON schema for the output object.
        schema: JSONSchema,
        /// Optional name for the schema.
        name: Option<String>,
        /// Optional description for the schema.
        description: Option<String>,
    },
    /// Array output where each element conforms to a JSON schema.
    Array {
        /// The JSON schema for each array element.
        element_schema: JSONSchema,
    },
}

impl OutputMode {
    /// Create a text output mode.
    pub fn text() -> Self {
        Self::Text
    }

    /// Create an object output mode with a schema.
    pub fn object(schema: JSONSchema) -> Self {
        Self::Object {
            schema,
            name: None,
            description: None,
        }
    }

    /// Create an object output mode from a type.
    pub fn object_from_type<T: schemars::JsonSchema>() -> Self {
        let schema = schemars::schema_for!(T);
        let schema_value = serde_json::to_value(&schema).unwrap_or_default();
        Self::Object {
            schema: schema_value,
            name: None,
            description: None,
        }
    }

    /// Create an array output mode with an element schema.
    pub fn array(element_schema: JSONSchema) -> Self {
        Self::Array { element_schema }
    }

    /// Parse a complete output string according to this mode.
    pub fn parse_complete_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match self {
            Self::Text => Ok(Some(serde_json::Value::String(text.to_string()))),
            Self::Object { .. } | Self::Array { .. } => {
                let value: serde_json::Value = serde_json::from_str(text)?;
                Ok(Some(value))
            }
        }
    }

    /// Parse a partial output string (for streaming).
    pub fn parse_partial_output(
        &self,
        text: &str,
    ) -> Result<Option<serde_json::Value>, serde_json::Error> {
        match self {
            Self::Text => Ok(Some(serde_json::Value::String(text.to_string()))),
            Self::Object { .. } | Self::Array { .. } => {
                // Best-effort parse for partial JSON
                match serde_json::from_str::<serde_json::Value>(text) {
                    Ok(value) => Ok(Some(value)),
                    Err(_) => Ok(None), // Partial JSON is not yet parseable
                }
            }
        }
    }

    /// Convert to a ResponseFormat for the provider.
    pub fn to_response_format(&self) -> Option<ResponseFormat> {
        match self {
            Self::Text => None,
            Self::Object {
                schema,
                name,
                description,
            } => {
                let format_name = name.clone().unwrap_or_else(|| "output".to_string());
                let mut format =
                    ResponseFormat::json_with_schema(schema.clone()).with_name(format_name);
                if let Some(desc) = description {
                    format = format.with_description(desc.clone());
                }
                Some(format)
            }
            Self::Array { element_schema } => {
                // Wrap element schema in an array schema
                let array_schema = serde_json::json!({
                    "type": "array",
                    "items": element_schema
                });
                Some(ResponseFormat::json_with_schema(array_schema).with_name("output"))
            }
        }
    }
}

/// Trait for output strategies.
///
/// This trait allows different output formats to be used with generate_text.
pub trait OutputStrategy {
    /// Get the JSON schema for this output, if any.
    fn json_schema(&self) -> Option<&JSONSchema>;

    /// Get the response format for this output.
    fn response_format(&self) -> Option<ResponseFormat>;

    /// Get the name of the output.
    fn strategy_name(&self) -> Option<&str>;
}

impl OutputStrategy for Output {
    fn json_schema(&self) -> Option<&JSONSchema> {
        Some(&self.schema)
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        Some(self.to_response_format())
    }

    fn strategy_name(&self) -> Option<&str> {
        self.name.as_deref()
    }
}

impl OutputStrategy for Option<Output> {
    fn json_schema(&self) -> Option<&JSONSchema> {
        self.as_ref()?.json_schema()
    }

    fn response_format(&self) -> Option<ResponseFormat> {
        self.as_ref()?.response_format()
    }

    fn strategy_name(&self) -> Option<&str> {
        self.as_ref()?.strategy_name()
    }
}

#[cfg(test)]
#[path = "output.test.rs"]
mod tests;
