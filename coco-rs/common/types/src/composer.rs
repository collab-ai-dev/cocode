use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;

/// Image payload paired with a queued command edit restore.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedCommandEditImage {
    pub media_type: String,
    pub data_base64: String,
    /// Byte position in `prompt` where the atomic image chip should be restored.
    #[serde(default)]
    pub insertion_offset: i64,
}

/// Lossless, payload-light description of a composer after it has been
/// resolved into the model-facing prompt. Paste payloads are addressed by
/// ranges in that prompt; image bytes remain in typed file parts.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedComposer {
    pub next_attachment_label: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<SubmittedComposerElement>,
}

#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmittedComposerElement {
    Paste {
        start: i64,
        end: i64,
        label: String,
    },
    Image {
        insertion_offset: i64,
        image_index: i64,
        label: String,
    },
    FileRef {
        start: i64,
        end: i64,
    },
}

impl SubmittedComposer {
    /// Validate the payload-light composer description against the resolved
    /// prompt and its separately stored image parts.
    pub fn is_valid_for(&self, text: &str, image_count: usize) -> bool {
        if self.next_attachment_label < 0 {
            return false;
        }
        let mut source_pos = 0usize;
        let mut image_indices = HashSet::with_capacity(image_count);
        let mut labels = HashSet::new();
        for element in &self.elements {
            match element {
                SubmittedComposerElement::Paste { start, end, label } => {
                    let Some(range) = valid_range(text, *start, *end, source_pos) else {
                        return false;
                    };
                    if !valid_label(label, "[Pasted text #", self.next_attachment_label)
                        || !labels.insert(label)
                    {
                        return false;
                    }
                    source_pos = range.end;
                }
                SubmittedComposerElement::Image {
                    insertion_offset,
                    image_index,
                    label,
                } => {
                    let Ok(offset) = usize::try_from(*insertion_offset) else {
                        return false;
                    };
                    let Ok(index) = usize::try_from(*image_index) else {
                        return false;
                    };
                    if offset < source_pos
                        || offset > text.len()
                        || !text.is_char_boundary(offset)
                        || index >= image_count
                        || !image_indices.insert(index)
                        || !valid_label(label, "[Image #", self.next_attachment_label)
                        || !labels.insert(label)
                    {
                        return false;
                    }
                }
                SubmittedComposerElement::FileRef { start, end } => {
                    let Some(range) = valid_range(text, *start, *end, source_pos) else {
                        return false;
                    };
                    source_pos = range.end;
                }
            }
        }
        image_indices.len() == image_count
    }

    /// Validate both the composer structure and its image-to-file-part anchors.
    pub fn is_valid_for_images(&self, text: &str, images: &[QueuedCommandEditImage]) -> bool {
        self.is_valid_for(text, images.len())
            && self.elements.iter().all(|element| match element {
                SubmittedComposerElement::Image {
                    insertion_offset,
                    image_index,
                    ..
                } => usize::try_from(*image_index)
                    .ok()
                    .and_then(|index| images.get(index))
                    .is_some_and(|image| image.insertion_offset == *insertion_offset),
                SubmittedComposerElement::Paste { .. }
                | SubmittedComposerElement::FileRef { .. } => true,
            })
    }
}

fn valid_range(
    text: &str,
    start: i64,
    end: i64,
    prior_end: usize,
) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(start).ok()?;
    let end = usize::try_from(end).ok()?;
    (start >= prior_end
        && start < end
        && end <= text.len()
        && text.is_char_boundary(start)
        && text.is_char_boundary(end))
    .then_some(start..end)
}

fn valid_label(label: &str, prefix: &str, next_label: i64) -> bool {
    label
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|value| value.parse::<i64>().ok())
        .is_some_and(|value| value > 0 && value <= next_label)
}

/// Cross-session composer element with an exact source anchor.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PersistedComposerElement {
    Paste {
        start: i64,
        end: i64,
        content: String,
    },
    Image {
        start: i64,
        end: i64,
        media_type: String,
        data_base64: String,
    },
    FileRef {
        start: i64,
        end: i64,
    },
}

/// Typed persistent history representation. The session layer stores image
/// bytes in a content-addressed attachment store rather than inline in JSONL.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedComposer {
    pub text: String,
    /// Monotonic label allocator state. This is persisted explicitly so
    /// deleting an older attachment cannot make a later restore reuse its
    /// human-visible chip number.
    pub next_attachment_label: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<PersistedComposerElement>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submitted_composer_validates_unicode_ranges_and_image_anchors() {
        let images = vec![QueuedCommandEditImage {
            media_type: "image/png".into(),
            data_base64: "AQ==".into(),
            insertion_offset: 6,
        }];
        let composer = SubmittedComposer {
            next_attachment_label: 2,
            elements: vec![
                SubmittedComposerElement::Paste {
                    start: 3,
                    end: 5,
                    label: "[Pasted text #1]".into(),
                },
                SubmittedComposerElement::Image {
                    insertion_offset: 6,
                    image_index: 0,
                    label: "[Image #2]".into(),
                },
                SubmittedComposerElement::FileRef { start: 6, end: 8 },
            ],
        };

        assert!(composer.is_valid_for_images("你α @x", &images));
    }

    #[test]
    fn submitted_composer_rejects_duplicate_images_and_forged_labels() {
        let images = vec![QueuedCommandEditImage {
            media_type: "image/png".into(),
            data_base64: "AQ==".into(),
            insertion_offset: 0,
        }];
        let duplicate = SubmittedComposer {
            next_attachment_label: 2,
            elements: vec![
                SubmittedComposerElement::Image {
                    insertion_offset: 0,
                    image_index: 0,
                    label: "[Image #1]".into(),
                },
                SubmittedComposerElement::Image {
                    insertion_offset: 0,
                    image_index: 0,
                    label: "[Image #2]".into(),
                },
            ],
        };
        assert!(!duplicate.is_valid_for_images("", &images));

        let forged = SubmittedComposer {
            next_attachment_label: 1,
            elements: vec![SubmittedComposerElement::Paste {
                start: 0,
                end: 1,
                label: "[Image #1]".into(),
            }],
        };
        assert!(!forged.is_valid_for("x", 0));
    }
}
