//! System prompt + extraction / dream / session templates.
//!
//! Prompt text is inlined in `builders.rs`; runtime values include
//! paths, manifests, message counts, and model-specific file mutation
//! tool names derived from `ToolOverrides`.

mod builders;

pub use builders::FileMutationPromptTools;
pub use builders::SystemPromptVariant;
pub use builders::build_dream_prompt;
pub use builders::build_extract_prompt;
pub use builders::build_kairos_prompt;
pub use builders::build_session_memory_template;
pub use builders::build_session_memory_update_prompt;
pub use builders::build_system_prompt_section;
