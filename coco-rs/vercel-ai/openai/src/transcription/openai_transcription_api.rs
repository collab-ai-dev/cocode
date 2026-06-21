use serde::Deserialize;
use serde::Serialize;

/// OpenAI API transcription response shape (verbose_json format).
#[derive(Debug, Deserialize, Serialize)]
pub struct OpenAITranscriptionResponse {
    pub text: String,
    pub language: Option<String>,
    pub duration: Option<f64>,
    pub words: Option<Vec<OpenAITranscriptionWord>>,
    #[serde(default)]
    pub segments: Option<Vec<OpenAITranscriptionSegment>>,
}

/// A word in the OpenAI transcription response with timing information.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAITranscriptionWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
}

/// A segment in the OpenAI transcription response.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAITranscriptionSegment {
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

#[cfg(test)]
#[path = "openai_transcription_api.test.rs"]
mod tests;
