//! Bounded, UTF-8-safe Server-Sent Event framing.

use bytes::BytesMut;

/// Default maximum size of one SSE event, including framing fields.
pub const DEFAULT_MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;

/// One fully framed SSE event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Failure while framing an SSE byte stream.
#[derive(Debug, thiserror::Error)]
pub enum SseDecodeError {
    #[error("SSE event exceeds the {limit}-byte safety limit")]
    EventTooLarge { limit: usize },
    #[error("SSE event is not valid UTF-8: {source}")]
    InvalidUtf8 {
        #[from]
        source: std::str::Utf8Error,
    },
}

/// Accumulates raw transport chunks and yields complete SSE events.
///
/// Bytes are decoded only after a full line is available, so a UTF-8 scalar
/// split across network chunks is never replaced or corrupted. The decoder
/// implements event boundaries, `event:` fields, and multi-line `data:`
/// joining rather than exposing provider-specific line framing.
#[derive(Debug)]
pub struct SseDecoder {
    buffer: BytesMut,
    event_type: Option<String>,
    data_lines: Vec<String>,
    event_bytes: usize,
    max_event_bytes: usize,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::with_max_event_bytes(DEFAULT_MAX_SSE_EVENT_BYTES)
    }

    pub fn with_max_event_bytes(max_event_bytes: usize) -> Self {
        Self {
            buffer: BytesMut::new(),
            event_type: None,
            data_lines: Vec::new(),
            event_bytes: 0,
            max_event_bytes,
        }
    }

    /// Append a raw transport chunk.
    pub fn push(&mut self, bytes: &[u8]) -> Result<(), SseDecodeError> {
        self.buffer.extend_from_slice(bytes);
        self.ensure_partial_event_is_bounded()
    }

    /// Yield the next complete event, if one is buffered.
    pub fn next_event(&mut self) -> Result<Option<SseEvent>, SseDecodeError> {
        while let Some(line_end) = self.buffer.iter().position(|&byte| byte == b'\n') {
            self.charge(line_end.saturating_add(1))?;
            let mut raw_line = self.buffer.split_to(line_end.saturating_add(1));
            raw_line.truncate(line_end);
            if raw_line.last() == Some(&b'\r') {
                raw_line.truncate(raw_line.len().saturating_sub(1));
            }
            let line = std::str::from_utf8(&raw_line)?;

            if line.is_empty() {
                self.event_bytes = 0;
                let event_type = self.event_type.take();
                if self.data_lines.is_empty() {
                    continue;
                }
                return Ok(Some(SseEvent {
                    event: event_type,
                    data: std::mem::take(&mut self.data_lines).join("\n"),
                }));
            }
            if line.starts_with(':') {
                continue;
            }

            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "event" => self.event_type = Some(value.to_string()),
                "data" => self.data_lines.push(value.to_string()),
                "id" | "retry" | "" => {}
                _ => {}
            }
        }

        self.ensure_partial_event_is_bounded()?;
        Ok(None)
    }

    /// Flush a final event when the transport closes without a blank line.
    pub fn finish(&mut self) -> Result<Option<SseEvent>, SseDecodeError> {
        if self.buffer.is_empty() && self.data_lines.is_empty() {
            return Ok(None);
        }
        if self.buffer.last() != Some(&b'\n') {
            self.buffer.extend_from_slice(b"\n");
        }
        self.buffer.extend_from_slice(b"\n");
        self.next_event()
    }

    fn charge(&mut self, bytes: usize) -> Result<(), SseDecodeError> {
        self.event_bytes = self.event_bytes.saturating_add(bytes);
        if self.event_bytes > self.max_event_bytes {
            return Err(SseDecodeError::EventTooLarge {
                limit: self.max_event_bytes,
            });
        }
        Ok(())
    }

    fn ensure_partial_event_is_bounded(&self) -> Result<(), SseDecodeError> {
        // Bound the entire undecoded transport buffer, not only the trailing
        // partial line. Otherwise a malicious chunk containing many newline-
        // terminated fields could grow memory before `next_event` runs.
        if self.event_bytes.saturating_add(self.buffer.len()) > self.max_event_bytes {
            return Err(SseDecodeError::EventTooLarge {
                limit: self.max_event_bytes,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "sse.test.rs"]
mod tests;
