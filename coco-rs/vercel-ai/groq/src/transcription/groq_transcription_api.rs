use serde::Deserialize;
use serde::Serialize;

/// Groq transcription response shape. Segment fields are present only when
/// `response_format: "verbose_json"` is requested. Mirrors
/// `groqTranscriptionResponseSchema`.
#[derive(Debug, Deserialize, Serialize)]
pub struct GroqTranscriptionResponse {
    pub text: String,
    pub x_groq: Option<GroqTranscriptionXGroq>,
    pub task: Option<String>,
    pub language: Option<String>,
    pub duration: Option<f64>,
    #[serde(default)]
    pub segments: Option<Vec<GroqTranscriptionSegment>>,
}

/// The `x_groq` extension object on a transcription response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroqTranscriptionXGroq {
    pub id: Option<String>,
}

/// A segment in a verbose Groq transcription response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GroqTranscriptionSegment {
    pub id: Option<u64>,
    pub seek: Option<u64>,
    pub text: String,
    pub start: f64,
    pub end: f64,
    pub tokens: Option<Vec<u64>>,
    pub temperature: Option<f64>,
    pub avg_logprob: Option<f64>,
    pub compression_ratio: Option<f64>,
    pub no_speech_prob: Option<f64>,
}
