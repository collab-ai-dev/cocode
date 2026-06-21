use super::*;
use crate::Cli;
use clap::Parser;

fn cli_with(args: &[&str]) -> Cli {
    let mut full: Vec<&str> = vec!["coco"];
    full.extend_from_slice(args);
    Cli::parse_from(full)
}

/// Synthetic cwd used by every test in this module. Any path works
/// — the resolver's worktree fallback collapses unknown paths
/// through `coco_paths::ProjectPaths` slugging.
const TEST_CWD: &str = "/test-cwd";

/// Resolve the on-disk project directory for `TEST_CWD` under
/// `memory_base` (= the tempdir root). Tests write fixture JSONLs
/// under this dir so the resolver finds them via the same path math
/// production code uses.
fn project_dir(memory_base: &std::path::Path) -> std::path::PathBuf {
    let paths =
        coco_paths::ProjectPaths::new(memory_base.to_path_buf(), std::path::Path::new(TEST_CWD));
    paths.project_dir()
}

fn write_minimal_session(memory_base: &std::path::Path, id: &str) {
    let dir = project_dir(memory_base);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{id}.jsonl"));
    let body = format!(
        "{}\n{}\n",
        serde_json::json!({
            "type": "user",
            "uuid": format!("{id}-u1"),
            "session_id": id,
            "timestamp": "2025-01-15T10:00:00Z",
            "message": {"role": "user", "content": [{"type": "text", "text": "hi"}]},
        }),
        serde_json::json!({
            "type": "assistant",
            "uuid": format!("{id}-a1"),
            "parent_uuid": format!("{id}-u1"),
            "session_id": id,
            "timestamp": "2025-01-15T10:00:01Z",
            "model": "claude-sonnet-4-6",
            "message": {"role": "assistant", "content": [{"type": "text", "text": "ack"}]},
        }),
    );
    std::fs::write(path, body).unwrap();
}

#[test]
fn no_flags_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli_with(&[]);
    let plan = resolve(&cli, dir.path(), std::path::Path::new(TEST_CWD)).expect("resolve runs");
    assert!(plan.is_none(), "no resume flags ⇒ no plan");
}

#[test]
fn resume_by_id_loads_messages() {
    let dir = tempfile::tempdir().unwrap();
    write_minimal_session(dir.path(), "alpha");

    let cli = cli_with(&["--resume", "alpha"]);
    let plan = resolve(&cli, dir.path(), std::path::Path::new(TEST_CWD))
        .expect("resolve runs")
        .expect("plan emitted");

    assert_eq!(plan.session_id, "alpha");
    assert_eq!(plan.source_session_id, "alpha");
    assert!(!plan.is_fork);
    assert_eq!(plan.prior_messages.len(), 2);
    assert_eq!(plan.conversation.turn_count, 1);
    assert_eq!(plan.destination_path, plan.source_path);
}

#[test]
fn resume_unknown_id_errors() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli_with(&["--resume", "nope"]);
    let err =
        resolve(&cli, dir.path(), std::path::Path::new(TEST_CWD)).expect_err("unknown id ⇒ error");
    assert!(err.to_string().contains("no session found"));
}

#[test]
fn continue_with_no_sessions_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let cli = cli_with(&["--continue"]);
    let plan = resolve(&cli, dir.path(), std::path::Path::new(TEST_CWD)).expect("resolve runs");
    assert!(plan.is_none(), "no sessions ⇒ fall through to fresh");
}

#[test]
fn continue_picks_most_recent() {
    let dir = tempfile::tempdir().unwrap();
    write_minimal_session(dir.path(), "older");
    // Bump mtime on the second file by writing it after a sleep so
    // the directory listing puts it first.
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_minimal_session(dir.path(), "newer");

    let cli = cli_with(&["--continue"]);
    let plan = resolve(&cli, dir.path(), std::path::Path::new(TEST_CWD))
        .expect("resolve runs")
        .expect("plan emitted");
    assert_eq!(plan.source_session_id, "newer");
}

#[test]
fn fork_creates_new_id_and_copies_jsonl() {
    let dir = tempfile::tempdir().unwrap();
    write_minimal_session(dir.path(), "src");

    let cli = cli_with(&["--resume", "src", "--fork-session"]);
    let plan = resolve(&cli, dir.path(), std::path::Path::new(TEST_CWD))
        .expect("resolve runs")
        .expect("plan emitted");

    assert!(plan.is_fork);
    assert_eq!(plan.source_session_id, "src");
    assert_ne!(plan.session_id, "src", "fork must mint a fresh id");
    assert_ne!(
        plan.destination_path, plan.source_path,
        "fork must write to a fresh path"
    );
    assert!(
        plan.destination_path.exists(),
        "fork copy must land on disk"
    );
    // The transcript copies verbatim except each entry's `session_id`,
    // which is relabeled to the freshly minted fork id so the fork owns
    // its own identity — the source id must not survive.
    let dest = std::fs::read_to_string(&plan.destination_path).unwrap();
    for line in dest.lines().filter(|l| !l.trim().is_empty()) {
        let entry: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            entry["session_id"], plan.session_id,
            "every entry must carry the fork id, not the source id"
        );
    }
    // Non-id content (per-entry uuids) is preserved verbatim.
    assert!(dest.contains("src-u1") && dest.contains("src-a1"));
}

#[test]
fn fork_honors_explicit_session_id() {
    let dir = tempfile::tempdir().unwrap();
    write_minimal_session(dir.path(), "src");

    let cli = cli_with(&[
        "--resume",
        "src",
        "--fork-session",
        "--session-id",
        "forked-1",
    ]);
    let plan = resolve(&cli, dir.path(), std::path::Path::new(TEST_CWD))
        .expect("resolve runs")
        .expect("plan emitted");
    assert_eq!(plan.session_id, "forked-1");
    assert!(
        plan.destination_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .starts_with("forked-1"),
    );
}
