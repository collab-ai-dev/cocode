use super::*;

/// Helper: assert the command opens the bare message selector.
fn expect_message_selector(result: CommandResult) {
    match result {
        CommandResult::OpenDialog(DialogSpec::MessageSelector) => {}
        other => panic!("expected OpenDialog(MessageSelector), got: {other:?}"),
    }
}

#[tokio::test]
async fn rewind_bare_opens_message_selector_without_preselect() {
    // bare `/rewind` ⇒ open picker on MessageSelect phase.
    // Mirrors `commands/rewind/rewind.ts:1-13`'s `_args`-unused handler
    // calling `context.openMessageSelector()`.
    let h = RewindHandler;
    expect_message_selector(h.execute_command("").await.unwrap());
}

#[tokio::test]
async fn rewind_whitespace_args_treated_as_bare() {
    let h = RewindHandler;
    expect_message_selector(h.execute_command("   \t\n  ").await.unwrap());
}

#[tokio::test]
async fn rewind_uuid_arg_is_ignored_like_ts() {
    let h = RewindHandler;
    let id = "7c3e8f9a-1234-5678-90ab-cdef01234567";
    expect_message_selector(h.execute_command(id).await.unwrap());
}

#[tokio::test]
async fn rewind_non_uuid_arg_is_ignored_like_ts() {
    let h = RewindHandler;
    expect_message_selector(h.execute_command("not-a-uuid").await.unwrap());
}

#[tokio::test]
async fn rewind_handler_name() {
    assert_eq!(RewindHandler.handler_name(), "rewind");
}
