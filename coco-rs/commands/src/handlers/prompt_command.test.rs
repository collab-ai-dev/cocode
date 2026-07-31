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

/// The projection is a *prompt*, not an execution: it must hand the model a
/// syntactically valid `Workflow(...)` call plus the cost disclosure
/// (`whenToUse` + phases) that justifies it.
#[tokio::test]
async fn workflow_launch_renders_metadata_and_a_valid_invocation() {
    let h = WorkflowLaunchPromptHandler {
        name: "deep-research".to_string(),
        description: "Deep research harness".to_string(),
        when_to_use: Some("When the user wants a cited report.".to_string()),
        phases: vec![
            ("Scope".to_string(), Some("Decompose".to_string())),
            ("Verify".to_string(), None),
        ],
        progress_message: "running dynamic workflow".to_string(),
    };

    let text = extract_text(h.execute_command("why is the sky blue?").await.unwrap());

    assert_eq!(
        text,
        "Run the \"deep-research\" workflow.\n\n\
         Deep research harness\n\n\
         When the user wants a cited report.\n\n\
         Phases:\n\
         - Scope: Decompose\n\
         - Verify\n\n\
         Invoke: Workflow({ name: \"deep-research\", args: \"why is the sky blue?\" })"
    );
}

/// A question carrying quotes or newlines must still produce a call the model
/// can reproduce verbatim — this is the only escaping in the whole path.
#[tokio::test]
async fn workflow_launch_json_escapes_the_user_argument() {
    let h = WorkflowLaunchPromptHandler {
        name: "deep-research".to_string(),
        description: "d".to_string(),
        when_to_use: None,
        phases: vec![],
        progress_message: "running dynamic workflow".to_string(),
    };

    let text = extract_text(
        h.execute_command("what is \"best\"?\nbe specific")
            .await
            .unwrap(),
    );

    assert!(
        text.ends_with(
            "Invoke: Workflow({ name: \"deep-research\", args: \"what is \\\"best\\\"?\\nbe specific\" })"
        ),
        "got: {text}"
    );
}

/// No args → no `args` key at all, rather than an empty string the harness
/// would reject as a missing research question.
#[tokio::test]
async fn workflow_launch_omits_args_when_none_were_typed() {
    let h = WorkflowLaunchPromptHandler {
        name: "release".to_string(),
        description: "d".to_string(),
        when_to_use: None,
        phases: vec![],
        progress_message: "running dynamic workflow".to_string(),
    };

    let text = extract_text(h.execute_command("   ").await.unwrap());

    assert!(
        text.ends_with("Invoke: Workflow({ name: \"release\" })"),
        "got: {text}"
    );
}
