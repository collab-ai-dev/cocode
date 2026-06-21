//! Test utilities for the vercel-ai crate.
//!
//! Provides mock models and providers for testing generate_text, stream_text, etc.

mod mock_embedding_model;
mod mock_image_model;
mod mock_language_model;
mod mock_provider;
mod mock_reranking_model;
mod mock_speech_model;
mod mock_transcription_model;
mod mock_video_model;

pub use mock_embedding_model::MockEmbeddingModel;
pub use mock_embedding_model::MockEmbeddingModelBuilder;
pub use mock_image_model::MockImageModel;
pub use mock_image_model::MockImageModelBuilder;
pub use mock_language_model::MockLanguageModel;
pub use mock_language_model::MockLanguageModelBuilder;
pub use mock_provider::MockProvider;
pub use mock_reranking_model::MockRerankingModel;
pub use mock_reranking_model::MockRerankingModelBuilder;
pub use mock_speech_model::MockSpeechModel;
pub use mock_speech_model::MockSpeechModelBuilder;
pub use mock_transcription_model::MockTranscriptionModel;
pub use mock_transcription_model::MockTranscriptionModelBuilder;
pub use mock_video_model::MockVideoModel;
pub use mock_video_model::MockVideoModelBuilder;
