use super::*;

#[test]
fn recorder_captures_and_drains_in_order() {
    let rec: Recorder<i32> = Recorder::new();
    assert!(rec.is_empty());
    rec.record(1);
    rec.record(2);
    assert_eq!(rec.count(), 2);
    assert_eq!(rec.snapshot(), vec![1, 2]);
    assert_eq!(rec.take(), vec![1, 2]);
    assert!(rec.is_empty());
}

#[test]
fn recorder_clone_shares_one_buffer() {
    let a: Recorder<&str> = Recorder::new();
    let b = a.clone();
    a.record("x");
    assert_eq!(b.snapshot(), vec!["x"]);
    b.clear();
    assert!(a.is_empty());
}

#[tokio::test]
async fn recording_hook_handle_captures_each_callback_in_order() {
    let handle = RecordingHookHandle::default();
    let input = serde_json::json!({ "file_path": "/tmp/x" });

    handle.run_pre_tool_use("Read", "toolu_1", &input).await;
    handle
        .run_post_tool_use(
            "Read",
            "toolu_1",
            &input,
            &serde_json::json!({ "ok": true }),
        )
        .await;
    handle
        .run_post_tool_use_failure("Bash", "toolu_2", &input, "boom")
        .await;

    let calls = handle.calls.snapshot();
    assert_eq!(calls.len(), 3);
    assert!(matches!(&calls[0], HookCall::Pre { tool_name, .. } if tool_name == "Read"));
    assert!(matches!(&calls[1], HookCall::Post { tool_name, .. } if tool_name == "Read"));
    assert!(
        matches!(&calls[2], HookCall::PostFailure { tool_name, error_message, .. }
            if tool_name == "Bash" && error_message == "boom")
    );
}

#[test]
#[should_panic(expected = "unexpected call to `frobnicate`")]
fn unexpected_call_panics_loudly() {
    unexpected_call("frobnicate");
}
