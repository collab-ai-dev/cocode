//! Bridges the provider transport's `WireTap` sink to the
//! provider-agnostic `coco-wire-dump` recorder.
//!
//! This adapter lives here — not in `coco-wire-dump` — so the recorder
//! crate stays a leaf free of any `coco-inference` / `vercel-ai`
//! dependency. `app/query` already depends on `coco-inference`, so the
//! `WireTap` knowledge belongs here.

use std::collections::HashMap;
use std::sync::Arc;

use coco_inference::WireTap;
use coco_wire_dump::SessionWireRecorder;

/// Forwards the three `WireTap` callbacks to the recorder's inherent
/// capture methods. `finish` is driven separately by the engine on the
/// concrete recorder handle (it is not part of `WireTap`).
#[derive(Debug)]
pub(crate) struct WireTapAdapter(pub(crate) Arc<SessionWireRecorder>);

impl WireTap for WireTapAdapter {
    fn on_request(&self, url: &str, headers: &HashMap<String, String>, body: &[u8]) {
        self.0.on_request(url, headers, body);
    }

    fn on_response_chunk(&self, chunk: &[u8]) {
        self.0.on_response_chunk(chunk);
    }

    fn on_response_body(&self, status: u16, headers: &HashMap<String, String>, body: &[u8]) {
        self.0.on_response_body(status, headers, body);
    }
}
