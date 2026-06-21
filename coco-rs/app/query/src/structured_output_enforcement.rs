use coco_hooks::FunctionHookPredicate;

/// Stop enforcement predicate for StructuredOutput.
///
/// Returns true once history contains a schema-valid StructuredOutput
/// silent attachment. The tool only emits that attachment after runtime
/// schema validation succeeds.
#[derive(Debug)]
pub struct StructuredOutputEnforcement;

impl FunctionHookPredicate for StructuredOutputEnforcement {
    fn evaluate(&self, messages: &[std::sync::Arc<coco_messages::Message>]) -> bool {
        use coco_messages::AttachmentBody;
        use coco_messages::Message;
        use coco_messages::SilentPayload;
        use coco_types::AttachmentKind;

        messages.iter().any(|m| match m.as_ref() {
            Message::Attachment(att) => {
                att.kind == AttachmentKind::StructuredOutput
                    && matches!(
                        &att.body,
                        AttachmentBody::Silent(SilentPayload::StructuredOutput(_))
                    )
            }
            _ => false,
        })
    }

    fn name(&self) -> &str {
        "StructuredOutputEnforcement"
    }
}
