use super::*;
use pretty_assertions::assert_eq;
use std::path::PathBuf;

fn paths() -> ProjectPaths {
    ProjectPaths::new(
        PathBuf::from("/home/u/.coco"),
        std::path::Path::new("/Users/foo/proj"),
    )
}

#[test]
fn projects_root_is_memory_base_join_projects() {
    assert_eq!(
        paths().projects_root(),
        PathBuf::from("/home/u/.coco/projects"),
    );
}

#[test]
fn project_dir_appends_slug() {
    assert_eq!(
        paths().project_dir(),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj"),
    );
}

#[test]
fn transcript_path() {
    assert_eq!(
        paths().transcript("sid-1"),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/sid-1.jsonl"),
    );
}

#[test]
fn agent_transcript_and_metadata() {
    let p = paths();
    assert_eq!(
        p.agent_transcript("sid-1", "a-7"),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/sid-1/subagents/agent-a-7.jsonl"),
    );
    assert_eq!(
        p.agent_metadata("sid-1", "a-7"),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/sid-1/subagents/agent-a-7.meta.json",),
    );
}

#[test]
fn agent_transcript_in_subdir() {
    assert_eq!(
        paths().agent_transcript_in_subdir("sid-1", "workflows/run-99", "a-3"),
        PathBuf::from(
            "/home/u/.coco/projects/-Users-foo-proj/sid-1/subagents/workflows/run-99/agent-a-3.jsonl"
        ),
    );
}

#[test]
fn remote_agent_metadata_path() {
    assert_eq!(
        paths().remote_agent_metadata("sid-1", "task-x"),
        PathBuf::from(
            "/home/u/.coco/projects/-Users-foo-proj/sid-1/remote-agents/remote-agent-task-x.meta.json",
        ),
    );
}

#[test]
fn tool_results_dir_path() {
    assert_eq!(
        paths().tool_results_dir("sid-1"),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/sid-1/tool-results"),
    );
}

#[test]
fn session_memory_summary_path() {
    assert_eq!(
        paths().session_memory_summary("sid-1"),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/sid-1/session-memory/summary.md",),
    );
}

#[test]
fn session_usage_path() {
    assert_eq!(
        paths().session_usage("sid-1"),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/sid-1/usage.json"),
    );
}

#[test]
fn memory_paths() {
    let p = paths();
    assert_eq!(
        p.memory_dir(),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory"),
    );
    assert_eq!(
        p.memory_entrypoint(),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory/MEMORY.md"),
    );
    assert_eq!(
        p.team_memory_dir(),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory/team"),
    );
    assert_eq!(
        p.team_memory_entrypoint(),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory/team/MEMORY.md"),
    );
    assert_eq!(
        p.consolidation_lock(),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory/.consolidate-lock"),
    );
}

#[test]
fn daily_log_zero_pads() {
    assert_eq!(
        paths().daily_log(2026, 5, 9),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory/logs/2026/05/2026-05-09.md"),
    );
}

#[test]
fn daily_log_double_digit_components() {
    assert_eq!(
        paths().daily_log(2026, 11, 23),
        PathBuf::from("/home/u/.coco/projects/-Users-foo-proj/memory/logs/2026/11/2026-11-23.md"),
    );
}
