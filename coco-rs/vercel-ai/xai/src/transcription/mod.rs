//! xAI transcription (speech-to-text) surface.
//!
//! Two paths, both hitting `/stt`:
//! - **Batch** (`do_transcribe`): multipart `POST /stt` with the audio as the
//!   final `file` field. Port of `doTranscribe`.
//! - **Streaming** (`do_stream`): real-time WebSocket STT. Port of `doStream` —
//!   see [`xai_transcription_stream`].

pub mod xai_transcription_model;
pub mod xai_transcription_options;
pub mod xai_transcription_stream;

pub use xai_transcription_model::XaiTranscriptionModel;
pub use xai_transcription_model::XaiTranscriptionResponse;
pub use xai_transcription_options::XaiKeyterm;
pub use xai_transcription_options::XaiStreamingOptions;
pub use xai_transcription_options::XaiTranscriptionAudioFormat;
pub use xai_transcription_options::XaiTranscriptionProviderOptions;
pub use xai_transcription_options::extract_xai_transcription_options;
