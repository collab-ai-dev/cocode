//! Type validation error type.

use std::fmt;
use thiserror::Error;

/// Context for type validation errors.
#[derive(Debug, Clone, Default)]
pub struct TypeValidationContext {
    /// Field path in dot notation (e.g., "message.metadata", "message.parts[3].data").
    pub field: Option<String>,
    /// Entity name (e.g., tool name, data type name).
    pub entity_name: Option<String>,
    /// Entity identifier (e.g., message ID, tool call ID).
    pub entity_id: Option<String>,
}

impl TypeValidationContext {
    /// Create a new validation context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the field path.
    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    /// Set the entity name.
    pub fn with_entity_name(mut self, name: impl Into<String>) -> Self {
        self.entity_name = Some(name.into());
        self
    }

    /// Set the entity ID.
    pub fn with_entity_id(mut self, id: impl Into<String>) -> Self {
        self.entity_id = Some(id.into());
        self
    }
}

/// Error thrown when type validation fails.
#[derive(Debug, Error)]
pub struct TypeValidationError {
    /// The value that failed validation.
    pub value: serde_json::Value,
    /// The validation context.
    pub context: Option<TypeValidationContext>,
    /// The error message.
    pub message: String,
    /// The underlying cause.
    #[source]
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Display for TypeValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl TypeValidationError {
    /// Create a new type validation error.
    pub fn new(value: serde_json::Value, cause: Box<dyn std::error::Error + Send + Sync>) -> Self {
        let message = format!(
            "Type validation failed: Value: {}.\nError message: {}",
            serde_json::to_string(&value).unwrap_or_default(),
            cause
        );
        Self {
            value,
            context: None,
            message,
            cause: Some(cause),
        }
    }

    /// Create with context.
    pub fn with_context(
        value: serde_json::Value,
        cause: Box<dyn std::error::Error + Send + Sync>,
        context: TypeValidationContext,
    ) -> Self {
        let mut context_prefix = "Type validation failed".to_string();

        if let Some(ref field) = context.field {
            context_prefix.push_str(&format!(" for {field}"));
        }

        if context.entity_name.is_some() || context.entity_id.is_some() {
            context_prefix.push_str(" (");
            let mut parts = Vec::new();
            if let Some(ref name) = context.entity_name {
                parts.push(name.clone());
            }
            if let Some(ref id) = context.entity_id {
                parts.push(format!("id: \"{id}\""));
            }
            context_prefix.push_str(&parts.join(", "));
            context_prefix.push(')');
        }

        let value_str = serde_json::to_string(&value).unwrap_or_default();
        let message = format!("{context_prefix}: Value: {value_str}.\nError message: {cause}");

        Self {
            value,
            context: Some(context),
            message,
            cause: Some(cause),
        }
    }
}

#[cfg(test)]
#[path = "type_validation_error.test.rs"]
mod tests;
