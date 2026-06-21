//! Paste handling — bracketed paste mode and inline paste pills.
//!
//! When the user pastes content, it's wrapped as a paste pill `[Pasted text #N]`
//! and stored in a cache. The pill is displayed inline in the input while the
//! full content is attached when the message is submitted.

/// Pasted text longer than this (in chars) is stored as a pill instead of
/// flooding the composer; it expands back to the full content at submit.
/// Mirrors codex-rs `LARGE_PASTE_CHAR_THRESHOLD`.
pub const LARGE_PASTE_CHAR_THRESHOLD: usize = 1000;

/// Check if text matches a paste pill pattern.
///
/// Valid patterns: `[Pasted text #N]`, `[Image #N]`
pub fn is_paste_pill(text: &str) -> bool {
    if !text.starts_with('[') || !text.ends_with(']') {
        return false;
    }
    let inner = &text[1..text.len() - 1];
    inner.starts_with("Pasted text #") || inner.starts_with("Image #")
}

/// Image data extracted from a paste pill.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Resolved input after expanding paste pills.
#[derive(Debug, Clone)]
pub struct ResolvedInput {
    /// Text with paste pills resolved: text pills expand to their content,
    /// image pills are kept inline as `[Image #N]` (the raw bytes ship
    /// separately in `images`). Mirrors TS `expandPastedTextRefs`, which
    /// skips image refs so the placeholder survives into the message text.
    pub text: String,
    /// Image data extracted from image pills.
    pub images: Vec<ImageData>,
}

/// Paste entry stored in the cache.
#[derive(Debug, Clone)]
pub struct PasteEntry {
    /// The paste pill label (e.g., "[Pasted text #1]").
    pub pill: String,
    /// The actual pasted content (text content or file path for images).
    pub content: String,
    /// Whether this is an image paste.
    pub is_image: bool,
    /// Raw image bytes (only present when `is_image` is true).
    pub image_bytes: Option<Vec<u8>>,
    /// MIME type for image entries.
    pub image_mime: Option<String>,
}

/// Paste manager that tracks paste pills.
#[derive(Debug, Default)]
pub struct PasteManager {
    entries: Vec<PasteEntry>,
}

impl PasteManager {
    /// Create a new paste manager.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a text paste. Returns the pill label.
    pub fn add_text(&mut self, content: String) -> String {
        let n = self.entries.len() + 1;
        let pill = format!("[Pasted text #{n}]");
        self.entries.push(PasteEntry {
            pill: pill.clone(),
            content,
            is_image: false,
            image_bytes: None,
            image_mime: None,
        });
        pill
    }

    /// Add an image paste with raw bytes. Returns the pill label.
    ///
    /// There is deliberately no path-only variant: an image entry without
    /// bytes is silently dropped by [`Self::resolve_structured`] at submit,
    /// so callers must load the bytes first.
    pub fn add_image_data(&mut self, bytes: Vec<u8>, mime: String) -> String {
        let n = self.entries.len() + 1;
        let pill = format!("[Image #{n}]");
        self.entries.push(PasteEntry {
            pill: pill.clone(),
            content: String::new(),
            is_image: true,
            image_bytes: Some(bytes),
            image_mime: Some(mime),
        });
        pill
    }

    pub fn entries(&self) -> &[PasteEntry] {
        &self.entries
    }

    /// Resolve paste pills in the input, returning the expanded content.
    ///
    /// Simple string replacement — text pills become content, image pills become
    /// their file path (or empty string if bytes-only).
    pub fn resolve(&self, input: &str) -> String {
        let mut result = input.to_string();
        for entry in &self.entries {
            result = result.replace(&entry.pill, &entry.content);
        }
        result
    }

    /// Resolve paste pills, separating text expansions from image data.
    ///
    /// Text pills are expanded inline. Image pills are kept inline as their
    /// `[Image #N]` placeholder (so the transcript can echo `❯ [Image #N] …`
    /// and hang a `⎿ [Image #N]` confirmation row), while their raw bytes are
    /// returned separately for API content-block assembly. Mirrors TS
    /// `expandPastedTextRefs`, which expands text refs but leaves image refs in
    /// place.
    pub fn resolve_structured(&self, input: &str) -> ResolvedInput {
        let mut text = input.to_string();
        let mut images = Vec::new();

        for entry in &self.entries {
            if entry.is_image {
                // Keep the `[Image #N]` placeholder inline; only collect the
                // bytes for the separate image content block.
                if let (Some(bytes), Some(mime)) = (&entry.image_bytes, &entry.image_mime) {
                    images.push(ImageData {
                        bytes: bytes.clone(),
                        mime: mime.clone(),
                    });
                }
            } else {
                // Expand text pill inline
                text = text.replace(&entry.pill, &entry.content);
            }
        }

        let text = text.trim().to_string();

        ResolvedInput { text, images }
    }

    /// Clear all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Take ownership of all entries, leaving the manager empty.
    /// Used by `chat:stash` to snapshot paste state alongside text +
    /// cursor (TS `handleStash` saves `pastedContents`).
    pub fn take_entries(&mut self) -> Vec<PasteEntry> {
        std::mem::take(&mut self.entries)
    }

    /// Replace all entries — counterpart to [`Self::take_entries`].
    /// Used on stash-pop to restore the saved paste state. Replaces
    /// rather than appends so pop is symmetric with push.
    pub fn replace_entries(&mut self, entries: Vec<PasteEntry>) {
        self.entries = entries;
    }
}

#[cfg(test)]
#[path = "paste.test.rs"]
mod tests;
