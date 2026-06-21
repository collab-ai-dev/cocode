//! Plan mode file management: slug generation, CRUD, recovery.
//!
//! Plans are stored as markdown files at `~/.coco/plans/{slug}.md` where
//! the slug is a random `{adjective}-{verb}-{noun}` word combination.
//! Each session gets a unique slug cached for its lifetime.
//!
//! This module also hosts the per-turn system-reminder renderers that
//! keep the model aware it is in plan mode and must stay read-only.

use coco_types::SubagentType;
use coco_types::ToolName;
use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

use crate::attachment::Phase4Variant;
use crate::attachment::PlanModeAttachment;
use crate::attachment::PlanModeExitAttachment;
use crate::attachment::PlanWorkflow;
use crate::attachment::ReminderType;

/// Maximum retries when a generated slug collides with an existing file.
const MAX_SLUG_RETRIES: i32 = 10;

// ── Word lists ──

const ADJECTIVES: &[&str] = &[
    "abundant",
    "ancient",
    "bright",
    "calm",
    "cheerful",
    "clever",
    "cozy",
    "curious",
    "dapper",
    "dazzling",
    "deep",
    "delightful",
    "eager",
    "elegant",
    "enchanted",
    "fancy",
    "fluffy",
    "gentle",
    "gleaming",
    "golden",
    "graceful",
    "happy",
    "hidden",
    "humble",
    "jolly",
    "joyful",
    "keen",
    "kind",
    "lively",
    "lovely",
    "lucky",
    "luminous",
    "magical",
    "majestic",
    "mellow",
    "merry",
    "mighty",
    "misty",
    "noble",
    "peaceful",
    "playful",
    "polished",
    "precious",
    "proud",
    "quiet",
    "quirky",
    "radiant",
    "rosy",
    "serene",
    "shiny",
    "silly",
    "sleepy",
    "smooth",
    "snazzy",
    "snug",
    "soft",
    "sparkling",
    "spicy",
    "splendid",
    "sprightly",
    "starry",
    "steady",
    "sunny",
    "swift",
    "tender",
    "tidy",
    "toasty",
    "tranquil",
    "valiant",
    "vast",
    "velvet",
    "vivid",
    "warm",
    "whimsical",
    "wild",
    "wise",
    "witty",
    "wondrous",
    "zany",
    "zesty",
    "zippy",
    "breezy",
    "bubbly",
    "cosmic",
    "crispy",
    "dreamy",
    "ethereal",
    "fizzy",
    "fuzzy",
    "glimmering",
    "glowing",
    "groovy",
    "iridescent",
    "jazzy",
    "melodic",
    "moonlit",
    "mossy",
    "peppy",
    "shimmering",
    "squishy",
    "velvety",
    // Programming concepts
    "abstract",
    "agile",
    "async",
    "atomic",
    "cached",
    "compiled",
    "concurrent",
    "dynamic",
    "functional",
    "generic",
    "hashed",
    "idempotent",
    "immutable",
    "iterative",
    "lazy",
    "linked",
    "modular",
    "optimized",
    "parallel",
    "pure",
    "reactive",
    "recursive",
    "resilient",
    "robust",
    "scalable",
    "sorted",
    "stateless",
    "streamed",
    "structured",
    "typed",
    "unified",
    "validated",
];

const VERBS: &[&str] = &[
    "baking",
    "beaming",
    "bouncing",
    "brewing",
    "bubbling",
    "chasing",
    "conjuring",
    "crafting",
    "crunching",
    "dancing",
    "discovering",
    "doodling",
    "dreaming",
    "drifting",
    "enchanting",
    "exploring",
    "floating",
    "foraging",
    "forging",
    "frolicking",
    "gathering",
    "giggling",
    "gliding",
    "growing",
    "hatching",
    "hopping",
    "hugging",
    "humming",
    "imagining",
    "inventing",
    "jingling",
    "juggling",
    "jumping",
    "kindling",
    "knitting",
    "launching",
    "leaping",
    "mapping",
    "meandering",
    "mixing",
    "munching",
    "napping",
    "nibbling",
    "noodling",
    "orbiting",
    "painting",
    "pondering",
    "popping",
    "prancing",
    "purring",
    "puzzling",
    "questing",
    "roaming",
    "rolling",
    "scribbling",
    "seeking",
    "singing",
    "skipping",
    "snuggling",
    "soaring",
    "sparking",
    "spinning",
    "splashing",
    "sprouting",
    "stargazing",
    "stirring",
    "strolling",
    "swimming",
    "tickling",
    "tinkering",
    "toasting",
    "tumbling",
    "twirling",
    "waddling",
    "wandering",
    "weaving",
    "whistling",
    "wiggling",
    "wishing",
    "wondering",
    "zooming",
];

const NOUNS: &[&str] = &[
    // Nature & cosmic
    "aurora",
    "blossom",
    "breeze",
    "brook",
    "bubble",
    "canyon",
    "cascade",
    "cloud",
    "comet",
    "coral",
    "cosmos",
    "crescent",
    "crystal",
    "dawn",
    "dewdrop",
    "eclipse",
    "ember",
    "feather",
    "fern",
    "firefly",
    "flame",
    "forest",
    "frost",
    "galaxy",
    "garden",
    "glacier",
    "glade",
    "grove",
    "harbor",
    "horizon",
    "island",
    "lagoon",
    "lake",
    "leaf",
    "meadow",
    "meteor",
    "mist",
    "moon",
    "moonbeam",
    "mountain",
    "nebula",
    "nova",
    "ocean",
    "orbit",
    "petal",
    "pine",
    "planet",
    "pond",
    "quasar",
    "rain",
    "rainbow",
    "reef",
    "ripple",
    "river",
    "shore",
    "sky",
    "snowflake",
    "spark",
    "spring",
    "star",
    "stardust",
    "starlight",
    "storm",
    "stream",
    "summit",
    "sun",
    "sunrise",
    "sunset",
    "thunder",
    "tide",
    "twilight",
    "valley",
    "waterfall",
    "wave",
    "willow",
    "wind",
    // Cute creatures
    "alpaca",
    "axolotl",
    "badger",
    "bear",
    "beaver",
    "bee",
    "bunny",
    "cat",
    "chipmunk",
    "crane",
    "deer",
    "dolphin",
    "dragon",
    "dragonfly",
    "duckling",
    "eagle",
    "elephant",
    "falcon",
    "finch",
    "flamingo",
    "fox",
    "frog",
    "hedgehog",
    "hummingbird",
    "jellyfish",
    "kitten",
    "koala",
    "ladybug",
    "lemur",
    "llama",
    "lynx",
    "narwhal",
    "newt",
    "octopus",
    "otter",
    "owl",
    "panda",
    "parrot",
    "peacock",
    "penguin",
    "phoenix",
    "platypus",
    "puffin",
    "puppy",
    "quail",
    "quokka",
    "rabbit",
    "raccoon",
    "raven",
    "seahorse",
    "seal",
    "sloth",
    "snail",
    "sparrow",
    "squirrel",
    "starfish",
    "swan",
    "tiger",
    "toucan",
    "turtle",
    "unicorn",
    "whale",
    "wolf",
    "wombat",
    // Fun objects
    "acorn",
    "anchor",
    "balloon",
    "beacon",
    "biscuit",
    "cake",
    "candle",
    "castle",
    "clock",
    "cookie",
    "crayon",
    "crown",
    "cupcake",
    "donut",
    "dream",
    "flask",
    "flute",
    "fountain",
    "gem",
    "globe",
    "goblet",
    "hammock",
    "harp",
    "haven",
    "hearth",
    "honey",
    "journal",
    "kettle",
    "key",
    "kite",
    "lantern",
    "lighthouse",
    "locket",
    "melody",
    "mitten",
    "muffin",
    "nest",
    "noodle",
    "oasis",
    "origami",
    "pancake",
    "peach",
    "pearl",
    "pie",
    "pillow",
    "pixel",
    "popcorn",
    "pretzel",
    "prism",
    "pudding",
    "pumpkin",
    "puzzle",
    "quill",
    "quilt",
    "riddle",
    "rocket",
    "rose",
    "scroll",
    "shell",
    "sketch",
    "sonnet",
    "sparkle",
    "sprout",
    "sundae",
    "taco",
    "teacup",
    "teapot",
    "toast",
    "token",
    "tower",
    "treasure",
    "trinket",
    "truffle",
    "tulip",
    "umbrella",
    "waffle",
    "wand",
    "whisper",
    "widget",
    "zephyr",
];

// ── Slug generation ──

/// Generate a random word slug in the format `adjective-verb-noun`.
///
/// Example: "gleaming-brewing-phoenix", "cosmic-pondering-lighthouse"
pub fn generate_word_slug() -> String {
    use std::hash::BuildHasher;
    use std::hash::RandomState;

    let hasher = RandomState::new();
    let h = hasher.hash_one(std::time::Instant::now());

    let adj = ADJECTIVES[(h as usize) % ADJECTIVES.len()];
    let verb = VERBS[((h >> 16) as usize) % VERBS.len()];
    let noun = NOUNS[((h >> 32) as usize) % NOUNS.len()];

    format!("{adj}-{verb}-{noun}")
}

// ── Slug cache (per session) ──

fn slug_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get or generate a plan slug for the given session.
///
/// The slug is generated lazily on first access and cached for the session.
/// If a plan file with the generated slug already exists, retries up to 10 times.
pub fn get_plan_slug(session_id: &str, plans_dir: &Path) -> String {
    if let Ok(cache) = slug_cache().lock()
        && let Some(slug) = cache.get(session_id)
    {
        return slug.clone();
    }

    let mut slug = String::new();
    for _ in 0..MAX_SLUG_RETRIES {
        slug = generate_word_slug();
        let file_path = plans_dir.join(format!("{slug}.md"));
        if !file_path.exists() {
            break;
        }
    }

    if let Ok(mut cache) = slug_cache().lock() {
        cache.insert(session_id.to_string(), slug.clone());
    }

    slug
}

/// Set a specific plan slug for a session (used when resuming).
pub fn set_plan_slug(session_id: &str, slug: &str) {
    if let Ok(mut cache) = slug_cache().lock() {
        cache.insert(session_id.to_string(), slug.to_string());
    }
}

/// Clear the plan slug for a session (called on /clear).
pub fn clear_plan_slug(session_id: &str) {
    if let Ok(mut cache) = slug_cache().lock() {
        cache.remove(session_id);
    }
}

/// Clear all plan slug entries (all sessions).
pub fn clear_all_plan_slugs() {
    if let Ok(mut cache) = slug_cache().lock() {
        cache.clear();
    }
}

// ── Plans directory ──

/// Resolve the plans directory from settings or default.
///
/// Default: `~/.coco/plans/`. If `plans_directory` setting is set,
/// resolves it relative to the project root and validates it stays within.
pub fn resolve_plans_directory(
    config_dir: &Path,
    project_dir: Option<&Path>,
    plans_directory_setting: Option<&str>,
) -> PathBuf {
    if let Some(setting) = plans_directory_setting
        && let Some(proj) = project_dir
    {
        let project = proj
            .canonicalize()
            .unwrap_or_else(|_| normalize_path_lexically(proj));
        let setting_path = Path::new(setting);
        let resolved = if setting_path.is_absolute() {
            setting_path.to_path_buf()
        } else {
            project.join(setting_path)
        };
        let plans = resolved
            .canonicalize()
            .unwrap_or_else(|_| normalize_path_lexically(&resolved));
        // Absolute settings and `..` segments are allowed only when their
        // normalized target remains inside the project root.
        if plans == project || plans.starts_with(&project) {
            return plans;
        }
        tracing::warn!("plansDirectory must be within project root: {setting}, using default");
    }

    config_dir.join("plans")
}

fn normalize_path_lexically(path: &Path) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

// ── Plan file CRUD ──

/// Get the file path for a session's plan.
///
/// Main conversation: `{slug}.md`
/// Subagents: `{slug}-agent-{agent_id}.md`
pub fn get_plan_file_path(session_id: &str, plans_dir: &Path, agent_id: Option<&str>) -> PathBuf {
    let slug = get_plan_slug(session_id, plans_dir);
    match agent_id {
        None => plans_dir.join(format!("{slug}.md")),
        Some(id) => plans_dir.join(format!("{slug}-agent-{id}.md")),
    }
}

/// Read the plan content for a session. Returns `None` if no plan exists.
pub fn get_plan(session_id: &str, plans_dir: &Path, agent_id: Option<&str>) -> Option<String> {
    let file_path = get_plan_file_path(session_id, plans_dir, agent_id);
    std::fs::read_to_string(file_path).ok()
}

/// Write plan content for a session.
pub fn write_plan(
    session_id: &str,
    plans_dir: &Path,
    content: &str,
    agent_id: Option<&str>,
) -> crate::Result<()> {
    std::fs::create_dir_all(plans_dir)?;
    let file_path = get_plan_file_path(session_id, plans_dir, agent_id);
    std::fs::write(&file_path, content)?;
    Ok(())
}

/// Delete a plan file for a session.
pub fn delete_plan(
    session_id: &str,
    plans_dir: &Path,
    agent_id: Option<&str>,
) -> crate::Result<()> {
    let file_path = get_plan_file_path(session_id, plans_dir, agent_id);
    if file_path.exists() {
        std::fs::remove_file(file_path)?;
    }
    Ok(())
}

/// Check if a plan file exists for a session.
pub fn plan_exists(session_id: &str, plans_dir: &Path, agent_id: Option<&str>) -> bool {
    get_plan_file_path(session_id, plans_dir, agent_id).exists()
}

// ── Plan recovery (for resume) ──

/// Recovery sources for plan content, tried in priority order:
/// 1. File snapshot from transcript
/// 2. ExitPlanMode tool_use input
/// 3. planContent field on user messages
/// 4. plan_file_reference attachment
pub fn recover_plan_for_resume(
    session_id: &str,
    plans_dir: &Path,
    slug: &str,
    transcript_entries: &[serde_json::Value],
) -> bool {
    // Set the slug for the resumed session
    set_plan_slug(session_id, slug);

    let plan_path = plans_dir.join(format!("{slug}.md"));

    // If file already exists, nothing to recover
    if plan_path.exists() {
        return true;
    }

    // Try to recover content from transcript entries (search backwards)
    if let Some(content) = recover_plan_from_messages(transcript_entries) {
        std::fs::create_dir_all(plans_dir).ok();
        if std::fs::write(&plan_path, content).is_ok() {
            return true;
        }
    }

    false
}

/// Recover plan content from message history.
///
/// Searches backwards through transcript entries for plan content in:
/// 1. most recent `file_snapshot` system message with key `plan`
/// 2. ExitPlanMode tool_use input (plan field)
/// 3. planContent field on user messages
/// 4. plan_file_reference attachments
fn recover_plan_from_messages(entries: &[serde_json::Value]) -> Option<String> {
    for entry in entries.iter().rev() {
        if entry.get("type").and_then(|v| v.as_str()) == Some("system")
            && entry.get("subtype").and_then(|v| v.as_str()) == Some("file_snapshot")
        {
            let files = entry
                .get("snapshotFiles")
                .or_else(|| entry.get("snapshot_files"))
                .and_then(|v| v.as_array());
            if let Some(files) = files {
                for file in files {
                    if file.get("key").and_then(|v| v.as_str()) == Some("plan")
                        && let Some(plan) = file.get("content").and_then(|v| v.as_str())
                        && !plan.is_empty()
                    {
                        return Some(plan.to_string());
                    }
                }
            }
        }
    }

    for entry in entries.iter().rev() {
        // Check for ExitPlanMode tool_use input
        let assistant_content = if entry.get("role").and_then(|v| v.as_str()) == Some("assistant") {
            entry.get("content").and_then(|v| v.as_array())
        } else if entry.get("type").and_then(|v| v.as_str()) == Some("assistant") {
            entry
                .get("message")
                .and_then(|v| v.get("content"))
                .and_then(|v| v.as_array())
        } else {
            None
        };
        if let Some(content) = assistant_content {
            for block in content {
                if block.get("type").and_then(|v| v.as_str()) == Some("tool_use")
                    && block.get("name").and_then(|v| v.as_str())
                        == Some(ToolName::ExitPlanMode.as_str())
                    && let Some(plan) = block
                        .get("input")
                        .and_then(|v| v.get("plan"))
                        .and_then(|v| v.as_str())
                    && !plan.is_empty()
                {
                    return Some(plan.to_string());
                }
            }
        }

        // Check for planContent on user messages
        if entry.get("role").and_then(|v| v.as_str()) == Some("user")
            && let Some(plan) = entry.get("planContent").and_then(|v| v.as_str())
            && !plan.is_empty()
        {
            return Some(plan.to_string());
        }

        // Check for plan_file_reference attachment
        if let Some(attachment) = entry.get("attachment")
            && attachment.get("type").and_then(|v| v.as_str()) == Some("plan_file_reference")
            && let Some(plan) = attachment.get("planContent").and_then(|v| v.as_str())
            && !plan.is_empty()
        {
            return Some(plan.to_string());
        }
    }
    None
}

/// Copy a plan file for a forked session. Generates a NEW slug for the fork
/// to prevent the original and forked sessions from clobbering each other.
pub fn copy_plan_for_fork(
    source_session_id: &str,
    target_session_id: &str,
    plans_dir: &Path,
) -> bool {
    let source_path = get_plan_file_path(source_session_id, plans_dir, None);
    if !source_path.exists() {
        return false;
    }
    let target_path = get_plan_file_path(target_session_id, plans_dir, None);
    std::fs::copy(source_path, target_path).is_ok()
}

/// Outcome of verifying that the plan file was actually edited during
/// plan mode. Retained as a standalone helper for callers that want a
/// local plan-file edit check; the TS-style `VerifyPlanExecution` flow
/// does not use this mtime advisory.
///
/// "Skipped" is a caller-side concept (no entry timestamp, verification
/// disabled) and is modeled as `Option<PlanVerificationOutcome>::None`
/// at call sites rather than as a variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanVerificationOutcome {
    /// Plan file exists and was modified after `plan_mode_entry_ms`.
    Edited,
    /// Plan file exists but its mtime is earlier than the entry time —
    /// the model likely called ExitPlanMode without touching it.
    NotEdited,
    /// Plan file doesn't exist — the model called ExitPlanMode without
    /// ever creating a plan. Also a soft failure.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitPlanModeDerivedState {
    pub outcome: coco_types::ExitPlanModeOutcome,
    pub plan: Option<String>,
    pub plan_file_path: Option<String>,
    pub plan_was_edited: bool,
}

/// Derive the semantic ExitPlanMode state from the current plan file and
/// optional user-edited approval text. This is the single source of truth for
/// whether an exit carries an implementation plan.
pub fn derive_exit_plan_mode_state(
    session_id: Option<&str>,
    plans_dir: Option<&Path>,
    agent_id: Option<&str>,
    plan_mode_entry_ms: Option<i64>,
    edited_plan: Option<String>,
) -> ExitPlanModeDerivedState {
    let plan_file_path = match (session_id, plans_dir) {
        (Some(session_id), Some(plans_dir)) => {
            Some(get_plan_file_path(session_id, plans_dir, agent_id))
        }
        _ => None,
    };
    let plan_file_path_string = plan_file_path
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned());

    let Some(path) = plan_file_path.as_deref() else {
        return ExitPlanModeDerivedState {
            outcome: coco_types::ExitPlanModeOutcome::NoImplementationPlan,
            plan: None,
            plan_file_path: None,
            plan_was_edited: false,
        };
    };
    if let Some(plan) = edited_plan
        && !plan.trim().is_empty()
    {
        return ExitPlanModeDerivedState {
            outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
            plan: Some(plan),
            plan_file_path: plan_file_path_string,
            plan_was_edited: true,
        };
    }

    let verification = plan_mode_entry_ms
        .and_then(|entry_ms| verify_plan_was_edited(path, entry_ms))
        .unwrap_or(PlanVerificationOutcome::Missing);
    if verification != PlanVerificationOutcome::Edited {
        return ExitPlanModeDerivedState {
            outcome: coco_types::ExitPlanModeOutcome::NoImplementationPlan,
            plan: None,
            plan_file_path: None,
            plan_was_edited: false,
        };
    }

    let plan = match (session_id, plans_dir) {
        (Some(session_id), Some(plans_dir)) => get_plan(session_id, plans_dir, agent_id),
        _ => None,
    };
    if plan.as_deref().is_some_and(|plan| !plan.trim().is_empty()) {
        ExitPlanModeDerivedState {
            outcome: coco_types::ExitPlanModeOutcome::ImplementationPlan,
            plan,
            plan_file_path: plan_file_path_string,
            plan_was_edited: false,
        }
    } else {
        ExitPlanModeDerivedState {
            outcome: coco_types::ExitPlanModeOutcome::NoImplementationPlan,
            plan: None,
            plan_file_path: None,
            plan_was_edited: false,
        }
    }
}

/// Check whether the plan file was edited between EnterPlanMode and
/// the current call. Soft-failure: ExitPlanMode surfaces a warning; it
/// does NOT block approval.
///
/// - `plan_file`: fully-resolved path (as computed from slug + session).
/// - `entry_ms`: Unix-epoch-ms of EnterPlanMode, from app_state.
///
/// Returns `None` when `entry_ms <= 0` — the caller passed a
/// missing-or-zero timestamp, so there's no comparison baseline and
/// we can't produce a meaningful outcome. Production callers should
/// guard on `entry_ms > 0` before invoking; this fallback keeps the
/// API total for defensive code paths.
pub fn verify_plan_was_edited(plan_file: &Path, entry_ms: i64) -> Option<PlanVerificationOutcome> {
    if entry_ms <= 0 {
        return None;
    }
    let Ok(meta) = std::fs::metadata(plan_file) else {
        return Some(PlanVerificationOutcome::Missing);
    };
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    if mtime_ms >= entry_ms {
        Some(PlanVerificationOutcome::Edited)
    } else {
        Some(PlanVerificationOutcome::NotEdited)
    }
}

/// Delete the main plan file AND all subagent plan files for a session.
///
/// Called by `/clear all` to clean up after the session. Glob-matches
/// `{slug}.md` + `{slug}-agent-*.md` in the plans directory. Idempotent;
/// missing files silently ignored.
///
/// Returns the number of files actually removed.
pub fn delete_all_session_plan_files(session_id: &str, plans_dir: &Path) -> usize {
    let Ok(slug_cache) = slug_cache().lock() else {
        return 0;
    };
    let Some(slug) = slug_cache.get(session_id) else {
        return 0; // No slug cached = no plan file created yet
    };
    let slug = slug.clone();
    drop(slug_cache);

    let mut removed = 0;
    // Main plan file
    let main_path = plans_dir.join(format!("{slug}.md"));
    if main_path.exists() && std::fs::remove_file(&main_path).is_ok() {
        removed += 1;
    }
    // Subagent plan files: `{slug}-agent-*.md`
    let prefix = format!("{slug}-agent-");
    let Ok(entries) = std::fs::read_dir(plans_dir) else {
        return removed;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with(&prefix) && name.ends_with(".md") && std::fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

// ── System-reminder rendering ──

/// Render the plan-mode system-reminder text (unwrapped).
///
/// Each `(reminder_type, workflow, is_sub_agent)` combination maps to one
/// output variant:
/// - Any + sub-agent              → sub-agent instructions
///   (sub-agents always get the sub-agent variant regardless of cadence)
/// - Full + FivePhase + main-agent → main-agent 5-phase instructions;
///   Phase-4 block chosen by `attachment.phase4_variant`
/// - Full + Interview + main-agent → interview instructions
/// - Sparse + main-agent          → sparse instructions
/// - Reentry + main-agent         → re-entry reminder
///
/// Callers wrap the return value in a `<system-reminder>` XML tag.
pub fn render_plan_mode_reminder(attachment: &PlanModeAttachment) -> String {
    let ask_user_question = ToolName::AskUserQuestion.as_str();
    let exit_plan_mode = ToolName::ExitPlanMode.as_str();

    if attachment.is_sub_agent {
        return render_full_sub_agent(attachment, ask_user_question);
    }
    match attachment.reminder_type {
        ReminderType::Sparse => render_sparse(attachment, ask_user_question, exit_plan_mode),
        ReminderType::Reentry => render_reentry(attachment, exit_plan_mode),
        ReminderType::Full => match effective_workflow(attachment) {
            PlanWorkflow::FivePhase => {
                render_full_five_phase(attachment, ask_user_question, exit_plan_mode)
            }
            PlanWorkflow::Interview => {
                render_full_interview(attachment, ask_user_question, exit_plan_mode)
            }
        },
    }
}

fn effective_workflow(attachment: &PlanModeAttachment) -> PlanWorkflow {
    // TS has no separate five-phase-without-agents prompt: the five-phase
    // reminder assumes built-in Explore/Plan agents are available. When a Rust
    // host disables those built-ins, use the TS interview workflow instead of
    // inventing a second five-phase variant that points at unavailable tools.
    if attachment.workflow == PlanWorkflow::FivePhase && !attachment.explore_plan_agents_available {
        PlanWorkflow::Interview
    } else {
        attachment.workflow
    }
}

fn plan_file_info(attachment: &PlanModeAttachment) -> String {
    plan_file_info_impl(attachment, /*sub_agent*/ false)
}

/// Sub-agent plan-file info carries an extra "if you need to" softener on
/// both branches, differentiating sub-agent prose from the 5-phase / interview
/// versions.
fn plan_file_info_sub_agent(attachment: &PlanModeAttachment) -> String {
    plan_file_info_impl(attachment, /*sub_agent*/ true)
}

fn plan_file_info_impl(attachment: &PlanModeAttachment, sub_agent: bool) -> String {
    // Model-aware: the builder resolved these from the model's tool set
    // (Claude → Write/Edit, gpt-5 → apply_patch). Don't hardcode `ToolName::`
    // here — that's what told gpt-5 to use a `Write` tool it doesn't have.
    let file_edit = attachment.edit_tool.as_str();
    let file_write = attachment.write_tool.as_str();
    let softener = if sub_agent { " if you need to" } else { "" };
    if attachment.plan_exists {
        format!(
            "A plan file already exists at {path}. You can read it and make incremental edits using the {file_edit} tool{softener}.",
            path = attachment.plan_file_path,
        )
    } else {
        format!(
            "No plan file exists yet. You should create your plan at {path} using the {file_write} tool{softener}.",
            path = attachment.plan_file_path,
        )
    }
}

fn exit_plan_mode_deferred_note(attachment: &PlanModeAttachment, exit_plan_mode: &str) -> String {
    if attachment
        .deferred_tools
        .iter()
        .any(|name| name == exit_plan_mode)
    {
        format!(
            " If {exit_plan_mode} is listed as deferred, first use ToolSearch with query \"select:{exit_plan_mode}\" to load it."
        )
    } else {
        String::new()
    }
}

fn render_sparse(
    attachment: &PlanModeAttachment,
    ask_user_question: &str,
    exit_plan_mode: &str,
) -> String {
    // Adapts per workflow: emit the matching hint based on `attachment.workflow`.
    let workflow_hint = match attachment.workflow {
        PlanWorkflow::Interview => {
            "Follow iterative workflow: explore codebase, interview user, write to plan incrementally."
        }
        PlanWorkflow::FivePhase => "Follow 5-phase workflow.",
    };
    format!(
        "Plan mode still active (see full instructions earlier in conversation). \
         Read-only except plan file ({path}). {workflow_hint} End turns with \
         {ask_user_question} (for clarifications) or {exit_plan_mode} (for plan \
         approval).{deferred_note} Never ask about plan approval via text or AskUserQuestion.",
        path = attachment.plan_file_path,
        deferred_note = exit_plan_mode_deferred_note(attachment, exit_plan_mode),
    )
}

fn render_reentry(attachment: &PlanModeAttachment, exit_plan_mode: &str) -> String {
    // The caller only reaches this function when a plan file exists (Reentry
    // is only emitted when an existing plan is present). Assume the invariant
    // rather than rendering a no-plan branch.
    let plan_file_path = &attachment.plan_file_path;
    format!(
        "## Re-entering Plan Mode\n\n\
         You are returning to plan mode after having previously exited it. \
         A plan file exists at {plan_file_path} from your previous planning session.\n\n\
         **Before proceeding with any new planning, you should:**\n\
         1. Read the existing plan file to understand what was previously planned\n\
         2. Evaluate the user's current request against that plan\n\
         3. Decide how to proceed:\n\
         \t- **Different task**: If the user's request is for a different task—even \
         if it's similar or related—start fresh by overwriting the existing plan\n\
         \t- **Same task, continuing**: If this is explicitly a continuation or \
         refinement of the exact same task, modify the existing plan while cleaning \
         up outdated or irrelevant sections\n\
         4. Continue on with the plan process and most importantly you should always \
         edit the plan file one way or the other before calling {exit_plan_mode}.{deferred_note}\n\n\
         Treat this as a fresh planning session. Do not assume the existing plan is \
         relevant without evaluating it first.",
        deferred_note = exit_plan_mode_deferred_note(attachment, exit_plan_mode),
    )
}

fn render_full_sub_agent(attachment: &PlanModeAttachment, ask_user_question: &str) -> String {
    format!(
        "Plan mode is active. The user indicated that they do not want you to \
         execute yet -- you MUST NOT make any edits, run any non-readonly tools \
         (including changing configs or making commits), or otherwise make any \
         changes to the system. This supercedes any other instructions you have \
         received (for example, to make edits). Instead, you should:\n\n\
         ## Plan File Info:\n{file_info}\n\
         You should build your plan incrementally by writing to or editing this \
         file. NOTE that this is the only file you are allowed to edit - other \
         than this you are only allowed to take READ-ONLY actions.\n\
         Answer the user's query comprehensively, using the {ask_user_question} \
         tool if you need to ask the user clarifying questions. If you do use \
         the {ask_user_question}, make sure to ask all clarifying questions \
         you need to fully understand the user's intent before proceeding.",
        file_info = plan_file_info_sub_agent(attachment),
    )
}

/// Full 5-phase workflow. The Phase-4 block is chosen by `phase4_variant`.
fn render_full_five_phase(
    attachment: &PlanModeAttachment,
    ask_user_question: &str,
    exit_plan_mode: &str,
) -> String {
    let explore_agent = SubagentType::Explore.as_str();
    let plan_agent = SubagentType::Plan.as_str();
    let n_explore = attachment.explore_agent_count;
    let n_plan = attachment.plan_agent_count;

    let agent_count_multiple = if n_plan > 1 {
        format!(
            "- **Multiple agents**: Use up to {n_plan} agents for complex tasks that benefit from different perspectives\n\n\
             Examples of when to use multiple agents:\n\
             - The task touches multiple parts of the codebase\n\
             - It's a large refactor or architectural change\n\
             - There are many edge cases to consider\n\
             - You'd benefit from exploring different approaches\n\n\
             Example perspectives by task type:\n\
             - New feature: simplicity vs performance vs maintainability\n\
             - Bug fix: root cause vs workaround vs prevention\n\
             - Refactoring: minimal change vs clean architecture\n\n"
        )
    } else {
        String::new()
    };

    let phase4 = render_phase4_block(attachment.phase4_variant);

    format!(
        "Plan mode is active. The user indicated that they do not want you to \
         execute yet -- you MUST NOT make any edits (with the exception of the \
         plan file mentioned below), run any non-readonly tools (including \
         changing configs or making commits), or otherwise make any changes to \
         the system. This supercedes any other instructions you have received.\n\n\
         ## Plan File Info:\n{file_info}\n\
         You should build your plan incrementally by writing to or editing this \
         file. NOTE that this is the only file you are allowed to edit - other \
         than this you are only allowed to take READ-ONLY actions.\n\n\
         ## Plan Workflow\n\n\
         ### Phase 1: Initial Understanding\n\
         Goal: Gain a comprehensive understanding of the user's request by reading \
         through code and asking them questions. Critical: In this phase you should \
         only use the {explore_agent} subagent type.\n\n\
         1. Focus on understanding the user's request and the code associated with \
         their request. Actively search for existing functions, utilities, and \
         patterns that can be reused — avoid proposing new code when suitable \
         implementations already exist.\n\n\
         2. **Launch up to {n_explore} {explore_agent} agents IN PARALLEL** (single \
         message, multiple tool calls) to efficiently explore the codebase.\n\
         \t- Use 1 agent when the task is isolated to known files, the user provided \
         specific file paths, or you're making a small targeted change.\n\
         \t- Use multiple agents when: the scope is uncertain, multiple areas of the \
         codebase are involved, or you need to understand existing patterns before \
         planning.\n\
         \t- Quality over quantity - {n_explore} agents maximum, but you should try \
         to use the minimum number of agents necessary (usually just 1)\n\
         \t- If using multiple agents: Provide each agent with a specific search \
         focus or area to explore. Example: One agent searches for existing \
         implementations, another explores related components, a third investigating \
         testing patterns\n\n\
         ### Phase 2: Design\n\
         Goal: Design an implementation approach.\n\n\
         Launch {plan_agent} agent(s) to design the implementation based on the \
         user's intent and your exploration results from Phase 1.\n\n\
         You can launch up to {n_plan} agent(s) in parallel.\n\n\
         **Guidelines:**\n\
         - **Default**: Launch at least 1 Plan agent for most tasks - it helps \
         validate your understanding and consider alternatives\n\
         - **Skip agents**: Only for truly trivial tasks (typo fixes, single-line \
         changes, simple renames)\n\
         {agent_count_multiple}\
         In the agent prompt:\n\
         - Provide comprehensive background context from Phase 1 exploration \
         including filenames and code path traces\n\
         - Describe requirements and constraints\n\
         - Request a detailed implementation plan\n\n\
         ### Phase 3: Review\n\
         Goal: Review the plan(s) from Phase 2 and ensure alignment with the user's \
         intentions.\n\
         1. Read the critical files identified by agents to deepen your understanding\n\
         2. Ensure that the plans align with the user's original request\n\
         3. Use {ask_user_question} to clarify any remaining questions with the user\n\n\
         {phase4}\n\
         ### Phase 5: Call {exit_plan_mode}\n\
         At the very end of your turn, once you have asked the user questions and \
         are happy with your final plan file - you should always call {exit_plan_mode} \
         to indicate to the user that you are done planning.\n\
         This is critical - your turn should only end with either using the \
         {ask_user_question} tool OR calling {exit_plan_mode}. Do not stop unless \
         it's for these 2 reasons\n\n\
         **Important:** Use {ask_user_question} ONLY to clarify requirements or \
         choose between approaches. Use {exit_plan_mode} to request plan approval. \
         Do NOT ask about plan approval in any other way - no text questions, no \
         AskUserQuestion. Phrases like \"Is this plan okay?\", \"Should I proceed?\", \
         \"How does this plan look?\", \"Any changes before we start?\", or similar \
         MUST use {exit_plan_mode}.{deferred_note}\n\n\
         NOTE: At any point in time through this workflow you should feel free to \
         ask the user questions or clarifications using the {ask_user_question} \
         tool. Don't make large assumptions about user intent. The goal is to present \
         a well researched plan to the user, and tie any loose ends before \
         implementation begins.",
        file_info = plan_file_info(attachment),
        deferred_note = exit_plan_mode_deferred_note(attachment, exit_plan_mode),
    )
}

/// Four arms of the pewter-ledger experiment, exposed as a user-controllable
/// setting.
fn render_phase4_block(variant: Phase4Variant) -> String {
    match variant {
        Phase4Variant::Standard => "### Phase 4: Final Plan\n\
             Goal: Write your final plan to the plan file (the only file you can edit).\n\
             - Begin with a **Context** section: explain why this change is being made \
             — the problem or need it addresses, what prompted it, and the intended \
             outcome\n\
             - Include only your recommended approach, not all alternatives\n\
             - Ensure that the plan file is concise enough to scan quickly, but \
             detailed enough to execute effectively\n\
             - Include the paths of critical files to be modified\n\
             - Reference existing functions and utilities you found that should be \
             reused, with their file paths\n\
             - Include a verification section describing how to test the changes \
             end-to-end (run the code, use MCP tools, run tests)\n"
            .to_string(),
        Phase4Variant::Trim => "### Phase 4: Final Plan\n\
             Goal: Write your final plan to the plan file (the only file you can edit).\n\
             - One-line **Context**: what is being changed and why\n\
             - Include only your recommended approach, not all alternatives\n\
             - List the paths of files to be modified\n\
             - Reference existing functions and utilities to reuse, with their file \
             paths\n\
             - End with **Verification**: the single command to run to confirm the \
             change works (no numbered test procedures)\n"
            .to_string(),
        Phase4Variant::Cut => "### Phase 4: Final Plan\n\
             Goal: Write your final plan to the plan file (the only file you can edit).\n\
             - Do NOT write a Context or Background section. The user just told you \
             what they want.\n\
             - List the paths of files to be modified and what changes in each (one \
             line per file)\n\
             - Reference existing functions and utilities to reuse, with their file \
             paths\n\
             - End with **Verification**: the single command that confirms the change \
             works\n\
             - Most good plans are under 40 lines. Prose is a sign you are padding.\n"
            .to_string(),
        Phase4Variant::Cap => "### Phase 4: Final Plan\n\
             Goal: Write your final plan to the plan file (the only file you can edit).\n\
             - Do NOT write a Context, Background, or Overview section. The user just \
             told you what they want.\n\
             - Do NOT restate the user's request. Do NOT write prose paragraphs.\n\
             - List the paths of files to be modified and what changes in each (one \
             bullet per file)\n\
             - Reference existing functions to reuse, with file:line\n\
             - End with the single verification command\n\
             - **Hard limit: 40 lines.** If the plan is longer, delete prose — not \
             file paths.\n"
            .to_string(),
    }
}

/// Iterative ask-as-you-go workflow.
fn render_full_interview(
    attachment: &PlanModeAttachment,
    ask_user_question: &str,
    exit_plan_mode: &str,
) -> String {
    let explore_agent_sentence = if attachment.explore_plan_agents_available {
        format!(
            " You can use the `{}` agent type to parallelize complex \
             searches without filling your context, though for straightforward \
             queries direct tools are simpler.",
            SubagentType::Explore.as_str()
        )
    } else {
        String::new()
    };
    format!(
        "Plan mode is active. The user indicated that they do not want you to \
         execute yet -- you MUST NOT make any edits (with the exception of the \
         plan file mentioned below), run any non-readonly tools (including \
         changing configs or making commits), or otherwise make any changes to \
         the system. This supercedes any other instructions you have received.\n\n\
         ## Plan File Info:\n{file_info}\n\n\
         ## Iterative Planning Workflow\n\n\
         You are pair-planning with the user. Explore the code to build context, \
         ask the user questions when you hit decisions you can't make alone, and \
         write your findings into the plan file as you go. The plan file (above) \
         is the ONLY file you may edit — it starts as a rough skeleton and gradually \
         becomes the final plan.\n\n\
         ### The Loop\n\n\
         Repeat this cycle until the plan is complete:\n\n\
         1. **Explore** — Use Read, Glob, Grep, LSP, and other read-only tools \
         to read code. Look for existing functions, utilities, and patterns to \
         reuse.{explore_agent_sentence}\n\
         2. **Update the plan file** — After each discovery, immediately capture \
         what you learned. Don't wait until the end.\n\
         3. **Ask the user** — When you hit an ambiguity or decision you can't \
         resolve from code alone, use {ask_user_question}. Then go back to step 1.\n\n\
         ### First Turn\n\n\
         Start by quickly scanning a few key files to form an initial understanding \
         of the task scope. Then write a skeleton plan (headers and rough notes) \
         and ask the user your first round of questions. Don't explore exhaustively \
         before engaging the user.\n\n\
         ### Asking Good Questions\n\n\
         - Never ask what you could find out by reading the code\n\
         - Batch related questions together (use multi-question {ask_user_question} calls)\n\
         - Focus on things only the user can answer: requirements, preferences, \
         tradeoffs, edge case priorities\n\
         - Scale depth to the task — a vague feature request needs many rounds; a \
         focused bug fix may need one or none\n\n\
         ### Plan File Structure\n\
         Your plan file should be divided into clear sections using markdown headers, \
         based on the request. Fill out these sections as you go.\n\
         - Begin with a **Context** section: explain why this change is being made — \
         the problem or need it addresses, what prompted it, and the intended outcome\n\
         - Include only your recommended approach, not all alternatives\n\
         - Ensure that the plan file is concise enough to scan quickly, but detailed \
         enough to execute effectively\n\
         - Include the paths of critical files to be modified\n\
         - Reference existing functions and utilities you found that should be reused, \
         with their file paths\n\
         - Include a verification section describing how to test the changes \
         end-to-end (run the code, use MCP tools, run tests)\n\n\
         ### When to Converge\n\n\
         Your plan is ready when you've addressed all ambiguities and it covers: what \
         to change, which files to modify, what existing code to reuse (with file \
         paths), and how to verify the changes. Call {exit_plan_mode} when the plan \
         is ready for approval.\n\n\
         ### Ending Your Turn\n\n\
         Your turn should only end by either:\n\
         - Using {ask_user_question} to gather more information\n\
         - Calling {exit_plan_mode} when the plan is ready for approval\n\n\
         **Important:** Use {exit_plan_mode} to request plan approval. Do NOT ask \
         about plan approval via text or AskUserQuestion.{deferred_note}",
        file_info = plan_file_info(attachment),
        deferred_note = exit_plan_mode_deferred_note(attachment, exit_plan_mode),
    )
}

/// Render the plan-mode-exit system-reminder text (unwrapped).
///
/// Emitted exactly once on the turn immediately after `ExitPlanMode` is
/// approved.
pub fn render_plan_mode_exit_reminder(attachment: &PlanModeExitAttachment) -> String {
    if attachment.outcome == Some(coco_types::ExitPlanModeOutcome::NoImplementationPlan) {
        return "## Exited Plan Mode\n\n\
                You have exited plan mode without an implementation plan. \
                Continue answering the user without starting implementation."
            .to_string();
    }

    let plan_reference = if attachment.plan_exists {
        format!(
            " The plan file is located at {path} if you need to reference it.",
            path = attachment.plan_file_path,
        )
    } else {
        String::new()
    };

    format!(
        "## Exited Plan Mode\n\n\
         You have exited plan mode. You can now make edits, run tools, and take \
         actions.{plan_reference}"
    )
}

/// Render the auto-mode-exit system-reminder text (unwrapped).
///
/// Emitted exactly once on the turn after Auto mode is left — either by
/// `ExitPlanMode` from a plan entered via Auto (when the restore mode isn't
/// Auto) or by an unannounced Auto→non-Auto mode cycle detected by the
/// reminder. The one-shot flag on app_state is cleared after emission.
pub fn render_auto_mode_exit_reminder() -> String {
    "## Exited Auto Mode\n\n\
     You have exited auto mode. The user may now want to interact more \
     directly. You should ask clarifying questions when the approach is \
     ambiguous rather than making assumptions."
        .to_string()
}

#[cfg(test)]
#[path = "plan_mode.test.rs"]
mod tests;
