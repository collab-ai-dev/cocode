//! Per-fork canUseTool policies for memory-related forks.
//!
//! Three production policies + one shared helper:
//!
//! - [`create_auto_mem_handle`] — used by ExtractMemories.
//!   Allows `Read` / `Glob` / `Grep` unconditionally, read-only `Bash`
//!   via [`coco_shell_parser::safety::is_known_safe_command`], and
//!   `Edit` / `Write` / `apply_patch` only on `.md` paths under
//!   `memory_dir`. Everything else is denied.
//! - [`create_auto_dream_handle_with_telemetry`] — used by AutoDream.
//!   Same as auto-mem, plus `rm` for absolute `.md` paths under
//!   `memory_dir` so dream can prune stale topic files.
//! - [`create_session_mem_handle`] — used by SessionMemory (auto +
//!   manual). Allows `Edit` ONLY on the exact `memory_path`, allows
//!   `Read`, denies everything else. Tighter than auto-mem because
//!   session-memory writes should never sprawl outside the canonical
//!   session-memory file.
//!
//! ## Why path-prefix matters
//!
//! Both policies enforce a write fence so a misbehaving model can't
//! exfiltrate data into arbitrary locations. The fence is checked at
//! tool-execution time (step 3.5), so it composes with the
//! `allowed_write_roots` field on `ToolUseContext` — the callback's
//! check is the inner ring; the field is the outer ring.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use coco_tool_runtime::{
    CanUseToolCallContext, CanUseToolDecision, CanUseToolHandle, CanUseToolHandleRef,
    DecisionReason,
};
use serde_json::Value;

use crate::telemetry::{MemoryEvent, MemoryTelemetryEmitter, NoopEmitter};

/// Tool name constants used by the policies. Matches the canonical
/// `ToolName` strings via [`coco_types::ToolName::as_str`].
const TOOL_READ: &str = "Read";
const TOOL_GLOB: &str = "Glob";
const TOOL_GREP: &str = "Grep";
const TOOL_BASH: &str = "Bash";
const TOOL_EDIT: &str = "Edit";
const TOOL_WRITE: &str = "Write";
const TOOL_APPLY_PATCH: &str = "apply_patch";

/// Build the auto-mem canUseTool handle.
///
/// Policy:
/// - `Read` / `Glob` / `Grep` ⇒ Allow unconditionally.
/// - `Bash` ⇒ Allow when [`coco_shell_parser::safety::is_known_safe_command`]
///   returns `true`; else Deny.
/// - `Edit` / `Write` / `apply_patch` ⇒ Allow when every affected path
///   is a `.md` path under `memory_dir`; else Deny.
/// - Everything else ⇒ Deny.
pub fn create_auto_mem_handle(memory_dir: PathBuf) -> CanUseToolHandleRef {
    Arc::new(AutoMemHandle {
        memory_dir,
        telemetry: Arc::new(NoopEmitter),
        allow_rm_md_bash: false,
    })
}

/// Build the auto-mem handle with a telemetry emitter wired in so
/// `ExtractionToolDenied` events fire on each policy denial —
/// emitting `tengu_auto_mem_tool_denied` per deny so the variant
/// reaches dashboards.
pub fn create_auto_mem_handle_with_telemetry(
    memory_dir: PathBuf,
    telemetry: Arc<dyn MemoryTelemetryEmitter>,
) -> CanUseToolHandleRef {
    Arc::new(AutoMemHandle {
        memory_dir,
        telemetry,
        allow_rm_md_bash: false,
    })
}

/// Build the auto-dream handle without telemetry.
pub fn create_auto_dream_handle(memory_dir: PathBuf) -> CanUseToolHandleRef {
    Arc::new(AutoMemHandle {
        memory_dir,
        telemetry: Arc::new(NoopEmitter),
        allow_rm_md_bash: true,
    })
}

/// Build the auto-dream handle with a telemetry emitter wired in.
///
/// Auto-dream matches the v2.1.193 policy: read-only shell commands are
/// allowed, and `rm` may delete absolute `.md` paths inside the memory
/// directory so the consolidation pass can prune stale memories.
pub fn create_auto_dream_handle_with_telemetry(
    memory_dir: PathBuf,
    telemetry: Arc<dyn MemoryTelemetryEmitter>,
) -> CanUseToolHandleRef {
    Arc::new(AutoMemHandle {
        memory_dir,
        telemetry,
        allow_rm_md_bash: true,
    })
}

struct AutoMemHandle {
    memory_dir: PathBuf,
    telemetry: Arc<dyn MemoryTelemetryEmitter>,
    allow_rm_md_bash: bool,
}

impl std::fmt::Debug for AutoMemHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoMemHandle")
            .field("memory_dir", &self.memory_dir)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CanUseToolHandle for AutoMemHandle {
    async fn check(
        &self,
        tool_name: &str,
        input: &Value,
        ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        let decision = match tool_name {
            TOOL_READ | TOOL_GLOB | TOOL_GREP => allow(DecisionReason::Other {
                reason: format!("auto_mem: {tool_name} unrestricted"),
            }),
            TOOL_BASH => {
                if bash_is_read_only(input) {
                    allow(DecisionReason::Other {
                        reason: "auto_mem: read-only bash".into(),
                    })
                } else if self.allow_rm_md_bash && bash_is_rm_md_under_root(input, &self.memory_dir)
                {
                    allow(DecisionReason::Other {
                        reason: "auto_mem: rm .md within memory_dir".into(),
                    })
                } else {
                    deny(
                        "auto_mem: bash command not in known-safe set".to_string(),
                        "auto_mem_bash_mutating",
                    )
                }
            }
            TOOL_EDIT | TOOL_WRITE => {
                if input_path_is_md_under_root(input, &self.memory_dir, &ctx.cwd) {
                    allow(DecisionReason::Other {
                        reason: "auto_mem: write within memory_dir".into(),
                    })
                } else {
                    deny(
                        format!(
                            "auto_mem: {tool_name} only allowed for .md paths under {}",
                            self.memory_dir.display()
                        ),
                        "auto_mem_write_outside_dir",
                    )
                }
            }
            TOOL_APPLY_PATCH => {
                if apply_patch_paths_are_md_under_root(input, &self.memory_dir, &ctx.cwd) {
                    allow(DecisionReason::Other {
                        reason: "auto_mem: patch .md within memory_dir".into(),
                    })
                } else {
                    deny(
                        format!(
                            "auto_mem: {tool_name} only allowed for .md paths under {}",
                            self.memory_dir.display()
                        ),
                        "auto_mem_write_outside_dir",
                    )
                }
            }
            other => deny(
                format!("auto_mem: tool '{other}' not in policy"),
                "auto_mem_unknown_tool",
            ),
        };
        // Every Deny fires `tengu_auto_mem_tool_denied` with the
        // attempted tool name. Surfacing this lets operators see
        // _which_ policy is biting — useful when a model misroutes
        // a write or stumbles into an unsupported tool.
        if matches!(decision, CanUseToolDecision::Deny { .. }) {
            self.telemetry.emit(MemoryEvent::ExtractionToolDenied {
                tool_name: tool_name.to_string(),
            });
        }
        decision
    }
}

/// Build the session-mem canUseTool handle.
///
/// Policy:
/// - `Read` ⇒ Allow.
/// - `Edit` ⇒ Allow ONLY when `input.file_path == memory_path` (exact
///   path match — session-memory writes are pinned to the canonical
///   session-memory file).
/// - Everything else ⇒ Deny.
pub fn create_session_mem_handle(memory_path: PathBuf) -> CanUseToolHandleRef {
    Arc::new(SessionMemHandle { memory_path })
}

#[derive(Debug)]
struct SessionMemHandle {
    memory_path: PathBuf,
}

#[async_trait]
impl CanUseToolHandle for SessionMemHandle {
    async fn check(
        &self,
        tool_name: &str,
        input: &Value,
        _ctx: &CanUseToolCallContext,
    ) -> CanUseToolDecision {
        match tool_name {
            TOOL_READ => allow(DecisionReason::Other {
                reason: "session_mem: Read unrestricted".into(),
            }),
            TOOL_EDIT => {
                let path = input.get("file_path").and_then(|v| v.as_str());
                if let Some(p) = path
                    && Path::new(p) == self.memory_path.as_path()
                {
                    return allow(DecisionReason::Other {
                        reason: "session_mem: Edit on canonical file".into(),
                    });
                }
                deny(
                    format!(
                        "session_mem: Edit only allowed on {} (got {:?})",
                        self.memory_path.display(),
                        path
                    ),
                    "session_mem_edit_wrong_path",
                )
            }
            other => deny(
                format!("session_mem: tool '{other}' not in policy"),
                "session_mem_unknown_tool",
            ),
        }
    }
}

// ── Helpers ────────────────────────────────────────────────────

fn allow(reason: DecisionReason) -> CanUseToolDecision {
    CanUseToolDecision::Allow {
        updated_input: None,
        decision_reason: reason,
    }
}

fn deny(message: String, reason_label: &str) -> CanUseToolDecision {
    CanUseToolDecision::Deny {
        message,
        decision_reason: DecisionReason::Other {
            reason: reason_label.to_string(),
        },
    }
}

/// Is the Bash input's `command` known-safe (read-only)?
///
/// Uses `coco_shell_parser::ShellParser::try_extract_safe_commands`
/// to parse the command into a sequence of word-only argv stages
/// (chained with safe operators `&&` / `||` / `;` / `|`). When the
/// parse returns `None` the command has a redirection / subshell /
/// command-substitution — we fail closed.
///
/// Each stage's argv goes through
/// `coco_shell_parser::safety::is_known_safe_command`; ALL stages
/// must pass for the whole pipeline to be allowed. This means
/// `git log --oneline | head -10` (two safe stages joined by a
/// safe operator) is allowed, but `echo bad > /etc/passwd` (which
/// has a redirection) and `rm -rf /` (mutating first stage) are
/// rejected.
///
/// Uses the same full shell parse + per-stage safe-command lookup.
fn bash_is_read_only(input: &Value) -> bool {
    let Some(cmd) = input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }
    let mut parser = coco_shell_parser::ShellParser::new();
    let parsed = parser.parse(trimmed);
    let Some(stages) = parsed.try_extract_safe_commands() else {
        // Parse contains a redirection / subshell / command-sub /
        // syntax error — fail closed.
        return false;
    };
    if stages.is_empty() {
        return false;
    }
    stages
        .iter()
        .all(|argv| coco_shell_parser::safety::is_known_safe_command(argv))
}

fn bash_is_rm_md_under_root(input: &Value, root: &Path) -> bool {
    let Some(cmd) = input.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut parser = coco_shell_parser::ShellParser::new();
    let parsed = parser.parse(trimmed);
    let Some(commands) = parsed.try_extract_safe_commands() else {
        return false;
    };
    let [argv] = commands.as_slice() else {
        return false;
    };
    let Some(program) = argv.first() else {
        return false;
    };
    if program != "rm" {
        return false;
    }

    let mut saw_path = false;
    let mut end_of_options = false;
    for arg in argv.iter().skip(1) {
        if !end_of_options {
            if arg == "--" {
                end_of_options = true;
                continue;
            }
            if arg.starts_with('-') {
                if arg == "--recursive" || arg.chars().skip(1).any(|c| matches!(c, 'r' | 'R')) {
                    return false;
                }
                continue;
            }
        }
        if arg.contains(['*', '?', '[']) {
            return false;
        }
        let path = Path::new(arg);
        if !path.is_absolute() || !path_is_md_under_root(path, root) {
            return false;
        }
        saw_path = true;
    }
    saw_path
}

fn input_path_is_md_under_root(input: &Value, root: &Path, cwd: &Path) -> bool {
    let path_str = input
        .get("file_path")
        .or_else(|| input.get("notebook_path"))
        .or_else(|| input.get("path"))
        .and_then(|v| v.as_str());
    let Some(p) = path_str else {
        return false;
    };
    let candidate = Path::new(p);
    let absolute = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    };
    path_is_md_under_root(&absolute, root)
}

fn apply_patch_paths_are_md_under_root(input: &Value, root: &Path, cwd: &Path) -> bool {
    let Some(patch) = input.get("patch").and_then(|v| v.as_str()) else {
        return false;
    };
    let Ok(cwd) = coco_utils_absolute_path::AbsolutePathBuf::from_absolute_path(cwd) else {
        return false;
    };
    let Ok(parsed) = coco_apply_patch::parse_patch(patch) else {
        return false;
    };
    let path_effects = coco_apply_patch::collect_path_effects(&parsed.hunks, &cwd);
    !path_effects.permission_paths.is_empty()
        && path_effects
            .permission_paths
            .iter()
            .all(|path| path_is_md_under_root(path, root))
}

fn path_is_md_under_root(candidate: &Path, root: &Path) -> bool {
    if !candidate.to_string_lossy().ends_with(".md") {
        return false;
    }
    path_under_root(candidate, root)
}

/// True when `candidate` is a descendant of `root`.
///
/// Two-pass containment check:
///
/// 1. **Symlink-aware**: resolve the deepest existing ancestor of
///    both `root` and `candidate` via [`crate::path::realpath_deepest_existing`].
///    A model that plants `<memdir>/x -> /etc/passwd` and then asks
///    to `Edit` it gets caught here — the canonical candidate resolves
///    outside `canonical_root` even though the lexical form is under it.
///    Returns `false` on any non-recoverable symlink failure (ELOOP,
///    dangling, EACCES) — fail closed.
/// 2. **Lexical fallback**: if neither side could be canonicalized
///    (typically because we're testing against a hypothetical path on
///    a filesystem with no real `root` yet), compare via lexically
///    normalized `starts_with`. This is intentional only for the
///    benign "root doesn't exist yet" case; the symlink-escape vector
///    requires the symlink to actually exist on disk and so always
///    takes the canonical path.
fn path_under_root(candidate: &Path, root: &Path) -> bool {
    // Two-pass containment:
    //
    // (a) Lexical normalization first — collapses `.` and `..`
    //     components so `<memdir>/../etc/passwd`-style escapes can
    //     never slip past the symlink layer (realpath rejoins
    //     non-existing tail components verbatim, so the walk-up
    //     produces `<memdir>/../etc/passwd`, which deceptively
    //     `starts_with` `<memdir>` under `PathBuf::starts_with`
    //     component semantics).
    //
    // (b) Symlink resolution on the lexically-normalized form —
    //     catches the case where the model points at a symlink
    //     rooted inside `memory_dir` that resolves OUTSIDE. The
    //     `realpath_deepest_existing` helper fails closed on ELOOP /
    //     EACCES / dangling-symlink — see `path::symlink` for the
    //     full taxonomy.
    let lex_root = lexical_normalize(root);
    let lex_candidate = lexical_normalize(candidate);
    let real_root = crate::path::realpath_deepest_existing(&lex_root);
    let real_candidate = crate::path::realpath_deepest_existing(&lex_candidate);
    match (real_root.as_ref(), real_candidate.as_ref()) {
        // Both sides canonicalized — primary security check.
        (Some(rr), Some(rc)) => rc.starts_with(rr),
        // Both failed to canonicalize — typically because neither
        // path exists on disk (test fixtures, hypothetical paths
        // the model proposes). Lexical-only is safe here because
        // realpath's `None` for both sides means "doesn't exist."
        // The lexical normalization above already collapsed any
        // `..` traversal.
        (None, None) => lex_candidate.starts_with(&lex_root),
        // Asymmetric: one side resolved, the other failed.
        // `realpath_deepest_existing` returns `None` on ELOOP /
        // EACCES / **dangling symlink** (see `path::symlink:51-58`).
        // The exploit: a misbehaving subagent plants
        // `<memdir>/escape -> /etc/passwd` as a dangling symlink.
        // `lex_root` canonicalizes; `lex_candidate` fails because
        // of the dangling target. Falling back to lexical here
        // would Allow — POSIX write semantics would then create
        // `/etc/passwd` through the link. Fail closed instead.
        _ => {
            tracing::warn!(
                target: "coco_memory::can_use_tool",
                root = %lex_root.display(),
                candidate = %lex_candidate.display(),
                root_resolved = real_root.is_some(),
                candidate_resolved = real_candidate.is_some(),
                "auto_mem fence: asymmetric realpath outcome, failing closed"
            );
            false
        }
    }
}

/// Lexically normalize a path: collapse `.` and `..` components.
/// Doesn't touch the filesystem so it works for hypothetical paths
/// the model might propose.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
#[path = "can_use_tool.test.rs"]
mod tests;
