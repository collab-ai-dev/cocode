use base64::Engine as _;

pub(crate) fn images_from_user_message(
    user: &coco_messages::UserMessage,
) -> Vec<coco_types::QueuedCommandEditImage> {
    let coco_messages::LlmMessage::User { content, .. } = &user.message else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|part| {
            let coco_messages::UserContent::File(file) = part else {
                return None;
            };
            if !file.media_type.starts_with("image/") {
                return None;
            }
            let bytes = file.data.as_data()?.to_bytes()?;
            let insertion_offset = file
                .provider_metadata
                .as_ref()
                .and_then(|metadata| metadata.get("coco_composer_insertion_offset"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(i64::MAX);
            Some(coco_types::QueuedCommandEditImage {
                media_type: file.media_type.clone(),
                data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                insertion_offset,
            })
        })
        .collect()
}

pub(crate) fn submitted_composer_from_user_message(
    user: &coco_messages::UserMessage,
) -> Option<coco_types::SubmittedComposer> {
    let coco_messages::LlmMessage::User { content, .. } = &user.message else {
        return None;
    };
    let original_text = content.iter().find_map(|part| match part {
        coco_messages::UserContent::Text(text) => Some(text.text.as_str()),
        coco_messages::UserContent::File(_) => None,
    })?;
    let image_count = content
        .iter()
        .filter(|part| {
            matches!(part, coco_messages::UserContent::File(file) if file.media_type.starts_with("image/"))
        })
        .count();
    content.iter().find_map(|part| {
        let coco_messages::UserContent::Text(text) = part else {
            return None;
        };
        let submitted: coco_types::SubmittedComposer = serde_json::from_value(
            text.provider_metadata
                .as_ref()?
                .get("coco_submitted_composer")?
                .clone(),
        )
        .ok()?;
        submitted
            .is_valid_for(original_text, image_count)
            .then_some(submitted)
    })
}

pub(crate) fn submitted_composer_for_restored_text(
    user: &coco_messages::UserMessage,
    restored_text: &str,
) -> Option<coco_types::SubmittedComposer> {
    let coco_messages::LlmMessage::User { content, .. } = &user.message else {
        return None;
    };
    let original_text = content.iter().find_map(|part| match part {
        coco_messages::UserContent::Text(text) => Some(text.text.as_str()),
        coco_messages::UserContent::File(_) => None,
    })?;
    let mut submitted = submitted_composer_from_user_message(user)?;
    if original_text == restored_text {
        return Some(submitted);
    }
    let restored_start = original_text.find(restored_text)?;
    let restored_end = restored_start.checked_add(restored_text.len())?;
    let image_count = submitted
        .elements
        .iter()
        .filter(|element| matches!(element, coco_types::SubmittedComposerElement::Image { .. }))
        .count();
    submitted.elements = submitted
        .elements
        .into_iter()
        .filter_map(|element| match element {
            coco_types::SubmittedComposerElement::Paste { start, end, label } => {
                let start = usize::try_from(start).ok()?;
                let end = usize::try_from(end).ok()?;
                if start < restored_start || end > restored_end {
                    return None;
                }
                Some(coco_types::SubmittedComposerElement::Paste {
                    start: i64::try_from(start - restored_start).ok()?,
                    end: i64::try_from(end - restored_start).ok()?,
                    label,
                })
            }
            coco_types::SubmittedComposerElement::Image {
                insertion_offset,
                image_index,
                label,
            } => {
                let offset = usize::try_from(insertion_offset).ok()?;
                if offset < restored_start || offset > restored_end {
                    return None;
                }
                Some(coco_types::SubmittedComposerElement::Image {
                    insertion_offset: i64::try_from(offset - restored_start).ok()?,
                    image_index,
                    label,
                })
            }
            coco_types::SubmittedComposerElement::FileRef { start, end } => {
                let start = usize::try_from(start).ok()?;
                let end = usize::try_from(end).ok()?;
                if start < restored_start || end > restored_end {
                    return None;
                }
                Some(coco_types::SubmittedComposerElement::FileRef {
                    start: i64::try_from(start - restored_start).ok()?,
                    end: i64::try_from(end - restored_start).ok()?,
                })
            }
        })
        .collect();
    if submitted
        .elements
        .iter()
        .filter(|element| matches!(element, coco_types::SubmittedComposerElement::Image { .. }))
        .count()
        != image_count
    {
        return None;
    }
    submitted
        .is_valid_for(restored_text, image_count)
        .then_some(submitted)
}
