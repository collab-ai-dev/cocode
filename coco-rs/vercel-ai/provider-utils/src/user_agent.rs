//! User-Agent header construction utilities.
//!
//! This module provides utilities for building User-Agent headers.

use std::env;

/// The default SDK name.
const SDK_NAME: &str = "vercel-ai-sdk-rust";

/// Build a User-Agent string.
///
/// # Arguments
///
/// * `provider_name` - The name of the provider (e.g., "openai", "anthropic").
/// * `provider_version` - The version of the provider implementation.
///
/// # Returns
///
/// A User-Agent string in the format:
/// `{sdk_name}/{sdk_version} ({rust_version}; {os}; {arch}) {provider_name}/{provider_version}`
pub fn build_user_agent(provider_name: &str, provider_version: &str) -> String {
    let sdk_version = env!("CARGO_PKG_VERSION");
    let rust_version = rustc_version_runtime::version();
    let os = env::consts::OS;
    let arch = env::consts::ARCH;

    format!(
        "{SDK_NAME}/{sdk_version} ({rust_version}; {os}; {arch}) {provider_name}/{provider_version}"
    )
}

/// Build a simple User-Agent string without system info.
///
/// # Arguments
///
/// * `provider_name` - The name of the provider.
/// * `provider_version` - The version of the provider implementation.
pub fn build_simple_user_agent(provider_name: &str, provider_version: &str) -> String {
    let sdk_version = env!("CARGO_PKG_VERSION");
    format!("{SDK_NAME}/{sdk_version} {provider_name}/{provider_version}")
}

/// Build a User-Agent string with custom SDK name.
///
/// # Arguments
///
/// * `sdk_name` - Custom SDK name.
/// * `sdk_version` - Custom SDK version.
/// * `provider_name` - The name of the provider.
/// * `provider_version` - The version of the provider implementation.
pub fn build_custom_user_agent(
    sdk_name: &str,
    sdk_version: &str,
    provider_name: &str,
    provider_version: &str,
) -> String {
    let os = env::consts::OS;
    let arch = env::consts::ARCH;

    format!("{sdk_name}/{sdk_version} ({os}; {arch}) {provider_name}/{provider_version}")
}

#[cfg(test)]
#[path = "user_agent.test.rs"]
mod tests;
