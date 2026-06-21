//! `/memory` command — open the memory-file selector dialog.
//!
//! Behavior:
//! 1. Pre-flight: re-discovers memory files to populate the selector list.
//! 2. Computes the row list: discovered files + "create `~/.coco/CLAUDE.md`" /
//!    "create `./CLAUDE.md`" placeholders + auto-memory folder entries.
//! 3. On select: caller (TUI) writes the file (mode `wx` semantics) and
//!    opens it in `$VISUAL || $EDITOR`.
//! 4. On cancel: caller emits `Cancelled memory editing` system message.
//!
//! Handler scope: re-discovers memory files on every `/memory` invocation
//! and emits `CommandResult::OpenDialog(DialogSpec::MemoryFileSelector)`.
//! The dialog widget in `coco-tui` handles steps 3–4.

use async_trait::async_trait;
use std::path::Path;
use std::path::PathBuf;

use coco_context::MemoryFile;
use coco_context::MemoryFileSource;
use coco_context::discover_memory_files;

use crate::CommandHandler;
use crate::CommandResult;
use crate::DialogSpec;
use crate::MemoryFileEntry;
use crate::MemoryScope;

/// One active agent's per-type memory directory. Constructed by the
/// session bootstrap. The CLI resolver joins the `MemoryScope` + agent
/// type via [`coco_memory::agent_memory::agent_memory_dir`].
#[derive(Debug, Clone)]
pub struct AgentMemoryEntry {
    /// Agent type (sanitized via
    /// [`coco_paths::sanitize_agent_type_for_path`] on the caller side
    /// before reaching this struct).
    pub agent_type: String,
    /// One of "user" / "project" / "local" — the scope tag rendered
    /// in the dialog description column.
    pub scope_name: String,
    /// Resolved memory directory for the agent.
    pub dir: PathBuf,
}

pub struct MemoryDialogHandler {
    /// Project root (cwd).
    pub project_root: PathBuf,
    /// User config home (typically `~/.coco`).
    pub user_home: PathBuf,
    /// Optional managed dir (enterprise).
    pub managed_root: Option<PathBuf>,
    /// Auto-memory root for this project (`<config_home>/projects/
    /// <slug>/memory/`). `None` ⇒ auto-memory is disabled for this
    /// session and the folder rows are omitted entirely.
    pub auto_mem_dir: Option<PathBuf>,
    /// Team memory directory (under [`auto_mem_dir`]). `None` ⇒ team
    /// memory disabled.
    pub team_mem_dir: Option<PathBuf>,
    /// Per-agent memory directories for every active agent that
    /// declared a `memory: user|project|local` frontmatter entry.
    /// Empty vec ⇒ no agent rows. The vec is sorted by `agent_type`
    /// at construction so dialog ordering is deterministic.
    pub agent_mem_entries: Vec<AgentMemoryEntry>,
}

impl MemoryDialogHandler {
    pub fn new(project_root: PathBuf, user_home: PathBuf, managed_root: Option<PathBuf>) -> Self {
        Self {
            project_root,
            user_home,
            managed_root,
            auto_mem_dir: None,
            team_mem_dir: None,
            agent_mem_entries: Vec::new(),
        }
    }

    pub fn with_auto_mem(mut self, auto_mem_dir: PathBuf) -> Self {
        self.auto_mem_dir = Some(auto_mem_dir);
        self
    }

    pub fn with_team_mem(mut self, team_mem_dir: PathBuf) -> Self {
        self.team_mem_dir = Some(team_mem_dir);
        self
    }

    pub fn with_agent_memories(mut self, mut entries: Vec<AgentMemoryEntry>) -> Self {
        entries.sort_by(|a, b| a.agent_type.cmp(&b.agent_type));
        self.agent_mem_entries = entries;
        self
    }

    /// Build the entry list — order:
    /// (1) managed (when configured),
    /// (2) all discovered memory files via
    ///     [`coco_context::discover_memory_files`] in discovery order,
    /// (3) the two canonical "create me" placeholders for
    ///     `~/.coco/CLAUDE.md` and `<project>/CLAUDE.md` when
    ///     missing,
    /// (4) auto-memory / team-memory / per-agent folder rows when
    ///     auto-memory is enabled.
    ///
    /// Each `/memory` invocation recomputes — the dialog always sees
    /// a fresh discovery (newly-created CLAUDE.md files show up
    /// without a session restart).
    pub fn entries(&self) -> Vec<MemoryFileEntry> {
        let user_claudemd = self.user_home.join("CLAUDE.md");
        let project_claudemd = self.project_root.join("CLAUDE.md");

        let mut out: Vec<MemoryFileEntry> = Vec::new();

        // (1) Managed enterprise file. Discovery's
        // `~/.coco/CLAUDE.md` scope walks user-global, not managed —
        // we surface managed as a separate row when its path was
        // wired through.
        if let Some(m) = &self.managed_root {
            out.push(MemoryFileEntry {
                path: m.join("CLAUDE.md"),
                label: "Managed (enterprise) CLAUDE.md".into(),
                scope: MemoryScope::Managed,
                description: format!("Saved in {}", display_path(&m.join("CLAUDE.md"))),
                is_new: false,
                is_folder: false,
            });
        }

        // (2) Discovered files. `discover_memory_files` returns files
        // in load order (root → cwd, then nested via imports); the
        // dialog preserves that.
        let discovered: Vec<MemoryFile> = discover_memory_files(&self.project_root);
        let mut have_user = false;
        let mut have_project = false;
        for f in &discovered {
            let (scope, description) = describe_discovered(&f.path, f.source, &project_claudemd);
            if f.path == user_claudemd {
                have_user = true;
            }
            if f.path == project_claudemd {
                have_project = true;
            }
            let label = if f.path == user_claudemd {
                "User memory".to_string()
            } else if f.path == project_claudemd {
                "Project memory".to_string()
            } else {
                display_path(&f.path)
            };
            out.push(MemoryFileEntry {
                path: f.path.clone(),
                label,
                scope,
                description,
                is_new: false,
                is_folder: false,
            });
        }

        // (3) "Create me" placeholders for user / project CLAUDE.md
        // when discovery didn't surface them.
        if !have_user {
            out.push(MemoryFileEntry {
                path: user_claudemd.clone(),
                label: "User memory".into(),
                scope: MemoryScope::User,
                description: format!("Saved in {} (new)", display_path(&user_claudemd)),
                is_new: true,
                is_folder: false,
            });
        }
        if !have_project {
            out.push(MemoryFileEntry {
                path: project_claudemd.clone(),
                label: "Project memory".into(),
                scope: MemoryScope::Project,
                description: format!("Saved in {} (new)", display_path(&project_claudemd)),
                is_new: true,
                is_folder: false,
            });
        }

        // (4) Auto-mem / team-mem / per-agent folder rows. Gated on
        // `auto_mem_dir.is_some()` — when auto-memory is disabled the
        // whole folder section is omitted.
        if let Some(auto) = &self.auto_mem_dir {
            out.push(MemoryFileEntry {
                path: auto.clone(),
                label: "Open auto-memory folder".into(),
                scope: MemoryScope::AutoMemFolder,
                description: display_path(auto),
                is_new: false,
                is_folder: true,
            });
            if let Some(team) = &self.team_mem_dir {
                out.push(MemoryFileEntry {
                    path: team.clone(),
                    label: "Open team memory folder".into(),
                    scope: MemoryScope::TeamMemFolder,
                    description: display_path(team),
                    is_new: false,
                    is_folder: true,
                });
            }
            for agent in &self.agent_mem_entries {
                out.push(MemoryFileEntry {
                    path: agent.dir.clone(),
                    label: format!("Open {} agent memory", agent.agent_type),
                    scope: MemoryScope::AgentMemFolder,
                    description: format!("{} scope", agent.scope_name),
                    is_new: false,
                    is_folder: true,
                });
            }
        }

        out
    }
}

#[async_trait]
impl CommandHandler for MemoryDialogHandler {
    async fn execute_command(&self, _args: &str) -> crate::Result<CommandResult> {
        Ok(CommandResult::OpenDialog(DialogSpec::MemoryFileSelector {
            entries: self.entries(),
        }))
    }

    fn handler_name(&self) -> &str {
        "memory"
    }
}

/// Render a path with `~` substitution for `$HOME`. Falls back to lossy
/// display when the path is non-UTF8 (Windows / odd locales).
fn display_path(p: &Path) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Some(p_str) = p.to_str()
        && p_str.starts_with(&home)
    {
        return format!("~{}", &p_str[home.len()..]);
    }
    p.display().to_string()
}

/// Classify a discovered file's scope + dialog description. Used by
/// step (2) of [`MemoryDialogHandler::entries`].
///
/// `project_claudemd` lets us tag the canonical project root file
/// with the "checked in at ./CLAUDE.md" description;
/// any other `Project`-source file (subdir / @-imported) gets the
/// generic display path.
fn describe_discovered(
    path: &Path,
    source: MemoryFileSource,
    project_claudemd: &Path,
) -> (MemoryScope, String) {
    match source {
        MemoryFileSource::Managed => (
            MemoryScope::Managed,
            format!("Managed policy (read-only): {}", display_path(path)),
        ),
        MemoryFileSource::UserGlobal => (MemoryScope::User, "Saved in ~/.coco/CLAUDE.md".into()),
        MemoryFileSource::ProjectConfig => (
            MemoryScope::ProjectConfig,
            format!("Saved in {}", display_path(path)),
        ),
        MemoryFileSource::Project => {
            if path == project_claudemd {
                (MemoryScope::Project, "Saved in ./CLAUDE.md".into())
            } else {
                (MemoryScope::Subdir, "dynamically loaded".to_string())
            }
        }
        MemoryFileSource::Local => (
            MemoryScope::ProjectLocal,
            format!("Saved in {} (gitignored)", display_path(path)),
        ),
    }
}

#[cfg(test)]
#[path = "memory_dialog.test.rs"]
mod tests;
