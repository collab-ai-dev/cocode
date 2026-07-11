//! xAI speech (text-to-speech) surface.
//!
//! `POST /tts` with a JSON body, binary audio response. Port of
//! `xai-speech-model.ts` / `xai-speech-model-options.ts`.

pub mod xai_speech_model;
pub mod xai_speech_options;

pub use xai_speech_model::XaiSpeechModel;
pub use xai_speech_options::XaiSpeechProviderOptions;
pub use xai_speech_options::extract_xai_speech_options;
