//! Persistent cross-session memory.
//!
//! Structure:
//! - [`store`] — pure data: entry, frontmatter, MEMORY.md index, format
//! - [`path`] — git-canonical resolution, validation, scope, classify
//! - [`scan`] — single Scanner: 200-cap, 30-line frontmatter read, mtime sort
//! - [`recall`] — relevant-memory selection (LLM side-query + heuristic)
//! - [`lock`] — PID + mtime CAS lock for auto-dream consolidation
//! - [`prompt`] — system prompt + extract / dream / session templates
//! - [`service`] — async services: [`service::extract`], [`service::dream`],
//!   [`service::session`]
//! - [`runtime`] — [`runtime::MemoryRuntime`] composes the services
//! - [`compact_truncate`] — pure fn called by compact-side glue
//! - [`config`] — thin runtime adapter over [`coco_config::MemoryConfig`]
//! - [`telemetry`] — `MemoryEvent` + emitter trait

pub mod agent_memory;
pub mod agent_memory_snapshot;
pub mod can_use_tool;
pub mod compact_truncate;
pub mod config;
pub mod kairos;
pub mod lock;
pub mod notice;
pub mod path;
pub mod prompt;
pub mod recall;
pub mod runtime;
pub mod scan;
pub mod service;
pub mod store;
pub mod team_sync;
pub mod telemetry;

pub use config::MemoryConfig;
pub use notice::MemoryUserNotice;
pub use notice::NoticeInbox;
pub use notice::NoticeVerb;
pub use runtime::MemoryRuntime;
pub use runtime::SessionEnumerator;
pub use service::DreamService;
pub use service::ExtractService;
pub use service::SessionMemoryService;
pub use store::MemoryEntry;
pub use store::MemoryEntryType;
pub use store::MemoryFrontmatter;
pub use store::MemoryIndex;
pub use store::MemoryIndexEntry;
