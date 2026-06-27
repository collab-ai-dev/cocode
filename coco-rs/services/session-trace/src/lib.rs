//! Semantic execution-trace bundles for post-mortem debugging and golden
//! replay. See `CLAUDE.md` for the pipeline and integration notes.
//!
//! `coco-wire-dump` records raw LLM *traffic*; this crate records execution
//! *semantics* — tool lifecycle, MCP calls, turn boundaries, compaction edges —
//! as a small, stable, replayable artifact.

pub mod error;
pub mod event;
pub mod replay;
pub mod writer;

pub use error::Result;
pub use error::SessionTraceError;
pub use event::TraceEvent;
pub use replay::ReplayBundle;
pub use replay::ToolCallStatus;
pub use replay::replay_bundle;
pub use writer::EVENTS_FILE;
pub use writer::MANIFEST_FILE;
pub use writer::SCHEMA_VERSION;
pub use writer::TraceManifest;
pub use writer::TraceRecord;
pub use writer::TraceWriter;
