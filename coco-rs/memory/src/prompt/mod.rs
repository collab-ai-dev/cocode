//! System prompt + extraction / dream / session templates.
//!
//! The verbatim prompt text lives in `text/*.md` files compiled in via
//! `include_str!`. Builders here only concatenate the static blocks
//! with run-time values (paths, manifest, message count).

mod builders;

pub use builders::SystemPromptVariant;
pub use builders::build_dream_prompt;
pub use builders::build_extract_prompt;
pub use builders::build_kairos_prompt;
pub use builders::build_session_memory_template;
pub use builders::build_session_memory_update_prompt;
pub use builders::build_system_prompt_section;
