//! UTF-8-safe framing for OpenAI-wire Server-Sent Event streams.
//!
//! The chat-completions providers (OpenAI, OpenAI-compatible, Groq, …) all
//! consume the same `data: {json}\n\n … data: [DONE]` SSE shape over an async
//! byte stream. Each used to hand-roll the byte→line decoding, and each
//! carried the same latent bug: decoding every network chunk with
//! `String::from_utf8_lossy` corrupts any multi-byte UTF-8 sequence (CJK,
//! emoji) that straddles a chunk boundary.
//!
//! [`SseDecoder`] centralizes that framing once, correctly: it buffers raw
//! bytes and decodes only *complete* lines. `\n` is a single ASCII byte and can
//! never occur inside a multi-byte sequence, so every decoded line holds all of
//! its bytes and no character is ever split.

/// Accumulates raw SSE byte chunks and yields the `data:` payloads of complete
/// lines. Blank lines, non-`data:` lines, and the terminal `[DONE]` sentinel
/// are skipped.
#[derive(Debug, Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    /// Create an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a raw byte chunk as it arrives from the transport.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Pop the next complete `data:` payload, or `None` when no further
    /// complete line is buffered. Skips blank / non-`data:` / `[DONE]` lines
    /// internally, so a `Some` result is always a real event payload.
    pub fn next_data_line(&mut self) -> Option<String> {
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(&self.buffer[..pos])
                .trim_end_matches('\r')
                .to_string();
            self.buffer.drain(..=pos);
            if line.is_empty() {
                continue;
            }
            if let Some(data) = line
                .strip_prefix("data: ")
                .or_else(|| line.strip_prefix("data:"))
            {
                if data == "[DONE]" {
                    continue;
                }
                return Some(data.to_string());
            }
            // Non-data line (comment, event:, id:, …) — skip and keep scanning.
        }
        None
    }
}

#[cfg(test)]
#[path = "sse.test.rs"]
mod tests;
