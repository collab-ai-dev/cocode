use super::*;
use crate::generator::GeneratorContext;
use crate::generator::HookEvent;
use crate::generator::HookEventKind;
use crate::types::ReminderOutput;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn cfg() -> SystemReminderConfig {
    SystemReminderConfig::default()
}

#[tokio::test]
async fn hook_success_filters_to_session_start_and_user_prompt_events() {
    let c = cfg();
    let events = vec![
        HookEvent::Success {
            hook_name: "a".into(),
            hook_event: HookEventKind::SessionStart,
            content: "hello".into(),
        },
        HookEvent::Success {
            hook_name: "b".into(),
            hook_event: HookEventKind::Other, // filtered out
            content: "ignored".into(),
        },
        HookEvent::Success {
            hook_name: "c".into(),
            hook_event: HookEventKind::UserPromptSubmit,
            content: "world".into(),
        },
    ];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    let r = HookSuccessGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .expect("emits");
    let text = r.content().unwrap();
    assert!(text.contains("a hook success: hello"));
    assert!(text.contains("c hook success: world"));
    assert!(!text.contains("ignored"));
}

#[tokio::test]
async fn hook_success_skips_empty_content() {
    let c = cfg();
    let events = vec![HookEvent::Success {
        hook_name: "a".into(),
        hook_event: HookEventKind::SessionStart,
        content: String::new(),
    }];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    assert!(HookSuccessGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn blocking_error_formats_per_ts_template() {
    let c = cfg();
    let events = vec![HookEvent::BlockingError {
        hook_name: "pre".into(),
        command: "rm -rf /".into(),
        error: "too scary".into(),
    }];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    let text = HookBlockingErrorGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_eq!(
        text,
        "pre hook blocking error from command: \"rm -rf /\": too scary"
    );
}

#[tokio::test]
async fn additional_context_joins_lines_with_newlines() {
    let c = cfg();
    let events = vec![HookEvent::AdditionalContext {
        hook_name: "h".into(),
        content: vec!["line one".into(), "line two".into()],
    }];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    let text = HookAdditionalContextGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_eq!(text, "h hook additional context: line one\nline two");
}

#[tokio::test]
async fn additional_context_skips_empty_vec() {
    let c = cfg();
    let events = vec![HookEvent::AdditionalContext {
        hook_name: "h".into(),
        content: vec![],
    }];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    assert!(
        HookAdditionalContextGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn stopped_continuation_uses_message_field() {
    let c = cfg();
    let events = vec![HookEvent::StoppedContinuation {
        hook_name: "stop".into(),
        message: "halt reason".into(),
    }];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    let text = HookStoppedContinuationGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_eq!(text, "stop hook stopped continuation: halt reason");
}

#[tokio::test]
async fn async_response_emits_separate_messages_for_system_and_context() {
    let c = cfg();
    let events = vec![HookEvent::AsyncResponse {
        system_message: Some("sys".into()),
        additional_context: Some("ctx".into()),
    }];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    let r = AsyncHookResponseGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    match r.output {
        ReminderOutput::Messages(msgs) => {
            assert_eq!(msgs.len(), 2);
        }
        other => panic!("expected Messages, got {other:?}"),
    }
}

#[tokio::test]
async fn async_response_skips_when_both_fields_absent() {
    let c = cfg();
    let events = vec![HookEvent::AsyncResponse {
        system_message: None,
        additional_context: None,
    }];
    let ctx = GeneratorContext::builder(&c).hook_events(events).build();
    assert!(
        AsyncHookResponseGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}
