//! App-owned composer attachments and atomic snapshot lifecycle.

use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

use base64::Engine as _;
use coco_tui_ui::widgets::ElementDisplay;
use coco_tui_ui::widgets::ElementError;
use coco_tui_ui::widgets::ElementId;
use coco_tui_ui::widgets::ElementKind;
use coco_tui_ui::widgets::TextArea;
use coco_tui_ui::widgets::TextAreaSnapshot;
use ratatui::style::Modifier;
use ratatui::style::Style;

mod external_editor;
mod message_metadata;
mod store;

#[cfg(test)]
use external_editor::ExternalEditorError;
pub(crate) use external_editor::ExternalEditorSession;
pub(crate) use message_metadata::images_from_user_message;
pub(crate) use message_metadata::submitted_composer_for_restored_text;
pub(crate) use message_metadata::submitted_composer_from_user_message;

pub const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    pub bytes: Arc<[u8]>,
    pub mime: String,
    /// Byte position in the resolved prompt where the image element appeared.
    pub insertion_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AttachmentPayload {
    Text(Arc<str>),
    Image { bytes: Arc<[u8]>, mime: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct AttachmentStore {
    entries: HashMap<ElementId, AttachmentPayload>,
    next_label: i64,
}

#[derive(Debug, Default)]
pub(crate) struct Composer {
    textarea: TextArea,
    attachments: AttachmentStore,
}

pub(crate) struct ComposerTextAreaMut<'a> {
    textarea: &'a mut TextArea,
    attachments: &'a mut AttachmentStore,
}

impl std::ops::Deref for ComposerTextAreaMut<'_> {
    type Target = TextArea;

    fn deref(&self) -> &Self::Target {
        self.textarea
    }
}

impl std::ops::DerefMut for ComposerTextAreaMut<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.textarea
    }
}

impl Drop for ComposerTextAreaMut<'_> {
    fn drop(&mut self) {
        self.attachments.prune(self.textarea);
    }
}

impl Composer {
    pub(crate) fn textarea(&self) -> &TextArea {
        &self.textarea
    }

    pub(crate) fn textarea_mut(&mut self) -> ComposerTextAreaMut<'_> {
        ComposerTextAreaMut {
            textarea: &mut self.textarea,
            attachments: &mut self.attachments,
        }
    }

    pub(crate) fn set_text(&mut self, text: &str) {
        self.textarea.set_text(text);
        self.attachments.clear();
    }

    pub(crate) fn take_text(&mut self) -> String {
        self.attachments.clear();
        self.textarea.take_text()
    }

    pub(crate) fn snapshot(&self) -> ComposerSnapshot {
        ComposerSnapshot::new(self.textarea.snapshot(), self.attachments.clone())
    }

    pub(crate) fn take_snapshot(&mut self) -> ComposerSnapshot {
        ComposerSnapshot::new(
            self.textarea.take_snapshot(),
            std::mem::take(&mut self.attachments),
        )
    }

    pub(crate) fn take_snapshot_preserving_labels(&mut self) -> ComposerSnapshot {
        let next_label = self.attachments.next_label();
        let snapshot = self.take_snapshot();
        self.attachments = AttachmentStore::starting_after(next_label);
        snapshot
    }

    pub(crate) fn restore(&mut self, snapshot: ComposerSnapshot) {
        let (textarea, attachments) = snapshot.into_parts();
        self.textarea.restore_snapshot(textarea);
        self.attachments = attachments;
        self.attachments.prune(&self.textarea);
    }

    pub(crate) fn resolve(&self) -> Result<ResolvedInput, ResolveError> {
        self.attachments.resolve(&self.textarea)
    }

    pub(crate) fn persisted(&self) -> Result<coco_types::PersistedComposer, ResolveError> {
        self.attachments.persisted(&self.textarea)
    }

    pub(crate) fn prune(&mut self) {
        self.attachments.prune(&self.textarea);
    }

    pub(crate) fn insert_text(&mut self, content: String) -> Result<String, ElementError> {
        self.attachments.insert_text(&mut self.textarea, content)
    }

    pub(crate) fn insert_image(
        &mut self,
        bytes: impl Into<Arc<[u8]>>,
        mime: String,
    ) -> Result<String, ElementError> {
        self.attachments
            .insert_image(&mut self.textarea, bytes, mime)
    }

    #[cfg(test)]
    pub(crate) fn attachment_count(&self) -> usize {
        self.attachments.entries.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerSnapshot {
    textarea: TextAreaSnapshot,
    attachments: AttachmentStore,
}

impl ComposerSnapshot {
    pub(crate) fn plain(text: String, cursor: usize) -> Self {
        let mut textarea = TextArea::new();
        textarea.set_text(&text);
        textarea.set_cursor(cursor);
        Self::new(textarea.take_snapshot(), AttachmentStore::default())
    }

    pub(crate) fn text(&self) -> &str {
        self.textarea.text()
    }

    pub(crate) fn cursor(&self) -> usize {
        self.textarea.cursor()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.textarea.text().is_empty()
    }

    #[cfg(test)]
    pub(crate) fn attachments(&self) -> &AttachmentStore {
        &self.attachments
    }

    pub(crate) fn next_attachment_label(&self) -> i64 {
        self.attachments.next_label
    }

    pub(crate) fn into_parts(self) -> (TextAreaSnapshot, AttachmentStore) {
        (self.textarea, self.attachments)
    }

    pub(crate) fn new(textarea: TextAreaSnapshot, attachments: AttachmentStore) -> Self {
        Self {
            textarea,
            attachments,
        }
    }

    pub(crate) fn from_persisted(
        persisted: coco_types::PersistedComposer,
    ) -> Result<Self, ComposerBuildError> {
        if persisted.next_attachment_label < 0 {
            return Err(ComposerBuildError::InvalidOffset);
        }
        let mut textarea = TextArea::new();
        textarea.set_text(&persisted.text);
        textarea.set_cursor(persisted.text.len());
        let mut attachments = AttachmentStore {
            entries: HashMap::new(),
            next_label: persisted.next_attachment_label,
        };
        for element in persisted.elements {
            match element {
                coco_types::PersistedComposerElement::Paste {
                    start,
                    end,
                    content,
                } => {
                    let range = persisted_range(start, end)?;
                    let source = textarea
                        .text()
                        .get(range.clone())
                        .ok_or(ElementError::InvalidRange)?
                        .to_string();
                    validate_attachment_label(
                        &source,
                        "[Pasted text #",
                        persisted.next_attachment_label,
                    )?;
                    let id = textarea.register_element(
                        range,
                        ElementKind::Paste,
                        paste_chip_display(&source),
                    )?;
                    attachments
                        .entries
                        .insert(id, AttachmentPayload::Text(Arc::from(content)));
                }
                coco_types::PersistedComposerElement::Image {
                    start,
                    end,
                    media_type,
                    data_base64,
                } => {
                    let range = persisted_range(start, end)?;
                    let source = textarea
                        .text()
                        .get(range.clone())
                        .ok_or(ElementError::InvalidRange)?
                        .to_string();
                    validate_attachment_label(
                        &source,
                        "[Image #",
                        persisted.next_attachment_label,
                    )?;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data_base64)
                        .map_err(|_| ComposerBuildError::InvalidBase64)?;
                    let id = textarea.register_element(
                        range,
                        ElementKind::Image,
                        paste_chip_display(&source),
                    )?;
                    attachments.entries.insert(
                        id,
                        AttachmentPayload::Image {
                            bytes: Arc::from(bytes),
                            mime: media_type,
                        },
                    );
                }
                coco_types::PersistedComposerElement::FileRef { start, end } => {
                    let range = persisted_range(start, end)?;
                    let source = textarea
                        .text()
                        .get(range.clone())
                        .ok_or(ElementError::InvalidRange)?
                        .to_string();
                    textarea.register_element(
                        range,
                        ElementKind::FileRef,
                        file_ref_display(&source),
                    )?;
                }
            }
        }
        Ok(Self::new(textarea.take_snapshot(), attachments))
    }

    pub(crate) fn from_queued_edit(
        prompt: String,
        mut images: Vec<coco_types::QueuedCommandEditImage>,
        initial_label: i64,
    ) -> Result<Self, ComposerBuildError> {
        if initial_label < 0 {
            return Err(ComposerBuildError::InvalidOffset);
        }
        let mut textarea = TextArea::new();
        textarea.set_text(&prompt);
        let mut attachments = AttachmentStore {
            entries: HashMap::new(),
            next_label: initial_label,
        };
        images.sort_by_key(|image| image.insertion_offset);
        let mut inserted_bytes = 0usize;
        for image in images {
            let offset = usize::try_from(image.insertion_offset)
                .map_err(|_| ComposerBuildError::InvalidOffset)?;
            if offset > prompt.len() || !prompt.is_char_boundary(offset) {
                return Err(ComposerBuildError::InvalidOffset);
            }
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(image.data_base64)
                .map_err(|_| ComposerBuildError::InvalidBase64)?;
            let restored_offset = offset
                .checked_add(inserted_bytes)
                .ok_or(ComposerBuildError::InvalidOffset)?;
            textarea.set_cursor(restored_offset);
            let label = attachments.insert_image(&mut textarea, bytes, image.media_type)?;
            inserted_bytes = inserted_bytes
                .checked_add(label.len())
                .ok_or(ComposerBuildError::InvalidOffset)?;
        }
        textarea.set_cursor(textarea.text().len());
        Ok(Self::new(textarea.take_snapshot(), attachments))
    }

    pub(crate) fn from_submitted(
        prompt: String,
        images: Vec<coco_types::QueuedCommandEditImage>,
        submitted: coco_types::SubmittedComposer,
    ) -> Result<Self, ComposerBuildError> {
        if !submitted.is_valid_for(&prompt, images.len()) {
            return Err(ComposerBuildError::InvalidOffset);
        }
        let mut source_pos = 0usize;
        let mut restored_text = String::with_capacity(prompt.len());
        let mut persisted_elements = Vec::with_capacity(submitted.elements.len());
        let mut used_images = HashSet::new();
        for element in submitted.elements {
            match element {
                coco_types::SubmittedComposerElement::Paste { start, end, label } => {
                    let range = persisted_range(start, end)?;
                    if range.start < source_pos {
                        return Err(ComposerBuildError::InvalidOffset);
                    }
                    let content = prompt
                        .get(range.clone())
                        .ok_or(ComposerBuildError::InvalidOffset)?;
                    restored_text.push_str(
                        prompt
                            .get(source_pos..range.start)
                            .ok_or(ComposerBuildError::InvalidOffset)?,
                    );
                    let restored_start = restored_text.len();
                    restored_text.push_str(&label);
                    let restored_end = restored_text.len();
                    persisted_elements.push(coco_types::PersistedComposerElement::Paste {
                        start: i64::try_from(restored_start)
                            .map_err(|_| ComposerBuildError::InvalidOffset)?,
                        end: i64::try_from(restored_end)
                            .map_err(|_| ComposerBuildError::InvalidOffset)?,
                        content: content.to_string(),
                    });
                    source_pos = range.end;
                }
                coco_types::SubmittedComposerElement::Image {
                    insertion_offset,
                    image_index,
                    label,
                } => {
                    let offset = usize::try_from(insertion_offset)
                        .map_err(|_| ComposerBuildError::InvalidOffset)?;
                    if offset < source_pos
                        || offset > prompt.len()
                        || !prompt.is_char_boundary(offset)
                    {
                        return Err(ComposerBuildError::InvalidOffset);
                    }
                    restored_text.push_str(
                        prompt
                            .get(source_pos..offset)
                            .ok_or(ComposerBuildError::InvalidOffset)?,
                    );
                    source_pos = offset;
                    let image_index = usize::try_from(image_index)
                        .map_err(|_| ComposerBuildError::InvalidOffset)?;
                    if !used_images.insert(image_index) {
                        return Err(ComposerBuildError::InvalidOffset);
                    }
                    let image = images
                        .get(image_index)
                        .ok_or(ComposerBuildError::InvalidOffset)?;
                    if image.insertion_offset != insertion_offset {
                        return Err(ComposerBuildError::InvalidOffset);
                    }
                    let restored_start = restored_text.len();
                    restored_text.push_str(&label);
                    let restored_end = restored_text.len();
                    persisted_elements.push(coco_types::PersistedComposerElement::Image {
                        start: i64::try_from(restored_start)
                            .map_err(|_| ComposerBuildError::InvalidOffset)?,
                        end: i64::try_from(restored_end)
                            .map_err(|_| ComposerBuildError::InvalidOffset)?,
                        media_type: image.media_type.clone(),
                        data_base64: image.data_base64.clone(),
                    });
                }
                coco_types::SubmittedComposerElement::FileRef { start, end } => {
                    let range = persisted_range(start, end)?;
                    if range.start < source_pos {
                        return Err(ComposerBuildError::InvalidOffset);
                    }
                    restored_text.push_str(
                        prompt
                            .get(source_pos..range.end)
                            .ok_or(ComposerBuildError::InvalidOffset)?,
                    );
                    let restored_end = restored_text.len();
                    let restored_start = restored_end
                        .checked_sub(range.len())
                        .ok_or(ComposerBuildError::InvalidOffset)?;
                    persisted_elements.push(coco_types::PersistedComposerElement::FileRef {
                        start: i64::try_from(restored_start)
                            .map_err(|_| ComposerBuildError::InvalidOffset)?,
                        end: i64::try_from(restored_end)
                            .map_err(|_| ComposerBuildError::InvalidOffset)?,
                    });
                    source_pos = range.end;
                }
            }
        }
        if used_images.len() != images.len() {
            return Err(ComposerBuildError::InvalidOffset);
        }
        restored_text.push_str(
            prompt
                .get(source_pos..)
                .ok_or(ComposerBuildError::InvalidOffset)?,
        );
        Self::from_persisted(coco_types::PersistedComposer {
            text: restored_text,
            next_attachment_label: submitted.next_attachment_label,
            elements: persisted_elements,
        })
    }

    pub(crate) fn merged_with(self, suffix: Self) -> Result<Self, ComposerBuildError> {
        let suffix_cursor = suffix.cursor();
        let mut prefix = snapshot_to_persisted(&self)?;
        let mut suffix = snapshot_to_persisted(&suffix)?;
        let suffix_cursor =
            relabel_suffix_for_merge(&mut suffix, prefix.next_attachment_label, suffix_cursor)?;
        let separator = usize::from(!prefix.text.is_empty() && !suffix.text.is_empty());
        let suffix_offset = prefix
            .text
            .len()
            .checked_add(separator)
            .ok_or(ComposerBuildError::InvalidOffset)?;
        if separator == 1 {
            prefix.text.push('\n');
        }
        prefix.text.push_str(&suffix.text);
        for element in &mut suffix.elements {
            offset_persisted_element(element, suffix_offset)?;
        }
        prefix.next_attachment_label = prefix
            .next_attachment_label
            .max(suffix.next_attachment_label);
        prefix.elements.extend(suffix.elements);
        let cursor = suffix_offset
            .checked_add(suffix_cursor)
            .ok_or(ComposerBuildError::InvalidOffset)?;
        let mut merged = Self::from_persisted(prefix)?;
        merged.textarea = merged.textarea.with_cursor(cursor)?;
        Ok(merged)
    }
}

#[derive(Debug)]
pub(crate) enum ComposerBuildError {
    Element(ElementError),
    Resolve(ResolveError),
    InvalidBase64,
    InvalidOffset,
    InvalidAttachmentLabel,
}

impl std::fmt::Display for ComposerBuildError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Element(error) => write!(formatter, "invalid composer element: {error:?}"),
            Self::Resolve(error) => write!(formatter, "invalid composer payload: {error}"),
            Self::InvalidBase64 => formatter.write_str("invalid base64 image payload"),
            Self::InvalidOffset => formatter.write_str("invalid composer byte offset"),
            Self::InvalidAttachmentLabel => {
                formatter.write_str("invalid composer attachment label")
            }
        }
    }
}

impl std::error::Error for ComposerBuildError {}

impl From<ElementError> for ComposerBuildError {
    fn from(error: ElementError) -> Self {
        Self::Element(error)
    }
}

impl From<ResolveError> for ComposerBuildError {
    fn from(error: ResolveError) -> Self {
        Self::Resolve(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedInput {
    pub text: String,
    pub images: Vec<ImageData>,
    pub submitted: coco_types::SubmittedComposer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolveError {
    MissingPayload(ElementId),
    PayloadKindMismatch(ElementId),
    InvalidElementRange(ElementId),
    InvalidTextRange,
    OffsetOverflow,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPayload(_) => {
                formatter.write_str("composer element is missing its payload")
            }
            Self::PayloadKindMismatch(_) => {
                formatter.write_str("composer element payload kind does not match")
            }
            Self::InvalidElementRange(_) => {
                formatter.write_str("composer element range is invalid")
            }
            Self::InvalidTextRange => formatter.write_str("composer text range is invalid"),
            Self::OffsetOverflow => formatter.write_str("composer offset exceeds wire limits"),
        }
    }
}

impl std::error::Error for ResolveError {}

fn persisted_range(start: i64, end: i64) -> Result<Range<usize>, ElementError> {
    let start = usize::try_from(start).map_err(|_| ElementError::InvalidRange)?;
    let end = usize::try_from(end).map_err(|_| ElementError::InvalidRange)?;
    Ok(start..end)
}

fn validate_attachment_label(
    source: &str,
    prefix: &str,
    next_label: i64,
) -> Result<(), ComposerBuildError> {
    attachment_label_number(source, prefix)
        .filter(|label| *label <= next_label)
        .ok_or(ComposerBuildError::InvalidAttachmentLabel)?;
    Ok(())
}

fn attachment_label_number(source: &str, prefix: &str) -> Option<i64> {
    source
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|label| *label > 0)
}

fn snapshot_to_persisted(
    snapshot: &ComposerSnapshot,
) -> Result<coco_types::PersistedComposer, ResolveError> {
    let mut textarea = TextArea::new();
    textarea.restore_snapshot(snapshot.textarea.clone());
    snapshot.attachments.persisted(&textarea)
}

fn offset_persisted_element(
    element: &mut coco_types::PersistedComposerElement,
    offset: usize,
) -> Result<(), ComposerBuildError> {
    let offset = i64::try_from(offset).map_err(|_| ComposerBuildError::InvalidOffset)?;
    let (start, end) = match element {
        coco_types::PersistedComposerElement::Paste { start, end, .. }
        | coco_types::PersistedComposerElement::Image { start, end, .. }
        | coco_types::PersistedComposerElement::FileRef { start, end } => (start, end),
    };
    *start = start
        .checked_add(offset)
        .ok_or(ComposerBuildError::InvalidOffset)?;
    *end = end
        .checked_add(offset)
        .ok_or(ComposerBuildError::InvalidOffset)?;
    Ok(())
}

fn relabel_suffix_for_merge(
    suffix: &mut coco_types::PersistedComposer,
    prefix_next_label: i64,
    cursor: usize,
) -> Result<usize, ComposerBuildError> {
    let mut collision = false;
    for element in &suffix.elements {
        let (start, end, label_prefix) = match element {
            coco_types::PersistedComposerElement::Paste { start, end, .. } => {
                (*start, *end, "[Pasted text #")
            }
            coco_types::PersistedComposerElement::Image { start, end, .. } => {
                (*start, *end, "[Image #")
            }
            coco_types::PersistedComposerElement::FileRef { .. } => continue,
        };
        let range = persisted_range(start, end)?;
        let label = suffix
            .text
            .get(range)
            .ok_or(ComposerBuildError::InvalidOffset)?;
        let number = attachment_label_number(label, label_prefix)
            .ok_or(ComposerBuildError::InvalidAttachmentLabel)?;
        collision |= number <= prefix_next_label;
    }
    if !collision {
        return Ok(cursor);
    }

    let source = std::mem::take(&mut suffix.text);
    let source_elements = std::mem::take(&mut suffix.elements);
    let mut text = String::with_capacity(source.len());
    let mut elements = Vec::with_capacity(source_elements.len());
    let mut source_pos = 0usize;
    let mut next_label = prefix_next_label.max(suffix.next_attachment_label);
    let mut cursor_shift = 0usize;
    for mut element in source_elements {
        let (start, end) = match &element {
            coco_types::PersistedComposerElement::Paste { start, end, .. }
            | coco_types::PersistedComposerElement::Image { start, end, .. }
            | coco_types::PersistedComposerElement::FileRef { start, end } => (*start, *end),
        };
        let range = persisted_range(start, end)?;
        text.push_str(
            source
                .get(source_pos..range.start)
                .ok_or(ComposerBuildError::InvalidOffset)?,
        );
        let restored_start = text.len();
        match &element {
            coco_types::PersistedComposerElement::Paste { .. } => {
                next_label = next_label
                    .checked_add(1)
                    .ok_or(ComposerBuildError::InvalidOffset)?;
                text.push_str(&format!("[Pasted text #{next_label}]"));
            }
            coco_types::PersistedComposerElement::Image { .. } => {
                next_label = next_label
                    .checked_add(1)
                    .ok_or(ComposerBuildError::InvalidOffset)?;
                text.push_str(&format!("[Image #{next_label}]"));
            }
            coco_types::PersistedComposerElement::FileRef { .. } => text.push_str(
                source
                    .get(range.clone())
                    .ok_or(ComposerBuildError::InvalidOffset)?,
            ),
        }
        let restored_end = text.len();
        if cursor >= range.end {
            let growth = restored_end
                .checked_sub(restored_start)
                .and_then(|new_len| new_len.checked_sub(range.len()))
                .ok_or(ComposerBuildError::InvalidOffset)?;
            cursor_shift = cursor_shift
                .checked_add(growth)
                .ok_or(ComposerBuildError::InvalidOffset)?;
        }
        set_persisted_element_range(&mut element, restored_start, restored_end)?;
        elements.push(element);
        source_pos = range.end;
    }
    text.push_str(
        source
            .get(source_pos..)
            .ok_or(ComposerBuildError::InvalidOffset)?,
    );
    suffix.text = text;
    suffix.elements = elements;
    suffix.next_attachment_label = next_label;
    cursor
        .checked_add(cursor_shift)
        .ok_or(ComposerBuildError::InvalidOffset)
}

fn set_persisted_element_range(
    element: &mut coco_types::PersistedComposerElement,
    start: usize,
    end: usize,
) -> Result<(), ComposerBuildError> {
    let start = i64::try_from(start).map_err(|_| ComposerBuildError::InvalidOffset)?;
    let end = i64::try_from(end).map_err(|_| ComposerBuildError::InvalidOffset)?;
    match element {
        coco_types::PersistedComposerElement::Paste {
            start: old_start,
            end: old_end,
            ..
        }
        | coco_types::PersistedComposerElement::Image {
            start: old_start,
            end: old_end,
            ..
        }
        | coco_types::PersistedComposerElement::FileRef {
            start: old_start,
            end: old_end,
        } => {
            *old_start = start;
            *old_end = end;
        }
    }
    Ok(())
}

pub(crate) fn file_ref_display(token: &str) -> ElementDisplay {
    ElementDisplay::new(token, Style::new().underlined())
}

pub(crate) fn paste_chip_display(label: &str) -> ElementDisplay {
    let text = label
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(label)
        .replacen("Pasted text", "Pasted", 1);
    ElementDisplay::new(
        format!(" {text} "),
        Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED),
    )
}

#[cfg(test)]
#[path = "composer.test.rs"]
mod tests;
