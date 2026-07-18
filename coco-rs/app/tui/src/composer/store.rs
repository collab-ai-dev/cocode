use base64::Engine as _;

use super::*;

impl AttachmentStore {
    pub(crate) fn next_label(&self) -> i64 {
        self.next_label
    }

    pub(crate) fn starting_after(next_label: i64) -> Self {
        Self {
            entries: HashMap::new(),
            next_label,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(crate) fn insert_text(
        &mut self,
        textarea: &mut TextArea,
        content: String,
    ) -> Result<String, ElementError> {
        self.prune(textarea);
        let label = self.allocate_label(ElementKind::Paste)?;
        let id = textarea.insert_element(&label, ElementKind::Paste, paste_chip_display(&label))?;
        self.entries
            .insert(id, AttachmentPayload::Text(Arc::from(content)));
        Ok(label)
    }

    pub(crate) fn insert_image(
        &mut self,
        textarea: &mut TextArea,
        bytes: impl Into<Arc<[u8]>>,
        mime: String,
    ) -> Result<String, ElementError> {
        self.prune(textarea);
        let label = self.allocate_label(ElementKind::Image)?;
        let id = textarea.insert_element(&label, ElementKind::Image, paste_chip_display(&label))?;
        self.entries.insert(
            id,
            AttachmentPayload::Image {
                bytes: bytes.into(),
                mime,
            },
        );
        Ok(label)
    }

    pub(crate) fn prune(&mut self, textarea: &TextArea) {
        let retained: HashSet<_> = textarea.retained_element_ids().into_iter().collect();
        self.entries.retain(|id, _| retained.contains(id));
    }

    pub(crate) fn resolve(&self, textarea: &TextArea) -> Result<ResolvedInput, ResolveError> {
        let input = textarea.text();
        let mut text = String::with_capacity(input.len());
        let mut images = Vec::new();
        let mut submitted_elements = Vec::new();
        let mut source_pos = 0usize;

        for element in textarea.elements() {
            let range = element.range();
            if range.start < source_pos
                || range.end > input.len()
                || !input.is_char_boundary(range.start)
                || !input.is_char_boundary(range.end)
            {
                return Err(ResolveError::InvalidElementRange(element.id()));
            }
            let plain = input
                .get(source_pos..range.start)
                .ok_or(ResolveError::InvalidElementRange(element.id()))?;
            text.push_str(plain);
            match element.kind() {
                ElementKind::Paste => match self.entries.get(&element.id()) {
                    Some(AttachmentPayload::Text(content)) => {
                        let start = text.len();
                        text.push_str(content);
                        submitted_elements.push(coco_types::SubmittedComposerElement::Paste {
                            start: wire_offset(start)?,
                            end: wire_offset(text.len())?,
                            label: input[range.clone()].to_string(),
                        });
                    }
                    Some(AttachmentPayload::Image { .. }) => {
                        return Err(ResolveError::PayloadKindMismatch(element.id()));
                    }
                    None => return Err(ResolveError::MissingPayload(element.id())),
                },
                ElementKind::Image => match self.entries.get(&element.id()) {
                    Some(AttachmentPayload::Image { bytes, mime }) => {
                        let image_index = images.len();
                        images.push(ImageData {
                            bytes: Arc::clone(bytes),
                            mime: mime.clone(),
                            insertion_offset: text.len(),
                        });
                        submitted_elements.push(coco_types::SubmittedComposerElement::Image {
                            insertion_offset: wire_offset(text.len())?,
                            image_index: wire_offset(image_index)?,
                            label: input[range.clone()].to_string(),
                        });
                    }
                    Some(AttachmentPayload::Text(_)) => {
                        return Err(ResolveError::PayloadKindMismatch(element.id()));
                    }
                    None => return Err(ResolveError::MissingPayload(element.id())),
                },
                ElementKind::FileRef => {
                    let start = text.len();
                    let source = input
                        .get(range.clone())
                        .ok_or(ResolveError::InvalidElementRange(element.id()))?;
                    text.push_str(source);
                    submitted_elements.push(coco_types::SubmittedComposerElement::FileRef {
                        start: wire_offset(start)?,
                        end: wire_offset(text.len())?,
                    });
                }
            }
            source_pos = range.end;
        }
        let tail = input
            .get(source_pos..)
            .ok_or(ResolveError::InvalidTextRange)?;
        text.push_str(tail);
        Ok(ResolvedInput {
            text,
            images,
            submitted: coco_types::SubmittedComposer {
                next_attachment_label: self.next_label,
                elements: submitted_elements,
            },
        })
    }

    pub(crate) fn persisted(
        &self,
        textarea: &TextArea,
    ) -> Result<coco_types::PersistedComposer, ResolveError> {
        let input = textarea.text();
        let mut text = String::with_capacity(input.len());
        let mut elements = Vec::new();
        let mut source_pos = 0usize;
        for element in textarea.elements() {
            let range = element.range();
            if range.start < source_pos || range.end > input.len() {
                return Err(ResolveError::InvalidElementRange(element.id()));
            }
            text.push_str(
                input
                    .get(source_pos..range.start)
                    .ok_or(ResolveError::InvalidElementRange(element.id()))?,
            );
            let start = text.len();
            text.push_str(
                input
                    .get(range.clone())
                    .ok_or(ResolveError::InvalidElementRange(element.id()))?,
            );
            let start = wire_offset(start)?;
            let end = wire_offset(text.len())?;
            let persisted = match element.kind() {
                ElementKind::Paste => match self.entries.get(&element.id()) {
                    Some(AttachmentPayload::Text(content)) => {
                        coco_types::PersistedComposerElement::Paste {
                            start,
                            end,
                            content: content.to_string(),
                        }
                    }
                    Some(AttachmentPayload::Image { .. }) => {
                        return Err(ResolveError::PayloadKindMismatch(element.id()));
                    }
                    None => return Err(ResolveError::MissingPayload(element.id())),
                },
                ElementKind::Image => match self.entries.get(&element.id()) {
                    Some(AttachmentPayload::Image { bytes, mime }) => {
                        coco_types::PersistedComposerElement::Image {
                            start,
                            end,
                            media_type: mime.clone(),
                            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                        }
                    }
                    Some(AttachmentPayload::Text(_)) => {
                        return Err(ResolveError::PayloadKindMismatch(element.id()));
                    }
                    None => return Err(ResolveError::MissingPayload(element.id())),
                },
                ElementKind::FileRef => {
                    coco_types::PersistedComposerElement::FileRef { start, end }
                }
            };
            elements.push(persisted);
            source_pos = range.end;
        }
        text.push_str(
            input
                .get(source_pos..)
                .ok_or(ResolveError::InvalidTextRange)?,
        );
        Ok(coco_types::PersistedComposer {
            text,
            next_attachment_label: self.next_label,
            elements,
        })
    }

    fn allocate_label(&mut self, kind: ElementKind) -> Result<String, ElementError> {
        let next = self
            .next_label
            .checked_add(1)
            .ok_or(ElementError::IdExhausted)?;
        self.next_label = next;
        match kind {
            ElementKind::Paste => Ok(format!("[Pasted text #{next}]")),
            ElementKind::Image => Ok(format!("[Image #{next}]")),
            ElementKind::FileRef => Err(ElementError::InvalidRange),
        }
    }
}

fn wire_offset(value: usize) -> Result<i64, ResolveError> {
    i64::try_from(value).map_err(|_| ResolveError::OffsetOverflow)
}
