//! Memory file discovery and loading.

use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;

/// Type of memory file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Managed by the system.
    Managed,
    /// User-level (~/.coco/memory/).
    User,
    /// Project-level (.coco/memory/).
    Project,
    /// Local (gitignored) memory.
    Local,
    /// Auto-extracted memory.
    AutoMem,
    /// Team memory.
    TeamMem,
}

/// A discovered memory file with its metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFileInfo {
    pub path: PathBuf,
    pub memory_type: MemoryType,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub globs: Option<Vec<String>>,
}

/// Discover memory files for a working directory.
///
/// Resolves the memory base via [`coco_config::global_config::config_home`].
/// TODO(parity): honour the `COCO_REMOTE_MEMORY_DIR` override here —
/// currently only the `coco-memory` crate consumes that env override
/// (via `MemoryConfig::resolve`); the context-layer discovery path
/// is still hard-coded to `config_home`.
pub fn get_memory_files(cwd: &Path) -> Vec<MemoryFileInfo> {
    let mut files = Vec::new();

    // Project memory: .coco/memory/
    let project_mem_dir = cwd.join(".coco/memory");
    collect_memory_files(&project_mem_dir, MemoryType::Project, &mut files);

    // Auto-memory and team memory live under the per-project facade —
    // single source of truth for the slug/NFC/hash math. No more
    // hand-rolled sanitize here.
    let memory_base = coco_config::global_config::config_home();
    let project_paths = coco_paths::ProjectPaths::new(memory_base.clone(), cwd);
    collect_memory_files(&project_paths.memory_dir(), MemoryType::AutoMem, &mut files);
    collect_memory_files(
        &project_paths.team_memory_dir(),
        MemoryType::TeamMem,
        &mut files,
    );

    // User memory: <memory_base>/memory/ (replaces the pre-fix
    // hard-coded `~/.claude/memory/` lookup, which silently
    // diverged from coco-rs's own config home).
    collect_memory_files(&memory_base.join("memory"), MemoryType::User, &mut files);

    files
}

/// Collect `.md` files from a directory into the memory file list.
fn collect_memory_files(dir: &Path, mem_type: MemoryType, out: &mut Vec<MemoryFileInfo>) {
    if !dir.is_dir() {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "md")
            && let Ok(content) = std::fs::read_to_string(&path)
        {
            out.push(MemoryFileInfo {
                path,
                memory_type: mem_type,
                content,
                parent: None,
                globs: None,
            });
        }
    }
}

/// Check if a path belongs to any memory-managed directory.
///
/// Used by the permission system to grant write carve-outs.
///
/// TODO(parity): plumb `memory_base` rather than calling
/// `config_home()` so `COCO_REMOTE_MEMORY_DIR` overrides apply here too.
pub fn is_memory_managed_path(path: &Path, cwd: &Path) -> bool {
    let project_paths =
        coco_paths::ProjectPaths::new(coco_config::global_config::config_home(), cwd);
    let auto_dir = project_paths.memory_dir();
    let project_mem = cwd.join(".coco/memory");

    path.starts_with(&auto_dir)
        || path.starts_with(&project_mem)
        || path.to_string_lossy().contains(".coco/memory")
}

/// Determine the memory type for a given path.
pub fn classify_memory_path(path: &Path, cwd: &Path) -> Option<MemoryType> {
    let project_paths =
        coco_paths::ProjectPaths::new(coco_config::global_config::config_home(), cwd);
    let auto_dir = project_paths.memory_dir();
    let team_dir = project_paths.team_memory_dir();
    let project_mem = cwd.join(".coco/memory");

    if path.starts_with(&team_dir) {
        Some(MemoryType::TeamMem)
    } else if path.starts_with(&auto_dir) {
        Some(MemoryType::AutoMem)
    } else if path.starts_with(&project_mem) {
        Some(MemoryType::Project)
    } else if path.to_string_lossy().contains("/.coco/memory/") {
        Some(MemoryType::User)
    } else {
        None
    }
}
