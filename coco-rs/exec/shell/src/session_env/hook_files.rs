//! Read hook-emitted env-shell files for a session.
//!
//! Layout: `$COCO_HOME/session-env/<session_id>/{event}-hook-{idx}.sh`
//! where `event ∈ {setup, sessionstart, cwdchanged, filechanged}`.
//!
//! Sorted by event priority (`setup` < `sessionstart` < `cwdchanged` <
//! `filechanged`), then by hook index — deterministic source order so a
//! later hook can override an earlier one.

use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::RwLock;

use regex::Regex;

/// Filename regex: `(setup|sessionstart|cwdchanged|filechanged)-hook-<idx>.sh`.
///
/// Capture 1 = event name, capture 2 = numeric index.
pub static HOOK_ENV_FILE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"^(setup|sessionstart|cwdchanged|filechanged)-hook-(\d+)\.sh$")
});

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(re) => re,
        Err(err) => panic!("invalid regex `{pattern}`: {err}"),
    }
}

const HOOK_PRIORITY: &[(&str, u8)] = &[
    ("setup", 0),
    ("sessionstart", 1),
    ("cwdchanged", 2),
    ("filechanged", 3),
];

fn event_priority(event: &str) -> u8 {
    HOOK_PRIORITY
        .iter()
        .find_map(|(name, p)| (name == &event).then_some(*p))
        .unwrap_or(99)
}

/// Compute the session-env directory for a given session under `coco_home`.
///
/// Path: `<coco_home>/session-env/<session_id>`.
pub fn session_env_dir(coco_home: &Path, session_id: &str) -> PathBuf {
    coco_home.join("session-env").join(session_id)
}

/// Reader + small cache for the concatenated hook env script.
///
/// Recompute is cheap but the script is sourced on every bash command,
/// so we avoid the dir walk in the hot path.
/// Call [`invalidate`](Self::invalidate) when the source files may
/// have changed (e.g. a fresh hook event just ran).
#[derive(Debug)]
pub struct SessionEnvReader {
    dir: PathBuf,
    cache: RwLock<CacheState>,
}

#[derive(Debug, Default)]
enum CacheState {
    #[default]
    Unloaded,
    Loaded(Option<String>),
}

impl SessionEnvReader {
    pub fn new(coco_home: &Path, session_id: &str) -> Self {
        Self {
            dir: session_env_dir(coco_home, session_id),
            cache: RwLock::new(CacheState::Unloaded),
        }
    }

    /// Directory the reader watches.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Concatenated hook env script, or `None` if no hook files exist.
    ///
    /// Honours the cache; call [`Self::invalidate`] to force a re-read.
    pub fn script(&self) -> Option<String> {
        // Fast path — cache hit.
        if let Ok(guard) = self.cache.read()
            && let CacheState::Loaded(ref out) = *guard
        {
            return out.clone();
        }
        // Slow path — read from disk and store.
        let computed = self.read_from_disk();
        if let Ok(mut guard) = self.cache.write() {
            *guard = CacheState::Loaded(computed.clone());
        }
        computed
    }

    /// Drop any cached script so the next [`Self::script`] call re-reads disk.
    pub fn invalidate(&self) {
        if let Ok(mut guard) = self.cache.write() {
            *guard = CacheState::Unloaded;
        }
    }

    fn read_from_disk(&self) -> Option<String> {
        let read_dir = std::fs::read_dir(&self.dir).ok()?;
        let regex: &Regex = &HOOK_ENV_FILE_REGEX;
        let mut entries: Vec<(u8, u32, PathBuf)> = Vec::new();
        for entry in read_dir.flatten() {
            let name = entry.file_name();
            let Some(name_str) = name.to_str() else {
                continue;
            };
            let Some(caps) = regex.captures(name_str) else {
                continue;
            };
            // `Captures::get(N)` returns `Some(_)` whenever the overall
            // pattern matches and the Nth group is on the matched path
            // — both groups in our pattern are unconditional, so `let
            // Some(...) =` falls through on the "this shouldn't happen"
            // branch instead of `unwrap` / `expect`.
            let (Some(event_m), Some(idx_m)) = (caps.get(1), caps.get(2)) else {
                continue;
            };
            let idx = idx_m.as_str().parse::<u32>().unwrap_or(u32::MAX);
            entries.push((event_priority(event_m.as_str()), idx, entry.path()));
        }
        if entries.is_empty() {
            return None;
        }
        entries.sort_by_key(|(p, i, _)| (*p, *i));

        let mut script = String::new();
        for (_, _, path) in entries {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    if !script.is_empty() {
                        script.push('\n');
                    }
                    script.push_str(trimmed);
                }
            }
        }
        if script.is_empty() {
            None
        } else {
            Some(script)
        }
    }
}

#[cfg(test)]
#[path = "hook_files.test.rs"]
mod tests;
