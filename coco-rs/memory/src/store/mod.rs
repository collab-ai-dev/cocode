//! Pure data layer for memory entries: types, frontmatter, MEMORY.md index.

mod format;
mod index;
mod types;

pub use format::format_entry_as_markdown;
pub use format::parse_memory_entry;
pub use format::parse_memory_frontmatter;
pub use index::EntrypointTruncation;
pub use index::MAX_ENTRYPOINT_BYTES;
pub use index::MAX_ENTRYPOINT_LINES;
pub use index::MemoryIndex;
pub use index::MemoryIndexEntry;
pub use index::parse_memory_index;
pub use index::truncate_entrypoint_content;
pub use types::ENTRYPOINT_NAME;
pub use types::MemoryEntry;
pub use types::MemoryEntryType;
pub use types::MemoryFrontmatter;
