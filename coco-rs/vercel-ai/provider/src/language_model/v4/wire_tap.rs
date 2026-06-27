//! Raw wire-traffic capture seam for debugging.
//!
//! A [`WireTap`] is an optional sink threaded down to the HTTP transport
//! via [`LanguageModelV4CallOptions::wire_tap`](super::LanguageModelV4CallOptions).
//! The transport layer (`vercel-ai-provider-utils`) feeds it the raw
//! outgoing request bytes and the raw response bytes as they stream in;
//! it never interprets them. The *owner* of the tap (a higher layer such
//! as `app/query`) decides what to do with the captured bytes and when to
//! flush — the transport only reports, it does not judge.
//!
//! This crate deliberately carries no I/O, no redaction and no path
//! logic: those belong to the concrete implementor. Keeping the trait in
//! the dependency-free provider crate is what lets the field live on
//! `LanguageModelV4CallOptions` without dragging coco crates into the
//! transport layer.

use std::collections::HashMap;
use std::sync::Arc;

/// A debug sink for raw LLM wire traffic.
///
/// Implementors capture (and typically redact + persist) the bytes. All
/// methods are infallible by contract — a diagnostic sink must never
/// perturb the live request path, so implementations swallow their own
/// errors rather than propagating them.
pub trait WireTap: Send + Sync + std::fmt::Debug {
    /// Called once with the outgoing request, immediately before send.
    /// `headers` may contain credentials — the implementor is responsible
    /// for redaction.
    fn on_request(&self, url: &str, headers: &HashMap<String, String>, body: &[u8]);

    /// Called for each raw response chunk as it streams in (streaming
    /// calls). May be called many times; never after [`WireTap::on_response_body`].
    fn on_response_chunk(&self, chunk: &[u8]);

    /// Called once with the full response body for non-streaming calls,
    /// and for the error body of a failed HTTP response on either path.
    fn on_response_body(&self, status: u16, headers: &HashMap<String, String>, body: &[u8]);
}

/// Shared, cloneable handle to a [`WireTap`].
pub type WireTapHandle = Arc<dyn WireTap>;
