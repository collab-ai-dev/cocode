//! Memory directory resolution + path validation.

mod resolve;
mod scope;
mod symlink;
mod team;
mod validate;

pub use resolve::MemoryDir;
pub use scope::MemoryScope;
pub use scope::SessionFileType;
pub use scope::WriteClassification;
pub use scope::classify_written_path;
pub use scope::is_auto_managed_memory_file;
pub use scope::is_auto_mem_file;
pub use scope::memory_scope_for_path;
pub use scope::should_bypass_dangerous_dirs;
pub use symlink::realpath_deepest_existing;
pub use team::validate_team_mem_key;
pub use team::validate_team_mem_write_path;
pub use validate::PathValidationError;
pub use validate::is_within_memory_dir;
pub use validate::sanitize_path_key;
pub use validate::validate_memory_path;
pub use validate::validate_resolved_path;
