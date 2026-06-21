//! Version constant for the SDK.

/// The version of the Vercel AI SDK Rust implementation.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
#[path = "version.test.rs"]
mod tests;
