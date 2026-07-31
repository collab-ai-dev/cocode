//! End-to-end execution of the bundled `deep-research` harness in the real
//! QuickJS realm.
//!
//! The harness is *data* — a ~330-line JavaScript string that nothing in the
//! build type-checks. `coco-workflow`'s unit tests prove it parses; only
//! running it proves the realm actually supports what it uses (the WHATWG
//! authority regex, `?.` / `??`, object spread, `Array.from`, the
//! `pipeline`→`parallel` nesting, and `phase()` interning against
//! `meta.phases`). A stub host stands in for the subagents and returns
//! schema-shaped fixtures, so the assertions are about the harness's own
//! control flow and arithmetic, not about any model's output.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use coco_types::WorkflowProgressEvent;
use coco_workflow_runtime::WorkflowAgentOpts;
use coco_workflow_runtime::WorkflowAgentOutcome;
use coco_workflow_runtime::WorkflowAgentResult;
use coco_workflow_runtime::WorkflowAgentStarted;
use coco_workflow_runtime::WorkflowEngine;
use coco_workflow_runtime::WorkflowHost;
use coco_workflow_runtime::WorkflowRun;
use coco_workflow_runtime::WorkflowRunState;
use pretty_assertions::assert_eq;
use tokio_util::sync::CancellationToken;

/// How a stubbed verifier should vote, so a test can drive a claim to each of
/// the harness's three outcomes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VerifierBehavior {
    /// Every voter returns `refuted: false` — the claim survives.
    Confirm,
    /// Every voter returns `refuted: true` — the claim is killed on merit.
    Refute,
    /// Every voter errors — no quorum, so the claim is *unverified* rather than
    /// refuted. This is the `.196` distinction the harness exists to make.
    Error,
}

/// Stands in for the subagent fleet: dispatches on the phase label and returns
/// a value matching that call's schema.
struct StubHost {
    verifier: VerifierBehavior,
    /// One entry per `agent()` call, in call order: `(label, phase)`.
    calls: Mutex<Vec<(String, Option<String>)>>,
    phases: Mutex<Vec<(i32, String)>>,
}

impl StubHost {
    fn new(verifier: VerifierBehavior) -> Arc<Self> {
        Arc::new(Self {
            verifier,
            calls: Mutex::new(Vec::new()),
            phases: Mutex::new(Vec::new()),
        })
    }

    fn calls(&self) -> Vec<(String, Option<String>)> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn labels_in_phase(&self, phase: &str) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|(_, call_phase)| call_phase.as_deref() == Some(phase))
            .map(|(label, _)| label)
            .collect()
    }
}

#[async_trait::async_trait(?Send)]
impl WorkflowHost for StubHost {
    async fn run_agent(
        &self,
        _prompt: String,
        opts: WorkflowAgentOpts,
        started: WorkflowAgentStarted<'_>,
    ) -> Result<WorkflowAgentOutcome, String> {
        started();
        let label = opts.label.clone().unwrap_or_default();
        self.calls
            .lock()
            .expect("calls lock")
            .push((label.clone(), opts.phase));

        let value = if label == "scope" {
            serde_json::json!({
                "question": "does it work",
                "summary": "three complementary angles",
                "angles": [
                    { "label": "primary", "query": "q1", "rationale": "r1" },
                    { "label": "skeptical", "query": "q2", "rationale": "r2" },
                    { "label": "practitioner", "query": "q3", "rationale": "r3" },
                ],
            })
        } else if label.starts_with("search:") {
            // Every angle returns the same shared URL plus one of its own, so
            // the dedup stage has something real to collapse. The shared URL
            // differs only by trailing slash / `www.` / case / port / userinfo /
            // query string across angles — all of which `normURL` must fold.
            let angle = label.trim_start_matches("search:").to_string();
            serde_json::json!({
                "results": [
                    { "url": shared_url_variant(&angle), "title": "Shared", "relevance": "high" },
                    { "url": format!("https://{angle}.example.com/a"), "title": "Own", "relevance": "medium" },
                ],
            })
        } else if label.starts_with("fetch:") {
            serde_json::json!({
                "sourceQuality": "primary",
                "publishDate": "2026-01-01",
                "claims": [{
                    "claim": format!("claim from {label}"),
                    "quote": "a direct quote",
                    "importance": "central",
                }],
            })
        } else if label.starts_with('v') && label.contains(':') {
            match self.verifier {
                VerifierBehavior::Confirm => serde_json::json!({
                    "refuted": false, "evidence": "corroborated", "confidence": "high",
                }),
                VerifierBehavior::Refute => serde_json::json!({
                    "refuted": true, "evidence": "contradicted by a primary source", "confidence": "high",
                }),
                VerifierBehavior::Error => return Err("verifier rate-limited".to_string()),
            }
        } else if label == "synthesize" {
            serde_json::json!({
                "summary": "it works",
                "findings": [{
                    "claim": "merged finding",
                    "confidence": "high",
                    "sources": ["https://example.com/shared"],
                    "evidence": "three confirming votes",
                }],
                "caveats": "stubbed run",
            })
        } else {
            return Err(format!("unexpected agent label: {label}"));
        };

        Ok(WorkflowAgentOutcome::Completed(WorkflowAgentResult {
            value,
            model: None,
            tokens: Some(1),
            tool_calls: Some(1),
            duration_ms: Some(1),
        }))
    }

    fn push_progress(&self, event: WorkflowProgressEvent) {
        if let WorkflowProgressEvent::WorkflowPhase { index, title } = event {
            self.phases
                .lock()
                .expect("phases lock")
                .push((index, title));
        }
    }
}

/// Six spellings of one URL. `normURL` strips userinfo, `www.`, the port and a
/// trailing slash, lowercases, and drops the query — so all six collapse to the
/// single dedup key `example.com/shared`.
fn shared_url_variant(angle: &str) -> String {
    match angle {
        "primary" => "https://example.com/shared".to_string(),
        "skeptical" => "https://www.example.com/shared/".to_string(),
        _ => "https://user@Example.COM:443/shared?utm_source=x".to_string(),
    }
}

fn run(
    host: Arc<StubHost>,
    args: serde_json::Value,
) -> Result<serde_json::Value, coco_workflow_runtime::WorkflowRuntimeError> {
    let script = coco_workflow::bundled_workflow("deep-research")
        .expect("deep-research is bundled")
        .script;
    let parsed = coco_workflow::parse_workflow_script(script, /*check_determinism*/ true)
        .expect("bundled script parses");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        WorkflowEngine::run(WorkflowRun {
            script: parsed.script_body,
            args,
            // Pre-seed the phase table from `meta.phases`, exactly as the
            // launcher does — this is what keeps `Scope..Synthesize` at indices
            // 1..5 regardless of which concurrent agent resolves first.
            state: Arc::new(WorkflowRunState::new(
                parsed.meta.phases.iter().map(|phase| phase.title.clone()),
            )),
            host,
            cancel: CancellationToken::new(),
            sync_eval_budget: Duration::from_secs(30),
            depth: 0,
            child_group: None,
        })
        .await
    })
}

#[test]
fn test_deep_research_runs_end_to_end_and_reports_its_own_accounting() {
    let host = StubHost::new(VerifierBehavior::Confirm);
    let result = run(host.clone(), serde_json::json!("does it work")).expect("run");

    assert_eq!(result["question"], serde_json::json!("does it work"));
    assert_eq!(result["summary"], serde_json::json!("it works"));
    assert_eq!(result["findings"].as_array().expect("findings").len(), 1);

    // 3 angles × 2 results = 6, minus 2 collapsed duplicates of the shared URL
    // = 4 distinct sources, one claim each.
    let stats = &result["stats"];
    assert_eq!(stats["angles"], serde_json::json!(3));
    assert_eq!(stats["sourcesFetched"], serde_json::json!(4));
    assert_eq!(stats["claimsExtracted"], serde_json::json!(4));
    assert_eq!(stats["urlDupes"], serde_json::json!(2));
    assert_eq!(stats["confirmed"], serde_json::json!(4));
    assert_eq!(stats["killed"], serde_json::json!(0));
    assert_eq!(stats["unverified"], serde_json::json!(0));
    // 1 scope + 3 searches + 4 fetches + 4×3 verifiers + 1 synthesis.
    assert_eq!(stats["agentCalls"], serde_json::json!(21));
    assert_eq!(host.calls().len(), 21);
}

/// The `.207` label contract: a Fetch chip asserts the *real* fetch host, so it
/// may only be a bare hostname when the captured host survives sanitisation
/// intact. Anything else is quoted, and quoting is what stops web-controlled
/// text from forging a trusted domain in the terminal.
#[test]
fn test_fetch_labels_report_the_source_hostname() {
    let host = StubHost::new(VerifierBehavior::Confirm);
    run(host.clone(), serde_json::json!("does it work")).expect("run");

    let mut fetch_labels = host.labels_in_phase("Fetch");
    fetch_labels.sort();
    assert_eq!(
        fetch_labels,
        [
            "fetch:example.com",
            "fetch:practitioner.example.com",
            "fetch:primary.example.com",
            "fetch:skeptical.example.com",
        ]
    );
}

/// Phases are pre-seeded from `meta.phases`, so their indices reflect the
/// declared pipeline order rather than whichever concurrent agent finished
/// first. Without seeding, `Fetch` could outrace `Search` for index 2.
#[test]
fn test_phase_indices_follow_the_declared_pipeline_not_completion_order() {
    let host = StubHost::new(VerifierBehavior::Confirm);
    run(host.clone(), serde_json::json!("does it work")).expect("run");

    // Titles are interned at seed time, so no `phase()` call in the script and
    // no `{phase:}` opt publishes a new group node.
    assert_eq!(host.phases.lock().expect("phases lock").len(), 0);
}

/// `.196`: a verifier panel that never ran must report `unverified`, not
/// `refuted`. An infra failure that reads as "all claims refuted" tells the user
/// their research found nothing when it actually never happened.
#[test]
fn test_failed_verifier_panels_report_unverified_not_refuted() {
    let host = StubHost::new(VerifierBehavior::Error);
    let result = run(host, serde_json::json!("does it work")).expect("run");

    assert_eq!(result["findings"], serde_json::json!([]));
    assert_eq!(result["refuted"], serde_json::json!([]));
    assert_eq!(
        result["unverified"].as_array().expect("unverified").len(),
        4
    );
    assert_eq!(result["stats"]["killed"], serde_json::json!(0));
    assert_eq!(result["stats"]["unverified"], serde_json::json!(4));
    let summary = result["summary"].as_str().expect("summary");
    assert!(
        summary.contains("infrastructure failure"),
        "summary must name the infra failure, got: {summary}"
    );
}

/// The other side of `.196`: claims the panel actually adjudicated against are
/// `refuted`, and the summary must not blame infrastructure.
#[test]
fn test_refuted_claims_are_reported_as_refuted_on_merit() {
    let host = StubHost::new(VerifierBehavior::Refute);
    let result = run(host, serde_json::json!("does it work")).expect("run");

    assert_eq!(result["refuted"].as_array().expect("refuted").len(), 4);
    assert_eq!(result["unverified"], serde_json::json!([]));
    let summary = result["summary"].as_str().expect("summary");
    assert!(
        summary.contains("refuted by adversarial verification"),
        "got: {summary}"
    );
    assert!(!summary.contains("infrastructure"), "got: {summary}");
}

/// The only hard input contract in the harness: no question, no run — and it
/// must say so instead of spending a scope agent on an empty string.
#[test]
fn test_missing_question_returns_an_error_without_spawning_any_agent() {
    for args in [serde_json::Value::Null, serde_json::json!("   ")] {
        let host = StubHost::new(VerifierBehavior::Confirm);
        let result = run(host.clone(), args).expect("run");
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|error| error.contains("No research question provided")),
            "got: {result}"
        );
        assert_eq!(host.calls().len(), 0);
    }
}
