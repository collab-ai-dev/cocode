use super::*;
use crate::BashToolHandle;
use crate::CommandHandler;
use crate::CommandResult;
use crate::PromptPart;
use std::sync::Arc;
use std::sync::RwLock;

/// Mock handle: echoes each command wrapped, or denies/fails uniformly.
struct MockHandle {
    deny: Option<String>,
}

#[async_trait]
impl BashToolHandle for MockHandle {
    async fn execute_with_permissions(
        &self,
        command: &str,
        _allowed_tools: &[String],
    ) -> std::result::Result<String, String> {
        match &self.deny {
            Some(msg) => Err(msg.clone()),
            None => Ok(format!("<{command}>")),
        }
    }
}

/// Build a shared cell pre-filled with the given handle.
fn cell_with(handle: Arc<dyn BashToolHandle>) -> crate::SharedBashToolHandle {
    Arc::new(RwLock::new(Some(handle)))
}

fn extract_text(result: CommandResult) -> String {
    match result {
        CommandResult::Prompt { parts, .. } => parts
            .into_iter()
            .map(|p| match p {
                PromptPart::Text { text } => text,
                PromptPart::File { .. } => String::new(),
            })
            .collect::<Vec<_>>()
            .join(""),
        other => panic!("expected Prompt, got {other:?}"),
    }
}

#[tokio::test]
async fn static_prompt_returns_body_verbatim_with_no_args() {
    let h = StaticPromptHandler::new("test", "running", "BODY");
    let r = h.execute_command("").await.unwrap();
    assert_eq!(extract_text(r), "BODY");
}

#[tokio::test]
async fn static_prompt_with_task_append_appends_args() {
    let h = StaticPromptHandler::with_task_append("test", "running", "BODY");
    let r = h.execute_command("hello world").await.unwrap();
    assert_eq!(extract_text(r), "BODY\n\n## Task\n\nhello world");
}

#[tokio::test]
async fn static_prompt_with_task_append_skips_blank_args() {
    let h = StaticPromptHandler::with_task_append("test", "running", "BODY");
    let r = h.execute_command("   ").await.unwrap();
    assert_eq!(extract_text(r), "BODY");
}

#[tokio::test]
async fn shell_expanding_routes_through_handle_and_substitutes() {
    let cell = cell_with(Arc::new(MockHandle { deny: None }));
    let h = ShellExpandingPromptHandler::new("test", "running", "before !`echo hello` after", cell);
    let r = h.execute_command("").await.unwrap();
    assert_eq!(extract_text(r), "before <echo hello> after");
}

#[tokio::test]
async fn shell_expanding_deny_aborts_with_error() {
    let cell = cell_with(Arc::new(MockHandle {
        deny: Some("permission denied".into()),
    }));
    let h = ShellExpandingPromptHandler::new("test", "running", "before !`rm -rf /` after", cell);
    let err = h.execute_command("").await.unwrap_err();
    assert!(
        matches!(err, crate::CommandsError::ShellCommandError { ref message } if message == "permission denied"),
        "got: {err:?}"
    );
}

#[tokio::test]
async fn shell_expanding_without_handle_leaves_body_verbatim() {
    // No handle injected (default empty cell) → no unguarded shell runs;
    // the marker is left in place.
    let cell: crate::SharedBashToolHandle = Arc::new(RwLock::new(None));
    let h = ShellExpandingPromptHandler::new("test", "running", "before !`echo hi` after", cell);
    let r = h.execute_command("").await.unwrap();
    assert_eq!(extract_text(r), "before !`echo hi` after");
}

#[tokio::test]
async fn shell_expanding_appends_args_after_expansion() {
    let cell = cell_with(Arc::new(MockHandle { deny: None }));
    let mut h = ShellExpandingPromptHandler::new("test", "running", "body !`echo x`", cell);
    h.args_handling = ArgsHandling::AppendUnderTask;
    let r = h.execute_command("the task").await.unwrap();
    assert_eq!(extract_text(r), "body <echo x>\n\n## Task\n\nthe task");
}
