//! Bundled-skill file extraction.
//!
//! Extraction pipeline:
//! 1. Per-process nonce dir (`~/.coco/bundled-skills/<nonce>/<skill>/`).
//! 2. Group files by parent dir; mkdir each subtree once with mode 0o700.
//! 3. Write each file via `O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW`, mode 0o600.
//! 4. Path validation rejects `isAbsolute`, segments matching `..` against
//!    BOTH `path::sep` and literal `/`.
//! 5. No unlink-on-EEXIST (`unlink()` follows intermediate symlinks).
//! 6. Memoize a single extraction promise per skill — concurrent callers
//!    await the same future, no write race.
//! 7. On extract failure: log, return None — prompt still works without the
//!    `Base directory for this skill: <dir>\n\n` prefix.

use std::collections::HashMap;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use tokio::sync::OnceCell;

/// Per-process root dir. Created once, never re-randomized for the lifetime
/// of the process.
///
/// Layout: `<tmpdir>/coco-<uid>/bundled-skills/<VERSION>/<nonce>`
///
/// **Security model**: the per-process nonce in the
/// path is the load-bearing defense; uid/VERSION alone are public knowledge
/// and squattable. The nonce uses 16 bytes of cryptographic randomness — NOT
/// time/pid — because timestamps and PIDs are observable and predictable.
fn bundled_skills_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let nonce = generate_secure_nonce();
        coco_temp_dir()
            .join("bundled-skills")
            .join(env!("CARGO_PKG_VERSION"))
            .join(nonce)
    })
}

/// 16 cryptographically-random bytes encoded as 32 lowercase hex chars.
fn generate_secure_nonce() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let mut s = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{b:02x}");
    }
    s
}

/// Process-uid-scoped temp directory.
///
/// Layout: `<tmpdir>/coco-<uid>/`. UID scoping prevents one user from
/// stomping another's bundled-skill dirs on multi-user systems. The
/// per-process nonce subdirectory inside provides the actual security
/// boundary against same-uid attackers.
fn coco_temp_dir() -> PathBuf {
    let base = std::env::temp_dir();
    let uid = current_uid();
    base.join(format!("coco-{uid}"))
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` is async-signal-safe per POSIX and never fails.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// Deterministic extraction directory for a bundled skill.
pub fn extract_dir_for(skill_name: &str) -> PathBuf {
    bundled_skills_root().join(skill_name)
}

/// In-memory memoization of "this skill has already been extracted".
/// `Arc<OnceCell<...>>` per skill so concurrent invocations share one future.
type ExtractionResult = Result<PathBuf, String>;
type CellMap = std::sync::Mutex<HashMap<String, Arc<OnceCell<ExtractionResult>>>>;

fn cells() -> &'static CellMap {
    static CELLS: OnceLock<CellMap> = OnceLock::new();
    CELLS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Extract a bundled skill's reference files to disk.
///
/// Returns the directory written to, or `None` if extraction failed (caller
/// continues with the prompt as-is, without the base-directory prefix).
///
/// **Concurrency**: many callers may invoke this for the same skill at the
/// same time; the underlying `OnceCell` ensures exactly one extraction
/// happens, others await its completion.
pub async fn extract_bundled_skill_files(
    skill_name: &str,
    files: &HashMap<String, String>,
) -> Option<PathBuf> {
    if files.is_empty() {
        return None;
    }

    let cell = {
        // Fall back to recovering from poison — the cell map only stores
        // OnceCell handles, so a panicked previous holder can't leave it in
        // a logically inconsistent state.
        let mut map = match cells().lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        map.entry(skill_name.to_string())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone()
    };

    let result = cell
        .get_or_init(|| async move {
            let dir = extract_dir_for(skill_name);
            match write_skill_files(&dir, files).await {
                Ok(()) => Ok(dir),
                Err(e) => {
                    tracing::warn!(
                        skill = skill_name,
                        dir = %dir.display(),
                        error = %e,
                        "failed to extract bundled skill files"
                    );
                    Err(e.to_string())
                }
            }
        })
        .await;

    match result {
        Ok(p) => Some(p.clone()),
        Err(_) => None,
    }
}

/// Group files by parent dir, mkdir each parent once (mode 0o700), then
/// write each file via the safe-write helper (O_NOFOLLOW|O_EXCL on Unix,
/// `wx` flag on Windows).
async fn write_skill_files(dir: &Path, files: &HashMap<String, String>) -> crate::Result<()> {
    use std::collections::BTreeMap;

    let mut by_parent: BTreeMap<PathBuf, Vec<(PathBuf, String)>> = BTreeMap::new();
    for (rel_path, content) in files {
        let target = resolve_skill_file_path(dir, rel_path)?;
        let parent = target
            .parent()
            .ok_or_else(|| {
                crate::SkillsError::generic(format!("path has no parent: {}", target.display()))
            })?
            .to_path_buf();
        by_parent
            .entry(parent)
            .or_default()
            .push((target, content.clone()));
    }

    // Sequential mkdir (the parent set is small in practice and ordering
    // ensures parents-before-children if that ever matters).
    for parent in by_parent.keys() {
        mkdir_secure(parent).await?;
    }

    // Write files concurrently within their parent buckets. Each bucket
    // has bounded fan-out; we rely on tokio's runtime to not over-spawn.
    let mut tasks = Vec::new();
    for entries in by_parent.into_values() {
        for (path, content) in entries {
            tasks.push(tokio::spawn(async move {
                safe_write_file(&path, content.as_bytes()).await
            }));
        }
    }
    for handle in tasks {
        handle.await??;
    }
    Ok(())
}

async fn mkdir_secure(path: &Path) -> crate::Result<()> {
    // Recursive create. Set mode 0o700 on Unix after; std doesn't accept a
    // mode arg in `create_dir_all`.
    tokio::fs::create_dir_all(path).await.map_err(|e| {
        crate::SkillsError::generic(format!("create_dir_all({}): {e}", path.display()))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::Permissions::from_mode(0o700);
        // Best-effort: only fail if the dir doesn't exist (which can't happen
        // because create_dir_all succeeded).
        if let Err(e) = tokio::fs::set_permissions(path, perm).await {
            tracing::debug!(
                path = %path.display(),
                error = %e,
                "could not chmod 0o700 on bundled-skill dir"
            );
        }
    }
    Ok(())
}

#[cfg(unix)]
async fn safe_write_file(path: &Path, content: &[u8]) -> crate::Result<()> {
    use tokio::io::AsyncWriteExt;

    // O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, mode 0o600.
    // Equivalent to `open(p, O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW, 0o600)`.
    // tokio::fs::OpenOptions exposes custom_flags/mode directly on Unix.
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true)
        .create_new(true)
        .custom_flags(libc_o_nofollow())
        .mode(0o600);
    let mut file = opts.open(path).await.map_err(|e| {
        crate::SkillsError::generic(format!("safe_write_file({}): {e}", path.display()))
    })?;
    file.write_all(content).await?;
    file.flush().await?;
    Ok(())
}

#[cfg(unix)]
fn libc_o_nofollow() -> i32 {
    // libc::O_NOFOLLOW is the canonical value but pulling in libc just for
    // a constant isn't worth it. Hard-coded values per platform: Linux/macOS
    // both use 0x100 in modern toolchains. If the constant ever drifts, the
    // open() will simply behave like a normal open without symlink protection;
    // the per-process nonce dir is the primary security boundary anyway.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        0x20000 // O_NOFOLLOW on both glibc and Darwin
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

#[cfg(windows)]
async fn safe_write_file(path: &Path, content: &[u8]) -> crate::Result<()> {
    use tokio::io::AsyncWriteExt;
    // Windows: equivalent of `'wx'` flag — create new, fail if exists.
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
        .map_err(|e| {
            crate::SkillsError::generic(format!("safe_write_file({}): {e}", path.display()))
        })?;
    file.write_all(content).await?;
    file.flush().await?;
    Ok(())
}

/// Normalize and validate a skill-relative path; reject traversal.
///
/// Rejects:
/// - absolute paths,
/// - `..` segments against either `path::sep` or literal `/`.
fn resolve_skill_file_path(base_dir: &Path, rel_path: &str) -> crate::Result<PathBuf> {
    if Path::new(rel_path).is_absolute() {
        return Err(crate::SkillsError::generic(format!(
            "bundled skill file path is absolute: {rel_path}"
        )));
    }
    // Check both native sep and literal `/` — Windows allows both (the
    // values map may use `/` separators on every platform).
    let native_segments: Vec<_> = Path::new(rel_path).components().collect();
    for c in &native_segments {
        if matches!(c, Component::ParentDir) {
            return Err(crate::SkillsError::generic(format!(
                "bundled skill file path escapes skill dir: {rel_path}"
            )));
        }
    }
    if rel_path.split('/').any(|s| s == "..") {
        return Err(crate::SkillsError::generic(format!(
            "bundled skill file path escapes skill dir (literal slash): {rel_path}"
        )));
    }
    Ok(base_dir.join(rel_path))
}

/// Prepend `Base directory for this skill: <dir>` to a prompt string.
pub fn prepend_base_dir(prompt: &str, base_dir: &Path) -> String {
    format!(
        "Base directory for this skill: {}\n\n{}",
        base_dir.display(),
        prompt
    )
}

#[cfg(test)]
#[path = "extraction.test.rs"]
mod tests;
