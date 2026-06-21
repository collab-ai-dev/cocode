use super::*;
use crate::CommandHandler;

#[test]
fn test_resolve_model_alias() {
    let m = resolve_model("sonnet").unwrap();
    assert_eq!(m.provider, "anthropic");
    assert_eq!(m.model_id, "claude-sonnet-4-6");
}

#[test]
fn test_resolve_model_alias_case_insensitive() {
    let m = resolve_model("OPUS").unwrap();
    assert_eq!(m.model_id, "claude-opus-4-7");
}

#[test]
fn test_resolve_model_full_id() {
    let m = resolve_model("claude-haiku-4-5").unwrap();
    assert_eq!(m.provider, "anthropic");
    assert_eq!(m.model_id, "claude-haiku-4-5");
}

#[test]
fn test_resolve_model_prefix() {
    let m = resolve_model("claude-sonnet").unwrap();
    assert_eq!(m.provider, "anthropic");
    // Prefix matches the first registry key alphabetically — for the
    // current builtin set that's `claude-sonnet-4-6`. The test stays
    // robust if more sonnet variants land because the assertion is on
    // prefix membership rather than exact id.
    assert!(m.model_id.starts_with("claude-sonnet"));
}

#[test]
fn test_resolve_model_provider_inference() {
    assert_eq!(resolve_model("gpt5").unwrap().provider, "openai");
    assert_eq!(resolve_model("gemini-pro").unwrap().provider, "google");
    assert_eq!(resolve_model("deepseek-pro").unwrap().provider, "deepseek");
}

#[test]
fn test_resolve_model_unknown() {
    assert!(resolve_model("llama").is_none());
    assert!(resolve_model("totally-not-a-model").is_none());
}

#[test]
fn test_format_context_units() {
    assert_eq!(format_context(1_000_000), "1M");
    assert_eq!(format_context(200_000), "200K");
    assert_eq!(format_context(272_000), "272K");
    assert_eq!(format_context(900), "900");
}

#[test]
fn test_builtin_summary_sorted_by_provider() {
    let entries = builtin_summary();
    // Verify provider clusters: anthropic first, then deepseek, google, openai.
    let providers: Vec<&str> = entries.iter().map(|e| e.provider).collect();
    let mut last: Option<&str> = None;
    for p in &providers {
        if let Some(prev) = last {
            assert!(prev <= *p, "providers out of order: {prev} > {p}");
        }
        last = Some(*p);
    }
}

/// Run `f` with `COCO_CONFIG_DIR` pointed at a fresh tempdir so the
/// settings-write side effect of `/model <name>` doesn't touch the
/// developer's real `~/.coco/settings.json`.
async fn with_tmp_config_dir<F, Fut, T>(f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let tmp = tempfile::tempdir().unwrap();
    let prev = std::env::var_os("COCO_CONFIG_DIR");
    // SAFETY: tests run with --test-threads=1 by default in nextest;
    // env vars are restored on scope exit. Mirror of the pattern in
    // `commands/handlers/keybindings.test.rs`.
    unsafe {
        std::env::set_var("COCO_CONFIG_DIR", tmp.path());
    }
    let result = f().await;
    unsafe {
        match prev {
            Some(v) => std::env::set_var("COCO_CONFIG_DIR", v),
            None => std::env::remove_var("COCO_CONFIG_DIR"),
        }
    }
    result
}

#[tokio::test]
async fn test_handler_no_args_opens_picker() {
    let handler = ModelHandler;
    let result = handler.execute_command("").await.unwrap();
    assert!(matches!(
        result,
        CommandResult::OpenDialog(DialogSpec::ModelPicker)
    ));
}

#[tokio::test]
async fn test_handler_valid_model_persists() {
    let output = with_tmp_config_dir(|| async {
        let handler = ModelHandler;
        handler.execute_command("opus").await.unwrap()
    })
    .await;
    let text = match output {
        CommandResult::Text(t) => t,
        other => panic!("expected Text result, got {other:?}"),
    };
    assert!(text.contains("Set Main"), "missing 'Set Main' in {text}");
    assert!(text.contains("anthropic/claude-opus-4-7"));
    assert!(text.contains("persisted to"));
}

#[tokio::test]
async fn test_handler_unknown_model() {
    let handler = ModelHandler;
    let result = handler.execute_command("llama").await.unwrap();
    let text = match result {
        CommandResult::Text(t) => t,
        other => panic!("expected Text result, got {other:?}"),
    };
    assert!(text.contains("Unknown model"));
    assert!(text.contains("anthropic/claude-"));
}
