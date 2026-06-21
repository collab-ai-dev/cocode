//! End-to-end multimodal coverage for the `Tool::render_for_model`
//! refactor (TS parity: `mapToolResultToToolResultBlockParam`).
//!
//! Each test boots a real `coco_query::QueryEngine` driven by a
//! capturing scripted provider, registers the production
//! `coco_tools::ReadTool` / `coco_tools::BashTool`, and runs the
//! agent loop end-to-end with a real on-disk image fixture. The
//! scripted provider records the engine-assembled prompt on each
//! call so tests inspect what a real provider would have received
//! on turn 2 — i.e. the post-tool multimodal payload.
//!
//! No live provider is involved — these run in default `cargo test`
//! without any environment setup. Provider-specific wire conversion
//! (Anthropic image block / OpenAI Chat degradation / OpenAI
//! Responses native input_image) is covered by per-provider unit
//! tests inside each `vercel-ai-*` crate; the seam check forbids
//! `tests/live` from depending on those crates, so we stop one layer
//! up — at the normalized prompt — and trust those tests to catch
//! wire-shape drift.
//!
//! # Running
//!
//! ```bash
//! cargo test -p coco-tests-live --test multimodal_image
//! cargo test -p coco-tests-live --test multimodal_image png
//! ```

mod common;
mod multimodal;

use anyhow::Result;

#[tokio::test]
async fn test_multimodal_read_png_round_trip_via_real_loop() -> Result<()> {
    multimodal::scenarios::read_png_round_trip_via_real_loop().await
}

#[tokio::test]
async fn test_multimodal_read_jpeg_round_trip_via_real_loop() -> Result<()> {
    multimodal::scenarios::read_jpeg_round_trip_via_real_loop().await
}

#[tokio::test]
async fn test_multimodal_read_webp_round_trip_via_real_loop() -> Result<()> {
    multimodal::scenarios::read_webp_round_trip_via_real_loop().await
}

#[tokio::test]
async fn test_multimodal_read_text_uses_text_fast_path_in_real_loop() -> Result<()> {
    multimodal::scenarios::read_text_uses_text_fast_path_in_real_loop().await
}

#[tokio::test]
async fn test_multimodal_bash_cat_image_round_trip_via_real_loop() -> Result<()> {
    multimodal::scenarios::bash_cat_image_round_trip_via_real_loop().await
}
