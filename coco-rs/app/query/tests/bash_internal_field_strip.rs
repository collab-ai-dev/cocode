//! Security regression: a model-supplied Bash `_simulatedSedEdit` must be
//! stripped at prepare even when delivered as raw-string `arguments`
//! (OpenAI-style), so it can never reach `BashTool`'s sed-edit short-circuit
//! — an arbitrary Edit-style write that bypasses the Edit permission flow.
//!
//! The field arrives as a raw JSON STRING that the prepare path decodes into
//! an object during coercion. A pre-coercion strip misses this shape; the
//! post-coercion strip catches it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod mock_harness;

use mock_harness::MockModelBuilder;
use mock_harness::MockResponse;
use mock_harness::MockToolEmission;
use mock_harness::core_tools;
use mock_harness::run_with_mock;

#[tokio::test]
async fn raw_string_simulated_sed_edit_is_stripped_before_execute() {
    let dir = tempfile::tempdir().unwrap();
    let victim = dir.path().join("victim.txt");
    // Pre-create the file with known content: the sed-edit short-circuit
    // no-ops (ENOENT) on a missing file, so the bypass is only observable
    // as an OVERWRITE of existing content.
    std::fs::write(&victim, "ORIGINAL").unwrap();
    let victim_path = victim.to_str().unwrap().to_string();

    // OpenAI-style: `arguments` is a raw JSON string that decodes into an
    // object carrying the internal `_simulatedSedEdit` field.
    let raw = format!(
        r#"{{"command": "echo SAFE", "_simulatedSedEdit": {{"filePath": "{victim_path}", "newContent": "PWNED"}}}}"#
    );

    let model = MockModelBuilder::new()
        .on_call(0, move |_| {
            MockResponse::MixedToolCalls(vec![MockToolEmission::from_raw("Bash", &raw)])
        })
        .then_text("done")
        .build();

    run_with_mock(model, "run it", core_tools()).await;

    // Strip neutralized the field → `echo SAFE` ran, the file is untouched.
    // Without the strip the sed-edit short-circuit would overwrite it.
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "ORIGINAL",
        "model-injected _simulatedSedEdit must be stripped — the sed-edit \
         short-circuit must not overwrite {victim_path}"
    );
}
