use super::*;

#[test]
fn local_command_messages_adds_context_only_caveat_when_not_responding() {
    let messages = local_command_messages("pwd", "/repo", false);

    assert_eq!(messages.len(), 2);
    let caveat = coco_messages::wrapping::extract_text_from_message(&messages[0]);
    assert!(caveat.contains("<local-command-caveat>"));
    match &messages[1] {
        coco_messages::Message::System(coco_messages::SystemMessage::LocalCommand(command)) => {
            assert_eq!(command.command, "pwd");
            assert_eq!(command.output, "/repo");
        }
        other => panic!("expected local command message, got {other:?}"),
    }
}

#[test]
fn local_command_messages_omits_caveat_when_responding() {
    let messages = local_command_messages("pwd", "/repo", true);

    assert_eq!(messages.len(), 1);
    match &messages[0] {
        coco_messages::Message::System(coco_messages::SystemMessage::LocalCommand(command)) => {
            assert_eq!(command.command, "pwd");
            assert_eq!(command.output, "/repo");
        }
        other => panic!("expected local command message, got {other:?}"),
    }
}

#[test]
fn compact_summary_message_marks_non_empty_summary() {
    let message = compact_summary_message("summary text").expect("non-empty summary");

    match message {
        coco_messages::Message::User(user) => {
            assert!(user.is_compact_summary);
            let text = coco_messages::wrapping::extract_text_from_message(
                &coco_messages::Message::User(user),
            );
            assert!(text.contains("summary text"));
        }
        other => panic!("expected compact user message, got {other:?}"),
    }
}

#[test]
fn compact_summary_message_skips_blank_summary() {
    assert!(compact_summary_message(" \n\t ").is_none());
}

#[test]
fn fork_skill_result_messages_include_metadata_and_body() {
    let messages = fork_skill_result_messages(
        "<command-name>/demo</command-name>",
        "<local-command-stdout>\nok\n</local-command-stdout>",
    );

    assert_eq!(messages.len(), 2);
    match &messages[0] {
        coco_messages::Message::Attachment(attachment) => {
            assert_eq!(
                attachment.kind,
                coco_types::AttachmentKind::SlashCommandMetadata
            );
        }
        other => panic!("expected slash metadata attachment, got {other:?}"),
    }
    let body = coco_messages::wrapping::extract_text_from_message(&messages[1]);
    assert!(body.contains("<local-command-stdout>"));
    assert!(body.contains("ok"));
}

#[test]
fn slash_command_metadata_matches_ts_shape() {
    assert_eq!(
        slash_command_metadata("simplify", "focus on tests"),
        "<command-message>simplify</command-message>\n\
         <command-name>/simplify</command-name>\n\
         <command-args>focus on tests</command-args>"
    );
}

#[test]
fn slash_command_metadata_omits_empty_args() {
    assert_eq!(
        slash_command_metadata("simplify", ""),
        "<command-message>simplify</command-message>\n\
         <command-name>/simplify</command-name>"
    );
}

#[test]
fn slash_result_messages_can_build_error_shape() {
    let messages = slash_result_messages("permissions", "bad", "Nope", true);

    assert_eq!(messages.len(), 2);
    let text = coco_messages::wrapping::extract_text_from_message(&messages[1]);
    assert!(text.contains("Nope"));
}
