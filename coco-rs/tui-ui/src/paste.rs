//! Clipboard transfer data.
//!
//! Composer attachment ownership lives in `coco-tui`; this lower-level crate
//! only returns bytes read from the platform clipboard.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    pub bytes: Vec<u8>,
    pub mime: String,
}
