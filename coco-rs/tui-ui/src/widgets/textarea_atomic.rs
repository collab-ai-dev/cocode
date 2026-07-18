//! Atomic element lifecycle and display projection for [`super::TextArea`].

use super::*;

impl TextArea {
    /// Atomic elements in source order.
    pub fn elements(&self) -> &[TextElement] {
        &self.elements
    }

    /// Capture the complete editable buffer without its undo/redo stacks.
    pub fn snapshot(&self) -> TextAreaSnapshot {
        TextAreaSnapshot {
            text: self.text.clone(),
            cursor: self.cursor_pos,
            elements: self.elements.clone(),
        }
    }

    /// Move the complete editable buffer out and leave an empty textarea.
    pub fn take_snapshot(&mut self) -> TextAreaSnapshot {
        let snapshot = TextAreaSnapshot {
            text: std::mem::take(&mut self.text),
            cursor: self.cursor_pos,
            elements: std::mem::take(&mut self.elements),
        };
        self.cursor_pos = 0;
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        self.reset_edit_history();
        snapshot
    }

    /// Replace the complete editable buffer with a snapshot produced by this
    /// module. `TextAreaSnapshot` has no public fields or unchecked
    /// constructors, so validity is guaranteed at its type boundary.
    pub fn restore_snapshot(&mut self, snapshot: TextAreaSnapshot) {
        debug_assert!(validate_snapshot(&snapshot).is_ok());
        let next_id = snapshot
            .elements
            .iter()
            .map(|element| element.id.raw())
            .max()
            .and_then(|max_id| max_id.checked_add(1))
            .unwrap_or(i64::MAX)
            .max(self.next_element_id);
        self.text = snapshot.text;
        self.cursor_pos = snapshot.cursor;
        self.elements = snapshot.elements;
        self.next_element_id = next_id;
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        self.reset_edit_history();
    }

    /// IDs still reachable from the live buffer or its bounded undo history.
    /// Hosts use this to keep external element payloads bounded without
    /// breaking undo after an element deletion.
    pub fn retained_element_ids(&self) -> Vec<ElementId> {
        let mut seen = std::collections::HashSet::new();
        for element in self
            .elements
            .iter()
            .chain(
                self.undo_stack
                    .iter()
                    .flat_map(|snapshot| snapshot.elements.iter()),
            )
            .chain(
                self.redo_stack
                    .iter()
                    .flat_map(|snapshot| snapshot.elements.iter()),
            )
        {
            seen.insert(element.id);
        }
        seen.into_iter().collect()
    }

    /// Insert an atomic source token and return its stable identifier.
    pub fn insert_element(
        &mut self,
        text: &str,
        kind: ElementKind,
        display: ElementDisplay,
    ) -> Result<ElementId, ElementError> {
        validate_element_content(text, &display)?;
        let next_element_id = self
            .next_element_id
            .checked_add(1)
            .ok_or(ElementError::IdExhausted)?;
        let id = ElementId::new(self.next_element_id);
        self.next_element_id = next_element_id;
        let start = self.clamp_insertion_pos(self.cursor_pos);
        self.pre_mutate(MutationKind::InsertBlock, self.cursor_pos);
        self.text.insert_str(start, text);
        self.shift_elements_after_edit(start, start, text.len());
        let end = start + text.len();
        self.elements.push(TextElement {
            id,
            range: start..end,
            kind,
            display,
        });
        self.elements.sort_by_key(|element| element.range.start);
        self.cursor_pos = end;
        self.wrap_cache.replace(WrapCache::dirty());
        self.preferred_col = None;
        self.last_op_was_kill = false;
        self.note_mutation(MutationKind::InsertBlock, /*ends_run*/ true);
        Ok(id)
    }

    /// Re-register an element whose source token already exists in the buffer.
    /// Used for typed wire inputs whose source text and ranges arrive together.
    pub fn register_element(
        &mut self,
        range: Range<usize>,
        kind: ElementKind,
        display: ElementDisplay,
    ) -> Result<ElementId, ElementError> {
        if range.start >= range.end
            || range.end > self.text.len()
            || !self.text.is_char_boundary(range.start)
            || !self.text.is_char_boundary(range.end)
        {
            return Err(ElementError::InvalidRange);
        }
        if self
            .elements
            .iter()
            .any(|element| ranges_overlap(&element.range, &range))
        {
            return Err(ElementError::OverlappingRange);
        }
        let source = self
            .text
            .get(range.clone())
            .ok_or(ElementError::InvalidRange)?;
        validate_element_content(source, &display)?;
        let next_element_id = self
            .next_element_id
            .checked_add(1)
            .ok_or(ElementError::IdExhausted)?;
        let id = ElementId::new(self.next_element_id);
        self.next_element_id = next_element_id;
        self.elements.push(TextElement {
            id,
            range,
            kind,
            display,
        });
        self.elements.sort_by_key(|element| element.range.start);
        self.wrap_cache.replace(WrapCache::dirty());
        Ok(id)
    }

    /// Project a source range while fitting atomic element labels to the
    /// available row width. The source element remains indivisible.
    pub fn display_projection_with_width(&self, range: Range<usize>, width: u16) -> TextProjection {
        let start = self.clamp_pos_to_char_boundary(range.start.min(self.text.len()));
        let end = self.clamp_pos_to_char_boundary(range.end.min(self.text.len()));
        if start >= end {
            return TextProjection::default();
        }
        let mut projection = TextProjection::default();
        let mut source_pos = start;
        for element in &self.elements {
            if element.range.end <= start {
                continue;
            }
            if element.range.start >= end {
                break;
            }
            if element.range.start < start || element.range.end > end {
                continue;
            }
            if source_pos < element.range.start
                && let Some(plain) = self.text.get(source_pos..element.range.start)
            {
                projection.text.push_str(plain);
            }
            let display_start = projection.text.len();
            let display = element.display.fitted(usize::from(width));
            projection.text.push_str(display.text());
            let display_end = projection.text.len();
            projection.elements.push(ProjectedTextElement {
                range: display_start..display_end,
                display,
            });
            source_pos = element.range.end;
        }
        if source_pos < end
            && let Some(plain) = self.text.get(source_pos..end)
        {
            projection.text.push_str(plain);
        }
        projection
    }

    pub fn display_offset_with_width(
        &self,
        source_start: usize,
        source_pos: usize,
        width: u16,
    ) -> usize {
        self.display_projection_with_width(source_start..source_pos.min(self.text.len()), width)
            .text
            .len()
    }
}

fn validate_element_content(source: &str, display: &ElementDisplay) -> Result<(), ElementError> {
    if source.is_empty() {
        return Err(ElementError::EmptySource);
    }
    if source.contains(['\n', '\r']) {
        return Err(ElementError::MultilineSource);
    }
    if display.text().contains(['\n', '\r']) {
        return Err(ElementError::MultilineDisplay);
    }
    Ok(())
}

fn validate_snapshot(snapshot: &TextAreaSnapshot) -> Result<(), ElementError> {
    if snapshot.cursor > snapshot.text.len()
        || !snapshot.text.is_char_boundary(snapshot.cursor)
        || snapshot.elements.iter().any(|element| {
            snapshot.cursor > element.range.start && snapshot.cursor < element.range.end
        })
    {
        return Err(ElementError::InvalidCursor);
    }
    let mut previous_end = 0usize;
    let mut ids = std::collections::HashSet::new();
    for element in &snapshot.elements {
        if element.range.start < previous_end {
            return Err(ElementError::OverlappingRange);
        }
        if element.range.start >= element.range.end
            || element.range.end > snapshot.text.len()
            || !snapshot.text.is_char_boundary(element.range.start)
            || !snapshot.text.is_char_boundary(element.range.end)
        {
            return Err(ElementError::InvalidRange);
        }
        if !ids.insert(element.id) {
            return Err(ElementError::DuplicateId);
        }
        let source = snapshot
            .text
            .get(element.range.clone())
            .ok_or(ElementError::InvalidRange)?;
        validate_element_content(source, &element.display)?;
        previous_end = element.range.end;
    }
    Ok(())
}
