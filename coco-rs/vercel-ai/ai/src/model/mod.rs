//! Model resolution module.

mod resolve_model;

pub use resolve_model::EmbeddingModel;
pub use resolve_model::ImageModelRef;
pub use resolve_model::LanguageModel;
pub use resolve_model::RerankingModelRef;
pub use resolve_model::SpeechModelRef;
pub use resolve_model::TranscriptionModelRef;
pub use resolve_model::VideoModelRef;
pub use resolve_model::resolve_embedding_model;
pub use resolve_model::resolve_embedding_model_with_provider;
pub use resolve_model::resolve_image_model;
pub use resolve_model::resolve_image_model_with_provider;
pub use resolve_model::resolve_language_model;
pub use resolve_model::resolve_language_model_id;
pub use resolve_model::resolve_language_model_with_provider;
pub use resolve_model::resolve_reranking_model;
pub use resolve_model::resolve_reranking_model_with_provider;
pub use resolve_model::resolve_speech_model;
pub use resolve_model::resolve_speech_model_with_provider;
pub use resolve_model::resolve_transcription_model;
pub use resolve_model::resolve_transcription_model_with_provider;
pub use resolve_model::resolve_video_model;
pub use resolve_model::resolve_video_model_with_provider;
