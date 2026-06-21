//! Reasoning output type.
//!
//! This module provides the `ReasoningOutput` type which represents
//! structured reasoning content from model responses, including
//! the text, optional signature, and provider metadata.

use vercel_ai_provider::AssistantContentPart;
use vercel_ai_provider::FileRawData;
use vercel_ai_provider::LanguageModelV4FileData;
use vercel_ai_provider::ProviderMetadata;
use vercel_ai_provider::ReasoningFilePart;
use vercel_ai_provider::ReasoningPart;

use super::generated_file::GeneratedFile;

/// Structured reasoning output from a model response.
///
/// This type represents a single reasoning block with `text`, `signature`,
/// and `providerMetadata`.
#[derive(Debug, Clone)]
pub struct ReasoningOutput {
    /// The reasoning text content.
    pub text: String,
    /// Optional signature for the reasoning (e.g., for verification).
    pub signature: Option<String>,
    /// Provider-specific metadata for this reasoning block.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ReasoningOutput {
    /// Create a new reasoning output with just text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            signature: None,
            provider_metadata: None,
        }
    }

    /// Set the signature.
    pub fn with_signature(mut self, signature: impl Into<String>) -> Self {
        self.signature = Some(signature.into());
        self
    }

    /// Set the provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A reasoning file output from a model response.
///
/// Contains file data (e.g., an image) that is part of reasoning,
/// as used by Google Gemini's thought images.
#[derive(Debug, Clone)]
pub struct ReasoningFileOutput {
    /// The generated file containing the reasoning data.
    pub file: GeneratedFile,
    /// Provider-specific metadata.
    pub provider_metadata: Option<ProviderMetadata>,
}

impl ReasoningFileOutput {
    /// Create a new reasoning file output.
    pub fn new(file: GeneratedFile) -> Self {
        Self {
            file,
            provider_metadata: None,
        }
    }

    /// Set the provider metadata.
    pub fn with_provider_metadata(mut self, metadata: ProviderMetadata) -> Self {
        self.provider_metadata = Some(metadata);
        self
    }
}

/// A reasoning output item: either text-based or file-based.
#[derive(Debug, Clone)]
pub enum ReasoningOutputItem {
    /// Text-based reasoning output.
    Text(ReasoningOutput),
    /// File-based reasoning output (e.g., thought images).
    File(ReasoningFileOutput),
}

impl ReasoningOutputItem {
    /// Get the text if this is a text reasoning output.
    pub fn as_text(&self) -> Option<&ReasoningOutput> {
        match self {
            Self::Text(t) => Some(t),
            Self::File(_) => None,
        }
    }
}

/// Convert reasoning output items back to provider-level content parts.
pub fn convert_from_reasoning_outputs(items: &[ReasoningOutputItem]) -> Vec<AssistantContentPart> {
    items
        .iter()
        .map(|item| match item {
            ReasoningOutputItem::Text(r) => {
                let mut part = ReasoningPart::new(&r.text);
                if let Some(ref pm) = r.provider_metadata {
                    part.provider_metadata = Some(pm.clone());
                }
                AssistantContentPart::Reasoning(part)
            }
            ReasoningOutputItem::File(rf) => {
                let mut part =
                    ReasoningFilePart::from_base64(&rf.file.content, &rf.file.media_type);
                if let Some(ref pm) = rf.provider_metadata {
                    part.provider_metadata = Some(pm.clone());
                }
                AssistantContentPart::ReasoningFile(part)
            }
        })
        .collect()
}

/// Convert provider-level content parts to reasoning output items.
pub fn convert_to_reasoning_outputs(parts: &[AssistantContentPart]) -> Vec<ReasoningOutputItem> {
    parts
        .iter()
        .filter_map(|part| match part {
            AssistantContentPart::Reasoning(r) => {
                let mut output = ReasoningOutput::new(&r.text);
                if let Some(ref pm) = r.provider_metadata {
                    output.provider_metadata = Some(pm.clone());
                }
                Some(ReasoningOutputItem::Text(output))
            }
            AssistantContentPart::ReasoningFile(rf) => {
                let file = match &rf.data {
                    LanguageModelV4FileData::Data {
                        data: FileRawData::Base64(b),
                    } => GeneratedFile::from_base64("reasoning-file", b, &rf.media_type),
                    LanguageModelV4FileData::Url { url } => {
                        GeneratedFile::new("reasoning-file", url, &rf.media_type)
                    }
                    LanguageModelV4FileData::Data {
                        data: FileRawData::Bytes(bytes),
                    } => {
                        use base64::Engine as _;
                        use base64::engine::general_purpose::STANDARD;
                        GeneratedFile::from_base64(
                            "reasoning-file",
                            STANDARD.encode(bytes),
                            &rf.media_type,
                        )
                    }
                };
                let mut output = ReasoningFileOutput::new(file);
                if let Some(ref pm) = rf.provider_metadata {
                    output.provider_metadata = Some(pm.clone());
                }
                Some(ReasoningOutputItem::File(output))
            }
            _ => None,
        })
        .collect()
}

/// Get the combined reasoning text from reasoning output items (text items only).
pub fn reasoning_text_from_items(items: &[ReasoningOutputItem]) -> String {
    items
        .iter()
        .filter_map(|item| item.as_text().map(|r| r.text.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Get the combined reasoning text from reasoning outputs.
pub fn reasoning_text(outputs: &[ReasoningOutput]) -> String {
    outputs
        .iter()
        .map(|r| r.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "reasoning_output.test.rs"]
mod tests;
