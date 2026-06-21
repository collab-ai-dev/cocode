//! Provider-specific tool definitions for the OpenAI Responses API.
//!
//! Each tool creates a `LanguageModelV4ProviderTool` that can be passed
//! to the Responses API.

pub mod apply_patch;
pub mod code_interpreter;
pub mod custom;
pub mod file_search;
pub mod image_generation;
pub mod local_shell;
pub mod mcp;
pub mod shell;
pub mod tool_search;
pub mod web_search;
pub mod web_search_preview;

pub use apply_patch::openai_apply_patch_tool;
pub use code_interpreter::openai_code_interpreter_tool;
pub use custom::openai_custom_grammar_tool;
pub use file_search::openai_file_search_tool;
pub use image_generation::ImageGenerationToolOptions;
pub use image_generation::openai_image_generation_tool;
pub use local_shell::openai_local_shell_tool;
pub use mcp::McpToolOptions;
pub use mcp::openai_mcp_tool;
pub use shell::openai_shell_tool;
pub use tool_search::openai_tool_search_tool;
pub use web_search::openai_web_search_tool;
pub use web_search_preview::openai_web_search_preview_tool;
