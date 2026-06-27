//! Tracks streaming tool call state across multiple deltas from an
//! OpenAI-compatible chat completion stream.
//!
//! Handles argument accumulation, emits tool-input-start/delta/end and
//! tool-call events, and finalizes unfinished tool calls on flush.
//!
//! Used by openai, openai-compatible, and other OpenAI-compatible providers.

use vercel_ai_provider::InvalidResponseDataError;
use vercel_ai_provider::JSONValue;
use vercel_ai_provider::LanguageModelV4StreamPart;
use vercel_ai_provider::LanguageModelV4ToolCall;
use vercel_ai_provider::ProviderMetadata;

use crate::generate_id::generate_tool_call_id;
use crate::json::is_parsable_json;

type ExtractMetadataFnObj =
    dyn Fn(&StreamingToolCallDelta) -> Option<ProviderMetadata> + Send + Sync;
type BuildMetadataFnObj =
    dyn Fn(Option<&ProviderMetadata>) -> Option<ProviderMetadata> + Send + Sync;
type ExtractMetadataFn = Box<ExtractMetadataFnObj>;
type BuildMetadataFn = Box<BuildMetadataFnObj>;

/// A single tool call delta from an OpenAI-compatible streaming response.
pub struct StreamingToolCallDelta {
    /// Index of this tool call in the chunk's `tool_calls` array.
    pub index: Option<usize>,
    /// Tool call ID.
    pub id: Option<String>,
    /// Type field (expected to be `"function"` for function tool calls).
    pub r#type: Option<String>,
    /// Function-specific fields.
    pub function: Option<ToolCallDeltaFunction>,
    /// Provider-specific extension fields (e.g. Google's
    /// `extra_content.google.thought_signature`). Forwarded to
    /// `extract_metadata` so callers can pull provider metadata out of the
    /// delta without modifying the tracker.
    pub extra: Option<JSONValue>,
}

/// Function-specific fields in a tool call delta.
pub struct ToolCallDeltaFunction {
    /// Tool name (only in the first delta for a given index).
    pub name: Option<String>,
    /// Incremental JSON arguments.
    pub arguments: Option<String>,
}

/// How to validate the `type` field on incoming tool call deltas.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum TypeValidation {
    /// No validation (default).
    #[default]
    None,
    /// Reject if type is present and not `"function"`.
    IfPresent,
    /// Reject if type is not exactly `"function"`.
    Required,
}

/// Options for `StreamingToolCallTracker`.
#[derive(Default)]
pub struct StreamingToolCallTrackerOptions {
    /// Custom ID generator (defaults to `generate_id`).
    pub generate_id: Option<Box<dyn Fn() -> String + Send + Sync>>,
    /// How to validate the `type` field.
    pub type_validation: TypeValidation,
    /// Extract provider metadata from a new tool call delta.
    pub extract_metadata: Option<ExtractMetadataFn>,
    /// Build `providerMetadata` for the final `tool-call` event.
    pub build_tool_call_provider_metadata: Option<BuildMetadataFn>,
}

struct TrackedToolCall {
    id: String,
    name: String,
    arguments: String,
    has_finished: bool,
    metadata: Option<ProviderMetadata>,
}

/// Tracks streaming tool call state and emits typed stream parts.
///
/// Call `process_delta` for each tool call delta in a streaming chunk, then
/// `flush` at the end of the stream to finalize any incomplete tool calls.
/// After each call, drain buffered parts via `take_parts`.
pub struct StreamingToolCallTracker {
    tool_calls: Vec<Option<TrackedToolCall>>,
    emitted: Vec<LanguageModelV4StreamPart>,
    generate_id: Box<dyn Fn() -> String + Send + Sync>,
    type_validation: TypeValidation,
    extract_metadata: Option<ExtractMetadataFn>,
    build_tool_call_provider_metadata: Option<BuildMetadataFn>,
}

impl StreamingToolCallTracker {
    /// Create with default options.
    pub fn new() -> Self {
        Self::with_options(StreamingToolCallTrackerOptions::default())
    }

    /// Create with custom options.
    pub fn with_options(opts: StreamingToolCallTrackerOptions) -> Self {
        Self {
            tool_calls: Vec::new(),
            emitted: Vec::new(),
            generate_id: opts
                .generate_id
                .unwrap_or_else(|| Box::new(generate_tool_call_id)),
            type_validation: opts.type_validation,
            extract_metadata: opts.extract_metadata,
            build_tool_call_provider_metadata: opts.build_tool_call_provider_metadata,
        }
    }

    /// Process one tool call delta from a streaming chunk.
    ///
    /// Returns `Err` if the delta is invalid (e.g. missing required fields).
    pub fn process_delta(
        &mut self,
        delta: StreamingToolCallDelta,
    ) -> Result<(), InvalidResponseDataError> {
        let index = delta.index.unwrap_or(self.tool_calls.len());

        while self.tool_calls.len() <= index {
            self.tool_calls.push(None);
        }

        if self.tool_calls[index].is_none() {
            self.process_new_tool_call(index, delta)?;
        } else {
            self.process_existing_tool_call(index, &delta);
        }
        Ok(())
    }

    /// Finalize any unfinished tool calls.
    ///
    /// Must be called at stream end to ensure all tool calls are completed.
    pub fn flush(&mut self) {
        for slot in &mut self.tool_calls {
            if let Some(tc) = slot
                && !tc.has_finished
            {
                Self::finish_tool_call_inner(
                    tc,
                    &mut self.emitted,
                    &self.generate_id,
                    self.build_tool_call_provider_metadata.as_deref(),
                );
            }
        }
    }

    /// Drain all buffered stream parts accumulated since the last call.
    pub fn take_parts(&mut self) -> Vec<LanguageModelV4StreamPart> {
        std::mem::take(&mut self.emitted)
    }

    fn process_new_tool_call(
        &mut self,
        index: usize,
        delta: StreamingToolCallDelta,
    ) -> Result<(), InvalidResponseDataError> {
        match self.type_validation {
            TypeValidation::Required => {
                if delta.r#type.as_deref() != Some("function") {
                    return Err(InvalidResponseDataError::with_message(
                        serde_json::Value::Null,
                        "Expected 'function' type.",
                    ));
                }
            }
            TypeValidation::IfPresent => {
                if let Some(ref t) = delta.r#type
                    && t != "function"
                {
                    return Err(InvalidResponseDataError::with_message(
                        serde_json::Value::Null,
                        "Expected 'function' type.",
                    ));
                }
            }
            TypeValidation::None => {}
        }

        let id = delta.id.clone().ok_or_else(|| {
            InvalidResponseDataError::with_message(
                serde_json::Value::Null,
                "Expected 'id' to be a string.",
            )
        })?;

        let name = delta
            .function
            .as_ref()
            .and_then(|f| f.name.clone())
            .ok_or_else(|| {
                InvalidResponseDataError::with_message(
                    serde_json::Value::Null,
                    "Expected 'function.name' to be a string.",
                )
            })?;

        self.emitted
            .push(LanguageModelV4StreamPart::ToolInputStart {
                id: id.clone(),
                tool_name: name.clone(),
                provider_executed: None,
                dynamic: None,
                title: None,
                provider_metadata: None,
            });

        let metadata = self.extract_metadata.as_ref().and_then(|f| f(&delta));
        let initial_args = delta
            .function
            .as_ref()
            .and_then(|f| f.arguments.clone())
            .unwrap_or_default();

        self.tool_calls[index] = Some(TrackedToolCall {
            id: id.clone(),
            name,
            arguments: initial_args.clone(),
            has_finished: false,
            metadata,
        });

        if !initial_args.is_empty() {
            self.emitted
                .push(LanguageModelV4StreamPart::ToolInputDelta {
                    id,
                    delta: initial_args.clone(),
                    provider_metadata: None,
                });
        }

        if is_parsable_json(&initial_args)
            && let Some(tc) = self.tool_calls[index].as_mut()
        {
            Self::finish_tool_call_inner(
                tc,
                &mut self.emitted,
                &self.generate_id,
                self.build_tool_call_provider_metadata.as_deref(),
            );
        }

        Ok(())
    }

    fn process_existing_tool_call(&mut self, index: usize, delta: &StreamingToolCallDelta) {
        let tc = match self.tool_calls[index].as_mut() {
            Some(tc) if !tc.has_finished => tc,
            _ => return,
        };

        if let Some(ref f) = delta.function
            && let Some(ref args) = f.arguments
        {
            tc.arguments.push_str(args);
            let id = tc.id.clone();
            self.emitted
                .push(LanguageModelV4StreamPart::ToolInputDelta {
                    id,
                    delta: args.clone(),
                    provider_metadata: None,
                });
        }

        let args = tc.arguments.clone();
        if is_parsable_json(&args)
            && let Some(tc) = self.tool_calls[index].as_mut()
        {
            Self::finish_tool_call_inner(
                tc,
                &mut self.emitted,
                &self.generate_id,
                self.build_tool_call_provider_metadata.as_deref(),
            );
        }
    }

    fn finish_tool_call_inner(
        tc: &mut TrackedToolCall,
        emitted: &mut Vec<LanguageModelV4StreamPart>,
        generate_id: &(dyn Fn() -> String + Send + Sync),
        build_metadata: Option<&BuildMetadataFnObj>,
    ) {
        emitted.push(LanguageModelV4StreamPart::ToolInputEnd {
            id: tc.id.clone(),
            provider_metadata: None,
        });

        let provider_metadata = build_metadata.and_then(|f| f(tc.metadata.as_ref()));
        let tool_call_id = if tc.id.is_empty() {
            generate_id()
        } else {
            tc.id.clone()
        };

        emitted.push(LanguageModelV4StreamPart::ToolCall(
            LanguageModelV4ToolCall {
                tool_call_id,
                tool_name: tc.name.clone(),
                input: tc.arguments.clone(),
                provider_executed: None,
                dynamic: None,
                provider_metadata,
                invalid: false,
                invalid_reason: None,
            },
        ));

        tc.has_finished = true;
    }
}

impl Default for StreamingToolCallTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[path = "streaming_tool_call_tracker.test.rs"]
mod tests;
