use super::*;

#[test]
fn normalize_output_trims_filters_empty_and_joins_lines() {
    assert_eq!(normalize_output("  one  \n\n two\n  \n"), "one\ntwo");
}

#[test]
fn strip_ansi_removes_escape_sequences() {
    assert_eq!(strip_ansi("\u{1b}[31mred\u{1b}[0m plain"), "red plain");
}

#[test]
fn apply_update_drops_stale_generation_and_failures() {
    let mut runtime = StatusLineRuntime {
        generation: 2,
        ..Default::default()
    };

    assert!(!runtime.apply_update(StatusLineUpdate {
        generation: 1,
        output: Some("old".to_string()),
    }));
    assert!(!runtime.apply_update(StatusLineUpdate {
        generation: 2,
        output: None,
    }));
    assert!(runtime.apply_update(StatusLineUpdate {
        generation: 2,
        output: Some("ok\nextra".to_string()),
    }));
    assert_eq!(runtime.last_success(), Some("ok"));
}

#[tokio::test]
async fn command_receives_json_stdin_and_trims_output() {
    let output = run_status_line_command(
        "python3 -c 'import sys,json; data=json.load(sys.stdin); print(\"  \" + data[\"session_id\"] + \"  \"); print(); print(\"ignored\")'",
        r#"{"session_id":"abc"}"#,
    )
    .await
    .expect("command succeeds");

    assert_eq!(output, "abc\nignored");
}

#[tokio::test]
async fn command_rejects_empty_output() {
    let output = run_status_line_command("printf '  \n\n'", "{}")
        .await
        .expect("command succeeds");

    assert!(output.is_empty());
}

#[test]
fn input_uses_main_role_provider_before_session_fallback() {
    let mut state = AppState::default();
    state.session.provider = "session-provider".into();
    state.session.model = "session-model".into();
    state
        .session
        .model_catalog
        .push(crate::state::ModelCatalogEntry {
            provider: "main-provider".into(),
            provider_display: "Main Provider".into(),
            model_id: "main-model".into(),
            display_name: "Main Model".into(),
            context_window: None,
            supported_efforts: Vec::new(),
            default_effort: None,
        });
    state.session.model_by_role.insert(
        coco_types::ModelRole::Main,
        crate::state::ModelBinding {
            provider: "main-provider".into(),
            model_id: "main-model".into(),
            context_window: None,
            effort: None,
        },
    );

    let value = serde_json::to_value(status_line_input(&state)).unwrap();

    assert_eq!(value["model"]["provider"], "main-provider");
    assert_eq!(value["model"]["id"], "main-model");
    assert_eq!(value["model"]["display_name"], "Main Model");
}

#[tokio::test]
async fn command_timeout_drops_child_before_late_side_effect() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("late-marker");
    let marker_arg = serde_json::to_string(marker.to_str().unwrap()).unwrap();
    let command = format!(
        "python3 -c 'import pathlib, time; time.sleep(0.2); pathlib.Path({marker_arg}).write_text(\"done\")'"
    );

    let result =
        run_status_line_command_with_timeout(&command, "{}", Duration::from_millis(25)).await;
    assert!(result.is_err());

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !marker.exists(),
        "timed out statusLine command kept running"
    );
}
