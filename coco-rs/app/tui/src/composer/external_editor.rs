use super::*;

#[derive(Debug, Clone)]
pub(crate) struct ExternalEditorSession {
    original: ComposerSnapshot,
    pub(super) elements: Vec<ExternalEditorElement>,
}

#[derive(Debug, Clone)]
pub(super) struct ExternalEditorElement {
    pub(super) marker: String,
    source: String,
    editor_text: String,
    payload: ExternalEditorPayload,
}

#[derive(Debug, Clone)]
enum ExternalEditorPayload {
    Paste(Arc<str>),
    Image { bytes: Arc<[u8]>, mime: String },
    FileRef,
}

#[derive(Debug)]
pub(crate) enum ExternalEditorError {
    Build(ComposerBuildError),
    Resolve(ResolveError),
    DuplicateElementMarker,
    InvalidElementMarker,
}

impl std::fmt::Display for ExternalEditorError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build(error) => write!(formatter, "{error}"),
            Self::Resolve(error) => write!(formatter, "{error}"),
            Self::DuplicateElementMarker => {
                formatter.write_str("an external-editor element marker was duplicated")
            }
            Self::InvalidElementMarker => {
                formatter.write_str("an external-editor element marker is invalid")
            }
        }
    }
}

impl std::error::Error for ExternalEditorError {}

impl From<ComposerBuildError> for ExternalEditorError {
    fn from(error: ComposerBuildError) -> Self {
        Self::Build(error)
    }
}

impl From<ResolveError> for ExternalEditorError {
    fn from(error: ResolveError) -> Self {
        Self::Resolve(error)
    }
}

impl ExternalEditorSession {
    pub(crate) fn prepare(
        original: ComposerSnapshot,
    ) -> Result<(Self, String), ExternalEditorError> {
        let text = original.textarea.text();
        let mut content = String::with_capacity(text.len());
        let mut elements = Vec::with_capacity(original.textarea.elements().len());
        let mut source_pos = 0usize;
        for element in original.textarea.elements() {
            let source = text
                .get(element.range().clone())
                .ok_or(ResolveError::InvalidElementRange(element.id()))?
                .to_string();
            content.push_str(
                text.get(source_pos..element.range().start)
                    .ok_or(ResolveError::InvalidElementRange(element.id()))?,
            );
            let (editor_text, payload) = match element.kind() {
                ElementKind::Paste => match original.attachments.entries.get(&element.id()) {
                    Some(AttachmentPayload::Text(payload)) => (
                        payload.to_string(),
                        ExternalEditorPayload::Paste(Arc::clone(payload)),
                    ),
                    Some(AttachmentPayload::Image { .. }) => {
                        return Err(ResolveError::PayloadKindMismatch(element.id()).into());
                    }
                    None => return Err(ResolveError::MissingPayload(element.id()).into()),
                },
                ElementKind::Image => match original.attachments.entries.get(&element.id()) {
                    Some(AttachmentPayload::Image { bytes, mime }) => (
                        String::new(),
                        ExternalEditorPayload::Image {
                            bytes: Arc::clone(bytes),
                            mime: mime.clone(),
                        },
                    ),
                    Some(AttachmentPayload::Text(_)) => {
                        return Err(ResolveError::PayloadKindMismatch(element.id()).into());
                    }
                    None => return Err(ResolveError::MissingPayload(element.id()).into()),
                },
                ElementKind::FileRef => (source.clone(), ExternalEditorPayload::FileRef),
            };
            let marker = format!("<!-- coco:element:{} -->", uuid::Uuid::new_v4());
            content.push_str(&editor_text);
            content.push_str(&marker);
            elements.push(ExternalEditorElement {
                marker,
                source,
                editor_text,
                payload,
            });
            source_pos = element.range().end;
        }
        content.push_str(
            text.get(source_pos..)
                .ok_or(ResolveError::InvalidTextRange)?,
        );
        Ok((Self { original, elements }, content))
    }

    pub(crate) fn finish(
        &self,
        content: String,
        modified: bool,
    ) -> Result<ComposerSnapshot, ExternalEditorError> {
        if !modified {
            return Ok(self.original.clone());
        }
        let mut found = Vec::new();
        for element in &self.elements {
            let positions = content
                .match_indices(&element.marker)
                .map(|(offset, _)| offset)
                .collect::<Vec<_>>();
            match positions.as_slice() {
                [] => {}
                [offset] => found.push((*offset, element)),
                _ => return Err(ExternalEditorError::DuplicateElementMarker),
            }
        }
        found.sort_by_key(|(offset, _)| *offset);
        let mut clean = String::with_capacity(content.len());
        let mut source = 0usize;
        let mut elements = Vec::with_capacity(found.len());
        for (offset, element) in found {
            if offset < source || !content.is_char_boundary(offset) {
                return Err(ExternalEditorError::InvalidElementMarker);
            }
            let representation_start = offset.saturating_sub(element.editor_text.len());
            let preserves_element = representation_start >= source
                && content.get(representation_start..offset) == Some(&element.editor_text);
            let copy_end = if preserves_element {
                representation_start
            } else {
                offset
            };
            clean.push_str(
                content
                    .get(source..copy_end)
                    .ok_or(ExternalEditorError::InvalidElementMarker)?,
            );
            if preserves_element {
                let start = i64::try_from(clean.len())
                    .map_err(|_| ExternalEditorError::InvalidElementMarker)?;
                clean.push_str(&element.source);
                let end = i64::try_from(clean.len())
                    .map_err(|_| ExternalEditorError::InvalidElementMarker)?;
                elements.push(match &element.payload {
                    ExternalEditorPayload::Paste(content) => {
                        coco_types::PersistedComposerElement::Paste {
                            start,
                            end,
                            content: content.to_string(),
                        }
                    }
                    ExternalEditorPayload::Image { bytes, mime } => {
                        coco_types::PersistedComposerElement::Image {
                            start,
                            end,
                            media_type: mime.clone(),
                            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                        }
                    }
                    ExternalEditorPayload::FileRef => {
                        coco_types::PersistedComposerElement::FileRef { start, end }
                    }
                });
            }
            source = offset
                .checked_add(element.marker.len())
                .ok_or(ExternalEditorError::InvalidElementMarker)?;
        }
        clean.push_str(
            content
                .get(source..)
                .ok_or(ExternalEditorError::InvalidElementMarker)?,
        );
        Ok(ComposerSnapshot::from_persisted(
            coco_types::PersistedComposer {
                text: clean,
                next_attachment_label: self.original.attachments.next_label,
                elements,
            },
        )?)
    }

    pub(crate) fn original(&self) -> ComposerSnapshot {
        self.original.clone()
    }
}
