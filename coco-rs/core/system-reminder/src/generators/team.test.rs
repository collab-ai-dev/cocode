use super::*;
use crate::generator::AgentPendingMessage;
use crate::generator::GeneratorContext;
use crate::generator::TeamContextSnapshot;
use crate::generator::TeammateMailboxInfo;
use crate::types::ContentBlock;
use crate::types::MessageRole;
use crate::types::ReminderOutput;
use coco_config::SystemReminderConfig;
use pretty_assertions::assert_eq;

fn pending_texts(reminder: &crate::types::SystemReminder) -> Vec<String> {
    let ReminderOutput::Messages(msgs) = &reminder.output else {
        panic!("expected Messages output, got {:?}", reminder.output);
    };
    msgs.iter()
        .map(|m| {
            assert_eq!(m.role, MessageRole::User);
            match m.blocks.as_slice() {
                [ContentBlock::Text { text }] => text.clone(),
                other => panic!("expected single text block, got {other:?}"),
            }
        })
        .collect()
}

#[tokio::test]
async fn teammate_mailbox_skips_when_none_or_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c).teammate_mailbox(None).build();
    assert!(
        TeammateMailboxGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
    let ctx = GeneratorContext::builder(&c)
        .teammate_mailbox(Some(TeammateMailboxInfo::default()))
        .build();
    assert!(
        TeammateMailboxGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn teammate_mailbox_passes_formatted_verbatim() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .teammate_mailbox(Some(TeammateMailboxInfo {
            formatted: "You have 2 messages:\n- @alice: hi".into(),
        }))
        .build();
    let text = TeammateMailboxGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert_eq!(text, "You have 2 messages:\n- @alice: hi");
}

#[tokio::test]
async fn team_context_skips_when_missing_required_fields() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .team_context(Some(TeamContextSnapshot {
            agent_id: String::new(), // missing
            agent_name: "name".into(),
            team_name: "team".into(),
            team_config_path: "/".into(),
            task_list_path: "/".into(),
        }))
        .build();
    assert!(TeamContextGenerator.generate(&ctx).await.unwrap().is_none());
}

#[tokio::test]
async fn team_context_renders_full_ts_template() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .team_context(Some(TeamContextSnapshot {
            agent_id: "worker-1".into(),
            agent_name: "Worker One".into(),
            team_name: "alpha".into(),
            team_config_path: "/cfg".into(),
            task_list_path: "/tasks".into(),
        }))
        .build();
    let text = TeamContextGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap()
        .content()
        .unwrap()
        .to_string();
    assert!(text.starts_with("# Team Coordination"));
    assert!(text.contains("team \"alpha\""));
    assert!(text.contains("**Your Identity:**\n- Name: Worker One"));
    assert!(text.contains("**Team Resources:**"));
    assert!(text.contains("- Team config: /cfg"));
    assert!(text.contains("- Task list: /tasks"));
    assert!(text.contains(
        "**Team Leader:** The team lead's name is \"team-lead\". Send updates and completion notifications to them."
    ));
    assert!(text.contains("Read the team config to discover your teammates' names."));
    assert!(
        text.contains(
            "**IMPORTANT:** Always refer to teammates by their NAME (e.g., \"team-lead\", \"analyzer\", \"researcher\"), never by UUID."
        )
    );
    assert!(text.contains("```json"));
    assert!(text.contains("\"to\": \"team-lead\""));
    assert!(text.contains("\"summary\": \"Brief 5-10 word preview\""));
    // agent_id is NOT surfaced in the body — only agent_name.
    assert!(
        !text.contains("worker-1"),
        "agent_id leaked into body: {text}"
    );
}

#[tokio::test]
async fn agent_pending_messages_skips_when_empty() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .agent_pending_messages(vec![])
        .build();
    assert!(
        AgentPendingMessagesGenerator
            .generate(&ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn agent_pending_messages_emits_one_coordinator_framed_per_entry() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .agent_pending_messages(vec![
            AgentPendingMessage {
                from: "alice".into(),
                text: "ping".into(),
            },
            AgentPendingMessage {
                from: "bob".into(),
                text: "please review".into(),
            },
        ])
        .build();
    let reminder = AgentPendingMessagesGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    let texts = pending_texts(&reminder);
    assert_eq!(texts.len(), 2);
    for body in &texts {
        assert!(
            body.starts_with("The coordinator sent a message while you were working:\n"),
            "missing coordinator framing: {body}"
        );
        assert!(body.ends_with("Address this before completing your current task."));
    }
    assert!(texts[0].contains("\nping\n"));
    assert!(texts[1].contains("\nplease review\n"));
}

#[tokio::test]
async fn agent_pending_messages_drops_empty_text_entries() {
    let c = SystemReminderConfig::default();
    let ctx = GeneratorContext::builder(&c)
        .agent_pending_messages(vec![
            AgentPendingMessage {
                from: "alice".into(),
                text: String::new(),
            },
            AgentPendingMessage {
                from: "bob".into(),
                text: "real".into(),
            },
        ])
        .build();
    let reminder = AgentPendingMessagesGenerator
        .generate(&ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending_texts(&reminder).len(), 1);
}
