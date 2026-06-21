//! In-process multimodal integration test suite. See `multimodal_image.rs`
//! for the test-target entry points.
//!
//! Layout:
//!
//! - `fixtures` — real PNG/JPEG/WebP byte builders (1×1 pixels, valid
//!   encodings).
//! - `scripted_model` — capturing [`LanguageModel`] mock that records
//!   the engine-assembled prompt on each call so tests can read it back.
//! - `harness` — `run_multimodal_scenario` boots a real `QueryEngine`
//!   with the scripted model + the production `ReadTool`/`BashTool`,
//!   runs the agent loop end-to-end, and returns captured prompts +
//!   events.
//! - `scenarios` — one `pub async fn` per scenario, called from the
//!   `multimodal_image.rs` test target.
//!
//! [`LanguageModel`]: coco_inference::LanguageModel

pub mod fixtures;
pub mod harness;
pub mod scenarios;
pub mod scripted_model;
