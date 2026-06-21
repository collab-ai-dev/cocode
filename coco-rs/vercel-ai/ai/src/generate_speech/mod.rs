//! Generate speech from text (text-to-speech).
//!
//! This module provides the `generate_speech` function for generating audio
//! from text using speech synthesis models.

#[allow(clippy::module_inception)]
mod generate_speech;
mod speech_result;

pub use generate_speech::GenerateSpeechOptions;
pub use generate_speech::SpeechModel;
pub use generate_speech::generate_speech;
pub use speech_result::GeneratedAudioFile;
pub use speech_result::SpeechResult;
