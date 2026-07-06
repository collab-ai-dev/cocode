use crate::CommandHandler;
use crate::CommandResult;

use super::MoaHandler;

#[tokio::test]
async fn moa_prompt_returns_one_shot_result() {
    let result = MoaHandler
        .execute_command("  compare these options  ")
        .await
        .expect("moa command");

    match result {
        CommandResult::MoaOneShot { prompt } => {
            assert_eq!(prompt, "compare these options");
        }
        other => panic!("expected MoA one-shot result, got {other:?}"),
    }
}

#[tokio::test]
async fn moa_without_prompt_returns_usage() {
    let result = MoaHandler
        .execute_command("   ")
        .await
        .expect("moa command");

    match result {
        CommandResult::Text(text) => {
            assert!(text.contains("Usage: /moa <prompt>"));
        }
        other => panic!("expected usage text, got {other:?}"),
    }
}
