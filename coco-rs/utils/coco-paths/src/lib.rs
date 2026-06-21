//! Single source of truth for project, session, and memory path layout.
//!
//! Why this crate exists: the pre-fix coco-rs codebase duplicated the
//! `sanitizePath` algorithm across `memory`, `core/context`, `tasks`,
//! and `plugins`, with at least one variant producing slugs that did
//! NOT match Claude Code's. The result was silent cross-tool data
//! isolation breakage — a session run under one runtime and one under
//! coco-rs against the same cwd would land in different `projects/<slug>/`
//! directories, with no error surfaced.
//!
//! Implements: `sanitizePath`, `simpleHash` (djb2), `getProjectsDir` /
//! `getProjectDir`, `findProjectDir` (prefix fallback),
//! `resolveSessionFilePath`, memory base / project / daily-log paths,
//! and `sanitizeAgentTypeForPath`.
//!
//! The crate is dependency-light on purpose: only `unicode-normalization`
//! at runtime — no env var reads, no subprocesses, no filesystem walks
//! beyond what `find_project_dir`'s prefix scan requires. Git operations
//! (worktree enumeration, canonical-root resolution) live in
//! [`coco_git`](https://docs.rs/coco-git); callers compose the two
//! crates explicitly so the path layer stays platform-neutral.

pub mod djb2;
pub mod nfc;
pub mod project_paths;
pub mod projects_root;
pub mod relative;
pub mod sanitize;
pub mod slug;

pub use djb2::{djb2, simple_hash};
pub use nfc::normalize_nfc;
pub use project_paths::ProjectPaths;
pub use projects_root::{find_project_dir, project_dir, projects_root};
pub use relative::{normalize_lexical, path_to_posix, relative_posix_path};
pub use sanitize::{MAX_SANITIZED_LENGTH, sanitize_agent_type_for_path, sanitize_path};
pub use slug::ProjectSlug;
