//! Generate structured object from a prompt.
//!
//! This module provides `generate_object` and `stream_object` functions
//! for generating structured output that conforms to a JSON schema.

mod generate;
mod generate_object_result;
mod inject_json_instruction;
mod output_strategy;
mod parse_validate;
mod repair_text;
mod stream_object;
mod validate_input;

/// Mode for generating structured output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ObjectGenerationMode {
    /// Auto - let the SDK choose the best mode.
    #[default]
    Auto,
    /// JSON mode - request JSON output but no schema validation.
    Json,
    /// Tool mode - use tool calling for structured output.
    Tool,
    /// Grammar mode - use grammar-constrained generation.
    Grammar,
}

// Re-exports from generate_object
pub use generate::GenerateObjectOptions;
pub use generate::generate_object;

// Re-exports from generate_object_result
pub use generate_object_result::GenerateObjectFinishEvent;
pub use generate_object_result::GenerateObjectResult;

// Re-exports from stream_object
pub use stream_object::ObjectStreamPart;
pub use stream_object::StreamObjectFinishEvent;
pub use stream_object::StreamObjectOptions;
pub use stream_object::StreamObjectResult;
pub use stream_object::stream_object;

// Re-exports from sub-modules
pub use inject_json_instruction::inject_json_instruction;
pub use inject_json_instruction::inject_json_instruction_with_options;
pub use output_strategy::ObjectOutputStrategy as OutputStrategy;
pub use parse_validate::ParsedObjectResult;
pub use parse_validate::parse_and_validate;
pub use parse_validate::parse_json_value;
pub use parse_validate::validate_against_schema;
pub use repair_text::RepairTextFunction;
pub use repair_text::repair_json_text;
pub use validate_input::determine_generation_mode;
pub use validate_input::validate_object_generation_input;

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;
