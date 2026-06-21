//! Per-feature DeepSeek test suite. Each module exposes one or more
//! `run*` functions that take a `LiveTarget` (or pair of targets, for
//! cross-protocol) and return `anyhow::Result<()>`.

pub mod basic;
pub mod cross_protocol;
pub mod inference_smoke;
pub mod streaming;
pub mod tools;
pub mod vision;
