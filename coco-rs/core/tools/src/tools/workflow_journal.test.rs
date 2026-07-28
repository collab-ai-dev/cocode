use coco_workflow_runtime::AgentCacheKey;
use coco_workflow_runtime::WorkflowAgentOpts;
use pretty_assertions::assert_eq;

use super::WorkflowJournal;
use super::journal_key;
use super::journal_path_for_output;
use super::script_path_for_output;

/// A cache key for the `call_index`-th `agent()` call in a run.
fn key_at(call_index: i32, prompt: &str, phase: Option<&str>) -> AgentCacheKey {
    let opts = WorkflowAgentOpts {
        phase: phase.map(str::to_string),
        ..WorkflowAgentOpts::default()
    };
    AgentCacheKey::new(call_index, prompt.to_string(), &opts)
}

/// Shorthand for tests that only care about one call site.
fn key(prompt: &str, phase: Option<&str>) -> AgentCacheKey {
    key_at(0, prompt, phase)
}

#[tokio::test]
async fn record_then_resume_replays_the_cached_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("run.journal.jsonl");

    // Live run: record two results.
    let live = WorkflowJournal::new(Some(path.clone()));
    let k1 = key_at(0, "first", Some("Plan"));
    let k2 = key_at(1, "second", Some("Plan"));
    live.record(&k1, &serde_json::json!("result-one")).await;
    live.record(&k2, &serde_json::json!({ "ok": true })).await;

    // Resume from the same journal: the cache hydrates and replays both.
    let resumed = WorkflowJournal::resumed(&path, Some(path.clone()));
    assert_eq!(resumed.lookup(&k1), Some(serde_json::json!("result-one")));
    assert_eq!(resumed.lookup(&k2), Some(serde_json::json!({ "ok": true })));
    // A key never recorded misses.
    assert_eq!(resumed.lookup(&key("third", Some("Plan"))), None);
}

#[tokio::test]
async fn record_skips_null_results() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("run.journal.jsonl");
    let live = WorkflowJournal::new(Some(path.clone()));
    let k = key("nullish", None);
    live.record(&k, &serde_json::Value::Null).await;

    // Null was not journaled, so resume finds no hit for it.
    let resumed = WorkflowJournal::resumed(&path, Some(path.clone()));
    assert_eq!(resumed.lookup(&k), None);
}

#[tokio::test]
async fn result_entry_is_last_write_wins() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("run.journal.jsonl");
    let live = WorkflowJournal::new(Some(path.clone()));
    let k = key("same", None);
    live.record(&k, &serde_json::json!("old")).await;
    live.record(&k, &serde_json::json!("new")).await;

    let resumed = WorkflowJournal::resumed(&path, Some(path.clone()));
    assert_eq!(resumed.lookup(&k), Some(serde_json::json!("new")));
}

#[test]
fn journal_key_is_stable_and_distinct() {
    let k = key("prompt", Some("Plan"));
    // Stable across calls.
    assert_eq!(journal_key(&k), journal_key(&k));
    // Versioned prefix.
    assert!(journal_key(&k).starts_with("wfj2:"));
    // A different prompt yields a different hash.
    assert_ne!(journal_key(&k), journal_key(&key("other", Some("Plan"))));
}

#[test]
fn journal_key_separates_repeated_identical_calls_by_ordinal() {
    // The loop-until-count pattern: one prompt, many calls. Without the ordinal
    // all of them collapse onto a single journal entry and a resumed run replays
    // one recorded value for every iteration.
    let first = key_at(0, "Find bugs in this codebase.", None);
    let second = key_at(1, "Find bugs in this codebase.", None);
    assert_ne!(journal_key(&first), journal_key(&second));
}

#[test]
fn journal_key_ignores_cosmetic_opts() {
    // Regrouping the progress tree must not re-spawn agents on resume, so the
    // phase is not part of the key.
    assert_eq!(
        journal_key(&key("prompt", Some("Plan"))),
        journal_key(&key("prompt", Some("Build")))
    );
    assert_eq!(
        journal_key(&key("prompt", Some("Plan"))),
        journal_key(&key("prompt", None))
    );
}

#[test]
fn journal_path_sits_alongside_output() {
    let out = std::path::Path::new("/x/cache/tasks/sess/w_abc.output");
    let journal = journal_path_for_output(out).expect("journal path");
    assert_eq!(
        journal,
        std::path::PathBuf::from("/x/cache/tasks/sess/w_abc.journal.jsonl")
    );
}

#[test]
fn script_path_sits_alongside_output() {
    let out = std::path::Path::new("/x/cache/tasks/sess/w_abc.output");
    assert_eq!(
        script_path_for_output(out),
        std::path::PathBuf::from("/x/cache/tasks/sess/w_abc.workflow.js")
    );
}

#[tokio::test]
async fn cross_run_resume_hydrates_prior_journal_into_a_new_run_journal() {
    // Step 4 launch semantics: a cross-run resume reads the PRIOR run's journal
    // (source) and continues appending to a NEW run's journal (target). Prior
    // results replay; the diverged tail's new results land in the new journal,
    // leaving the prior journal untouched.
    let dir = tempfile::tempdir().expect("tempdir");
    let prior = dir.path().join("prior.journal.jsonl");
    let new_run = dir.path().join("new.journal.jsonl");

    // Prior run completed one agent() result.
    let live = WorkflowJournal::new(Some(prior.clone()));
    let k1 = key_at(0, "first", Some("Plan"));
    live.record(&k1, &serde_json::json!("prior-result")).await;

    // Resume: hydrate from the prior journal, append to the new run's journal.
    let resumed = WorkflowJournal::resumed(&prior, Some(new_run.clone()));
    assert_eq!(
        resumed.lookup(&k1),
        Some(serde_json::json!("prior-result")),
        "prior result replays from the source journal"
    );
    // A diverged-tail result records into the NEW journal.
    let k2 = key_at(1, "second", Some("Plan"));
    resumed.record(&k2, &serde_json::json!("tail-result")).await;

    // The new run's journal now hydrates both (prior replay + new tail).
    let reopened = WorkflowJournal::resumed(&new_run, Some(new_run.clone()));
    assert_eq!(reopened.lookup(&k2), Some(serde_json::json!("tail-result")));

    // The prior journal was not mutated: it still only knows the first result.
    let prior_reopened = WorkflowJournal::resumed(&prior, Some(prior.clone()));
    assert_eq!(
        prior_reopened.lookup(&k1),
        Some(serde_json::json!("prior-result"))
    );
    assert_eq!(prior_reopened.lookup(&k2), None);
}

#[tokio::test]
async fn cache_only_journal_has_no_path_and_still_records_in_memory() {
    // No path → persistence disabled, but the in-memory cache still serves the
    // same run (record → lookup within one instance).
    let live = WorkflowJournal::new(None);
    let k = key("p", None);
    live.record(&k, &serde_json::json!("v")).await;
    assert_eq!(live.lookup(&k), Some(serde_json::json!("v")));
}
