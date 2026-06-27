//! Tool types.
//!
//! Re-exports from the provider crate for convenience.

pub use vercel_ai_provider::LanguageModelV4Tool;
pub use vercel_ai_provider::LanguageModelV4ToolChoice;
pub use vercel_ai_provider::ToolInvocation;
pub use vercel_ai_provider::language_model::v4::function_tool::LanguageModelV4FunctionTool;

// Legacy aliases for backward compatibility during migration
pub type ToolDefinitionV4 = LanguageModelV4FunctionTool;
pub type ToolChoice = LanguageModelV4ToolChoice;
