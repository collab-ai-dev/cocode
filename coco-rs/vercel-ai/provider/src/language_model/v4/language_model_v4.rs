//! Language model trait (V4).
//!
//! This module defines the `LanguageModelV4` trait for implementing language models
//! that follow the Vercel AI SDK v4 specification.

use async_trait::async_trait;
use regex::Regex;
use std::collections::HashMap;
use tokio_util::sync::CancellationToken;

use super::LanguageModelV4CallOptions;
use super::LanguageModelV4GenerateResult;
use super::LanguageModelV4StreamResult;
use crate::errors::AISdkError;

/// The language model trait (V4).
///
/// This trait defines the interface for language models following the
/// Vercel AI SDK v4 specification.
///
/// ## Why `options` is borrowed and `abort_signal` is separate
///
/// `LanguageModelV4CallOptions` is a **read-only request specification**
/// — prompt, tools, parameters. Providers only read these fields to
/// build the HTTP body, never mutate. Passing by `&` lets callers
/// (notably coco-inference runtime retry loops) reuse
/// the same options for multiple attempts without per-attempt
/// `Vec<LlmMessage>::clone`.
///
/// `abort_signal` is a **live cancellation handle** — semantically a
/// different concept from the request spec. It's `Arc`-backed (cheap
/// clone) and forwarded into the HTTP client to support `tokio::select!`
/// cancellation. Keeping it out of `LanguageModelV4CallOptions` means
/// the options struct contains no live handles and is trivially
/// `Clone`-cheap on the rare paths that need ownership.
#[async_trait]
pub trait LanguageModelV4: Send + Sync {
    /// Get the specification version.
    fn specification_version(&self) -> &'static str {
        "v4"
    }

    /// Get the provider name.
    fn provider(&self) -> &str;

    /// Get the model ID.
    fn model_id(&self) -> &str;

    /// Get the supported URL patterns for this model.
    ///
    /// Returns a map of URL schemes to regex patterns that match supported URLs.
    fn supported_urls(&self) -> HashMap<String, Vec<Regex>> {
        HashMap::new()
    }

    /// Generate a response.
    async fn do_generate(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelV4GenerateResult, AISdkError>;

    /// Generate a streaming response.
    async fn do_stream(
        &self,
        options: &LanguageModelV4CallOptions,
        abort_signal: Option<CancellationToken>,
    ) -> Result<LanguageModelV4StreamResult, AISdkError>;
}

#[cfg(test)]
#[path = "language_model_v4.test.rs"]
mod tests;
