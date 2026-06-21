//! Stream processing for vercel-ai streaming responses.
//!
//! Provides [`StreamProcessor`], a thin adapter that wraps a
//! `Stream<LanguageModelV4StreamPart>` with idle-timeout enforcement and
//! health metrics (ttft, stall_count, total_stall_ms).
//!
//! # Non-goals
//!
//! This module deliberately does not accumulate stream content into a
//! per-stream snapshot. Different consumers want different accumulators
//! (e.g. coco-inference needs per-part `provider_metadata` fidelity for
//! round-tripping Gemini `thoughtSignature` / Anthropic `signature` /
//! OpenAI `encrypted_content`), so the policy lives with the consumer.
//!
//! # Example
//!
//! ```ignore
//! use vercel_ai::stream::StreamProcessor;
//!
//! let result = model.do_stream(options).await?;
//! let mut processor = StreamProcessor::from_stream(result.stream);
//!
//! while let Some(part) = processor.next().await {
//!     // process `part?` however the consumer wants
//! }
//! let metrics = processor.metrics();
//! ```

mod metrics;
mod processor;

pub use metrics::StreamMetrics;
pub use processor::StreamProcessor;
pub use processor::StreamProcessorConfig;
