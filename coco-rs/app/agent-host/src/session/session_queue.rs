use coco_query::QueuePriority;
use coco_query::QueuedCommand;
use coco_query::QueuedImage;
use coco_system_reminder::QueueOrigin;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::session_runtime::SessionHandle;

pub struct EnqueuedCommand {
    pub id: uuid::Uuid,
    pub preview: String,
    pub editable: bool,
}

pub struct QueuedCommandForEdit {
    pub id: String,
    pub prompt: String,
    pub images: Vec<coco_types::QueuedCommandEditImage>,
    pub composer: coco_types::SubmittedComposer,
    pub original: QueuedCommand,
}

pub struct QueuedCommandsForEdit {
    pub ids: Vec<String>,
    pub prompt: String,
    pub images: Vec<coco_types::QueuedCommandEditImage>,
    pub composer: coco_types::SubmittedComposer,
    pub remaining_queued: usize,
    pub originals: Vec<QueuedCommand>,
}

pub struct DequeuedPromptBatch {
    pub ids: Vec<String>,
    pub messages: Vec<Arc<coco_messages::Message>>,
    pub remaining_queued: usize,
}

pub struct DequeuedSlashCommand {
    pub id: String,
    pub prompt: String,
    pub images: Vec<QueuedImage>,
    pub remaining_queued: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum QueuedCommandEditError {
    #[error("invalid queued command id")]
    InvalidId,
    #[error("queued command was already processed")]
    AlreadyProcessed,
    #[error("no editable queued commands")]
    NoEditableCommands,
}

pub async fn enqueue_human_prompt(
    session: &SessionHandle,
    prompt: String,
    images: Vec<QueuedImage>,
    composer: coco_types::SubmittedComposer,
) -> Option<EnqueuedCommand> {
    if prompt.trim().is_empty() && images.is_empty() {
        return None;
    }
    let queued = human_queued_command(prompt, images, composer);
    let result = EnqueuedCommand {
        id: queued.id,
        preview: queued.preview(),
        editable: queued.is_editable_by_user(),
    };
    session.command_queue().enqueue(queued).await;
    Some(result)
}

pub fn human_queued_command(
    prompt: String,
    images: Vec<QueuedImage>,
    composer: coco_types::SubmittedComposer,
) -> QueuedCommand {
    QueuedCommand::new(prompt, QueuePriority::Next)
        .with_origin(QueueOrigin::Human)
        .with_images(images)
        .with_editable_composer(composer)
}

pub fn coordinator_queued_command(content: String) -> QueuedCommand {
    QueuedCommand::new(content, QueuePriority::Later).with_origin(QueueOrigin::Coordinator)
}

pub async fn enqueue_coordinator_message(session: &SessionHandle, content: String) {
    if content.trim().is_empty() {
        return;
    }
    session
        .command_queue()
        .enqueue(coordinator_queued_command(content))
        .await;
}

pub fn cron_queued_command(prompt: String) -> QueuedCommand {
    QueuedCommand::new(prompt, QueuePriority::Later).with_origin(QueueOrigin::Cron)
}

pub async fn enqueue_cron_prompt(session: &SessionHandle, prompt: String) {
    if prompt.trim().is_empty() {
        return;
    }
    session
        .command_queue()
        .enqueue(cron_queued_command(prompt))
        .await;
}

pub async fn wait_for_command_queue_change(session: &SessionHandle) {
    session.command_queue().wait_for_change().await;
}

pub async fn dequeue_next_prompt_batch(
    session: &SessionHandle,
    event_tx: Option<mpsc::Sender<coco_types::CoreEvent>>,
) -> Option<DequeuedPromptBatch> {
    let first = session
        .command_queue()
        .dequeue_first_matching(|command| !command.is_slash_command && command.agent_id.is_none())
        .await?;
    let first_priority = first.priority;
    let first_origin = first.origin.clone();
    let mut queued = vec![first];
    let mut rest = session
        .command_queue()
        .dequeue_matching(|command| {
            !command.is_slash_command
                && command.agent_id.is_none()
                && command.priority == first_priority
                && command.origin == first_origin
        })
        .await;
    queued.append(&mut rest);

    let ids: Vec<String> = queued
        .iter()
        .map(|command| command.id.to_string())
        .collect();
    let messages = session
        .append_messages_to_history_and_emit(
            queued
                .iter()
                .map(coco_query::queued_command_to_message)
                .collect(),
            event_tx,
        )
        .await;
    Some(DequeuedPromptBatch {
        ids,
        messages,
        remaining_queued: session.command_queue().len().await,
    })
}

pub async fn dequeue_next_slash_command(session: &SessionHandle) -> Option<DequeuedSlashCommand> {
    let command = session
        .command_queue()
        .dequeue_first_matching(|command| command.is_slash_command && command.agent_id.is_none())
        .await?;
    Some(DequeuedSlashCommand {
        id: command.id.to_string(),
        prompt: command.prompt,
        images: command.images,
        remaining_queued: session.command_queue().len().await,
    })
}

pub async fn remove_queued_command_for_edit(
    session: &SessionHandle,
    id: &str,
) -> Result<QueuedCommandForEdit, QueuedCommandEditError> {
    let uuid = uuid::Uuid::parse_str(id).map_err(|_| QueuedCommandEditError::InvalidId)?;
    let queued = session
        .command_queue()
        .remove_by_id(uuid)
        .await
        .ok_or(QueuedCommandEditError::AlreadyProcessed)?;
    Ok(QueuedCommandForEdit {
        id: queued.id.to_string(),
        prompt: queued.prompt.clone(),
        images: queued_images_to_wire(queued.images.clone()),
        composer: queued.editable_composer.clone(),
        original: queued,
    })
}

pub async fn dequeue_editable_commands_for_edit(
    session: &SessionHandle,
) -> Result<QueuedCommandsForEdit, QueuedCommandEditError> {
    let queued = session.command_queue().dequeue_all_editable().await;
    if queued.is_empty() {
        return Err(QueuedCommandEditError::NoEditableCommands);
    }
    let remaining_queued = session.command_queue().len().await;
    let mut response = queued_commands_for_edit(&queued, remaining_queued);
    response.originals = queued;
    Ok(response)
}

fn queued_commands_for_edit(
    queued: &[QueuedCommand],
    remaining_queued: usize,
) -> QueuedCommandsForEdit {
    let ids: Vec<String> = queued.iter().map(|cmd| cmd.id.to_string()).collect();
    let mut queued_text = String::new();
    let mut images = Vec::new();
    let mut composer = coco_types::SubmittedComposer::default();
    for cmd in queued {
        if !queued_text.is_empty() {
            queued_text.push('\n');
        }
        let base = i64::try_from(queued_text.len()).unwrap_or(i64::MAX);
        let image_base = i64::try_from(images.len()).unwrap_or(i64::MAX);
        queued_text.push_str(&cmd.prompt);
        images.extend(cmd.images.iter().cloned().map(|mut image| {
            image.insertion_offset = image.insertion_offset.checked_add(base).unwrap_or(i64::MAX);
            image
        }));
        composer.next_attachment_label = composer
            .next_attachment_label
            .max(cmd.editable_composer.next_attachment_label);
        composer
            .elements
            .extend(
                cmd.editable_composer
                    .elements
                    .iter()
                    .cloned()
                    .map(|mut element| {
                        match &mut element {
                            coco_types::SubmittedComposerElement::Paste { start, end, .. }
                            | coco_types::SubmittedComposerElement::FileRef { start, end } => {
                                *start = start.checked_add(base).unwrap_or(i64::MAX);
                                *end = end.checked_add(base).unwrap_or(i64::MAX);
                            }
                            coco_types::SubmittedComposerElement::Image {
                                insertion_offset,
                                image_index,
                                ..
                            } => {
                                *insertion_offset =
                                    insertion_offset.checked_add(base).unwrap_or(i64::MAX);
                                *image_index =
                                    image_index.checked_add(image_base).unwrap_or(i64::MAX);
                            }
                        }
                        element
                    }),
            );
    }
    QueuedCommandsForEdit {
        ids,
        prompt: queued_text,
        images: queued_images_to_wire(images),
        composer,
        remaining_queued,
        originals: Vec::new(),
    }
}

fn queued_images_to_wire(images: Vec<QueuedImage>) -> Vec<coco_types::QueuedCommandEditImage> {
    images
        .into_iter()
        .map(|image| coco_types::QueuedCommandEditImage {
            media_type: image.media_type,
            data_base64: image.data_base64,
            insertion_offset: image.insertion_offset,
        })
        .collect()
}

#[cfg(test)]
#[path = "session_queue.test.rs"]
mod tests;
