use super::*;

#[test]
fn test_wrap_in_system_reminder() {
    let wrapped = wrap_in_system_reminder("hello world");
    assert!(wrapped.starts_with("<system-reminder>"));
    assert!(wrapped.ends_with("</system-reminder>"));
    assert!(wrapped.contains("hello world"));
}

#[test]
fn test_create_system_reminder_message() {
    // Post-Phase-2: system reminders land as Message::Attachment.
    let msg = create_system_reminder_message("test reminder");
    assert!(matches!(
        msg,
        Message::Attachment(ref a)
            if a.kind == coco_types::AttachmentKind::CriticalSystemReminder
    ));
    let text = extract_text_from_message(&msg);
    assert!(text.contains("test reminder"));
}

#[test]
fn test_extract_text_from_user_message() {
    let msg = crate::creation::create_user_message("hello");
    let text = extract_text_from_message(&msg);
    assert_eq!(text, "hello");
}
